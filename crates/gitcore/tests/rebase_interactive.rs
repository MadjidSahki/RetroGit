#![allow(clippy::unwrap_used)]
//! Interactive rebase through `git rebase -i` (needs `git`).
mod common;

use std::path::Path;

use common::remote::{configure, git};
use gitcore::{OpOutcome, Operation, Repo, TodoAction, TodoItem};

fn commit(dir: &Path, file: &str, text: &str, msg: &str) {
    std::fs::write(dir.join(file), text).unwrap();
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", msg]);
}

/// base, then four commits: one, two, three, four.
fn repo(dir: &Path) -> (Repo, String) {
    git(dir, &["-c", "init.defaultBranch=main", "init", "-q"]);
    configure(dir);
    commit(dir, "base.txt", "b\n", "base");
    let base = git(dir, &["rev-parse", "HEAD"]).trim().to_string();
    for (i, name) in ["one", "two", "three", "four"].iter().enumerate() {
        commit(dir, &format!("{name}.txt"), &format!("{i}\n"), name);
    }
    (Repo::open(dir).unwrap(), base)
}

fn subjects(dir: &Path) -> Vec<String> {
    git(dir, &["log", "--format=%s", "--reverse"])
        .lines()
        .map(str::to_string)
        .collect()
}

fn with(items: &[TodoItem], actions: &[TodoAction]) -> Vec<TodoItem> {
    items
        .iter()
        .zip(actions)
        .map(|(i, a)| TodoItem {
            action: a.clone(),
            ..i.clone()
        })
        .collect()
}

#[test]
fn the_list_has_the_commits_after_the_base_oldest_first() {
    if !gitcore::git_available() {
        return;
    }
    let d = tempfile::tempdir().unwrap();
    let (r, base) = repo(d.path());
    let items = r.rebase_list(&base).unwrap();
    let names: Vec<&str> = items.iter().map(|i| i.summary.as_str()).collect();
    assert_eq!(names, ["one", "two", "three", "four"]);
    assert!(items.iter().all(|i| i.action == TodoAction::Pick));
}

#[test]
fn reorder_squash_fixup_reword_and_drop() {
    if !gitcore::git_available() {
        return;
    }
    let d = tempfile::tempdir().unwrap();
    let (r, base) = repo(d.path());
    let items = r.rebase_list(&base).unwrap();
    // three first, then one + two squashed under a new message, four dropped.
    let mut plan = with(
        &items,
        &[
            TodoAction::Pick,
            TodoAction::Squash(Some("one and two".into())),
            TodoAction::Reword("third, renamed".into()),
            TodoAction::Drop,
        ],
    );
    plan.swap(0, 2);
    plan.swap(1, 2);
    // plan: three(reword), one(pick), two(squash)
    assert_eq!(r.interactive_rebase(&base, &plan).unwrap(), OpOutcome::Done);
    assert_eq!(
        subjects(d.path()),
        ["base", "third, renamed", "one and two"]
    );
    assert!(!d.path().join("four.txt").exists());
    assert!(d.path().join("two.txt").exists());
    // A fixup keeps the first message; a squash without a message keeps both.
    let items = r.rebase_list(&base).unwrap();
    let plan = with(&items, &[TodoAction::Pick, TodoAction::Fixup]);
    assert_eq!(r.interactive_rebase(&base, &plan).unwrap(), OpOutcome::Done);
    assert_eq!(subjects(d.path()), ["base", "third, renamed"]);
}

#[test]
fn a_conflict_stops_the_rebase_until_continued() {
    if !gitcore::git_available() {
        return;
    }
    let d = tempfile::tempdir().unwrap();
    let dir = d.path();
    git(dir, &["-c", "init.defaultBranch=main", "init", "-q"]);
    configure(dir);
    commit(dir, "f.txt", "base\n", "base");
    let base = git(dir, &["rev-parse", "HEAD"]).trim().to_string();
    commit(dir, "f.txt", "first\n", "first");
    commit(dir, "f.txt", "second\n", "second");
    let r = Repo::open(dir).unwrap();
    let mut plan = r.rebase_list(&base).unwrap();
    plan.reverse(); // second before first: conflicts
    assert_eq!(
        r.interactive_rebase(&base, &plan).unwrap(),
        OpOutcome::Conflicts
    );
    assert_eq!(r.operation_in_progress(), Some(Operation::Rebase));
    r.abort_operation().unwrap();
    assert_eq!(subjects(dir), ["base", "first", "second"], "abort restores");
}

#[test]
fn merges_in_the_range_are_refused() {
    if !gitcore::git_available() {
        return;
    }
    let d = tempfile::tempdir().unwrap();
    let (r, base) = repo(d.path());
    git(d.path(), &["switch", "-q", "-c", "side"]);
    commit(d.path(), "side.txt", "s\n", "side");
    git(d.path(), &["switch", "-q", "main"]);
    git(
        d.path(),
        &["merge", "-q", "--no-ff", "-m", "merge side", "side"],
    );
    assert!(matches!(
        r.rebase_list(&base),
        Err(gitcore::GitError::Unsupported(_))
    ));
}

#[test]
fn todo_lists_are_checked_and_written() {
    let item = |a: TodoAction, id: &str| TodoItem {
        action: a,
        id: id.into(),
        summary: format!("s{id}"),
    };
    assert!(gitcore::validate_todo(&[item(TodoAction::Squash(None), "a")]).is_err());
    assert!(gitcore::validate_todo(&[item(TodoAction::Drop, "a")]).is_err());
    assert!(
        gitcore::validate_todo(&[item(TodoAction::Fixup, "a"), item(TodoAction::Pick, "b")])
            .is_err()
    );
    assert!(
        gitcore::validate_todo(&[item(TodoAction::Drop, "a"), item(TodoAction::Fixup, "b")])
            .is_err(),
        "nothing to fix up"
    );
    assert!(
        gitcore::validate_todo(&[item(TodoAction::Pick, "a"), item(TodoAction::Fixup, "b")])
            .is_ok()
    );
    let msgs = std::path::Path::new("/m");
    let text = gitcore::todo_text(
        &[
            item(TodoAction::Reword("New".into()), "a"),
            item(TodoAction::Squash(None), "b"),
            item(TodoAction::Pick, "c"),
            item(TodoAction::Squash(Some("Both".into())), "d"),
            item(TodoAction::Drop, "e"),
        ],
        msgs,
    );
    assert_eq!(
        text.lines.join("\n"),
        "pick a sa\nexec git commit --amend --allow-empty -F '/m/0.txt'\nsquash b sb\npick c sc\nsquash d sd\nexec git commit --amend --allow-empty -F '/m/1.txt'"
    );
    assert_eq!(text.messages, ["New", "Both"]);
}
