#![allow(clippy::unwrap_used)]
//! Interactive rebase through `git rebase -i` (needs `git`).
mod common;

use std::path::Path;

use common::remote::{configure, git};
use gitcore::{OpOutcome, Operation, Refusal, Repo, TodoAction, TodoError, TodoItem};

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

/// base, then `first` and `second` both changing f.txt; the plan replays them reversed.
fn conflicting(dir: &Path) -> (Repo, String, Vec<TodoItem>) {
    git(dir, &["-c", "init.defaultBranch=main", "init", "-q"]);
    configure(dir);
    commit(dir, "f.txt", "base\n", "base");
    let base = git(dir, &["rev-parse", "HEAD"]).trim().to_string();
    commit(dir, "f.txt", "first\n", "first");
    commit(dir, "f.txt", "second\n", "second");
    let r = Repo::open(dir).unwrap();
    let mut plan = r.rebase_list(&base).unwrap();
    plan.reverse();
    plan[0].action = TodoAction::Reword("second, renamed".into());
    (r, base, plan)
}

#[test]
fn the_message_files_are_removed_once_the_rebase_is_done() {
    if !gitcore::git_available() {
        return;
    }
    let d = tempfile::tempdir().unwrap();
    let (r, base) = repo(d.path());
    let mut plan = r.rebase_list(&base).unwrap();
    plan[0].action = TodoAction::Reword("ONE".into());
    assert_eq!(r.interactive_rebase(&base, &plan).unwrap(), OpOutcome::Done);
    assert!(!d.path().join(".git/retrogit-rebase").exists());

    let d = tempfile::tempdir().unwrap();
    let (r, base, plan) = conflicting(d.path());
    let msgs = d.path().join(".git/retrogit-rebase");
    let mut outcome = r.interactive_rebase(&base, &plan).unwrap();
    let mut rounds = 0;
    while outcome == OpOutcome::Conflicts && rounds < 5 {
        assert!(msgs.exists(), "kept while paused");
        std::fs::write(d.path().join("f.txt"), "resolved\n").unwrap();
        git(d.path(), &["add", "-A"]);
        outcome = r.continue_operation().unwrap();
        rounds += 1;
    }
    assert_eq!(outcome, OpOutcome::Done);
    assert_eq!(r.operation_in_progress(), None);
    assert!(!msgs.exists(), "removed after the last continue");
}

#[test]
fn the_message_files_are_removed_when_the_rebase_is_aborted() {
    if !gitcore::git_available() {
        return;
    }
    let d = tempfile::tempdir().unwrap();
    let (r, base, plan) = conflicting(d.path());
    let msgs = d.path().join(".git/retrogit-rebase");
    assert_eq!(
        r.interactive_rebase(&base, &plan).unwrap(),
        OpOutcome::Conflicts
    );
    assert!(msgs.exists());
    r.abort_operation().unwrap();
    assert!(!msgs.exists());
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
        Err(gitcore::GitError::Refused(Refusal::MergesInRange))
    ));
}

#[test]
fn todo_lists_are_checked_and_written() {
    let item = |a: TodoAction, id: &str| TodoItem {
        action: a,
        id: id.into(),
        summary: format!("s{id}"),
        message: format!("s{id}"),
    };
    assert_eq!(
        gitcore::validate_todo(&[item(TodoAction::Squash(None), "a")]),
        Err(TodoError::NoKeptAbove)
    );
    assert_eq!(
        gitcore::validate_todo(&[item(TodoAction::Drop, "a")]),
        Err(TodoError::KeepOne)
    );
    assert_eq!(
        gitcore::validate_todo(&[item(TodoAction::Fixup, "a"), item(TodoAction::Pick, "b")]),
        Err(TodoError::NoKeptAbove)
    );
    assert_eq!(
        gitcore::validate_todo(&[item(TodoAction::Drop, "a"), item(TodoAction::Fixup, "b")]),
        Err(TodoError::NoKeptAbove),
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
            item(TodoAction::Fixup, "f"),
            item(TodoAction::Drop, "e"),
        ],
        msgs,
    );
    // The squash message is set once its whole group is melded; drops are written out
    // (rebase.missingCommitsCheck=error refuses missing lines).
    assert_eq!(
        text.lines.join("\n"),
        "pick a sa\nexec git commit --amend --allow-empty -F '/m/0.txt'\nsquash b sb\npick c sc\nsquash d sd\nfixup f sf\nexec git commit --amend --allow-empty -F '/m/1.txt'\ndrop e se"
    );
    assert_eq!(text.messages, ["New", "Both"]);
}

#[test]
fn the_list_carries_full_messages_and_a_reword_keeps_the_body() {
    if !gitcore::git_available() {
        return;
    }
    let d = tempfile::tempdir().unwrap();
    let (r, base) = repo(d.path());
    std::fs::write(d.path().join("five.txt"), "5\n").unwrap();
    git(d.path(), &["add", "-A"]);
    git(
        d.path(),
        &["commit", "-q", "-m", "five", "-m", "why five matters"],
    );
    let items = r.rebase_list(&base).unwrap();
    assert_eq!(items[4].summary, "five");
    assert_eq!(items[4].message, "five\n\nwhy five matters");
    let mut plan = items.clone();
    plan[4].action = TodoAction::Reword(format!("FIVE{}", &items[4].message[4..]));
    assert_eq!(r.interactive_rebase(&base, &plan).unwrap(), OpOutcome::Done);
    assert_eq!(
        git(d.path(), &["log", "-1", "--format=%B"]).trim_end(),
        "FIVE\n\nwhy five matters"
    );
}

#[test]
fn drops_work_with_missing_commits_check_error() {
    if !gitcore::git_available() {
        return;
    }
    let d = tempfile::tempdir().unwrap();
    let (r, base) = repo(d.path());
    git(d.path(), &["config", "rebase.missingCommitsCheck", "error"]);
    let items = r.rebase_list(&base).unwrap();
    let plan = with(
        &items,
        &[
            TodoAction::Pick,
            TodoAction::Drop,
            TodoAction::Pick,
            TodoAction::Pick,
        ],
    );
    assert_eq!(r.interactive_rebase(&base, &plan).unwrap(), OpOutcome::Done);
    assert_eq!(subjects(d.path()), ["base", "one", "three", "four"]);
}

#[test]
fn a_paused_rebase_is_not_restarted_and_keeps_its_messages() {
    if !gitcore::git_available() {
        return;
    }
    let d = tempfile::tempdir().unwrap();
    let (r, base) = repo(d.path());
    commit(d.path(), "one.txt", "changed\n", "touch one");
    let items = r.rebase_list(&base).unwrap();
    // "touch one" before "one": conflict on one.txt, then a reword pending.
    let mut plan = items.clone();
    plan.swap(0, 4);
    plan[1].action = TodoAction::Reword("two, renamed".into());
    let first = r.interactive_rebase(&base, &plan);
    assert!(
        r.operation_in_progress() == Some(Operation::Rebase),
        "{first:?}"
    );
    let msgs = d.path().join(".git/retrogit-rebase/0.txt");
    assert!(msgs.exists());
    assert!(r.interactive_rebase(&base, &plan).is_err());
    assert!(
        msgs.exists(),
        "the paused rebase still needs its message files"
    );
}

#[test]
fn untracked_files_stopping_a_rebase_midway_are_not_reported_as_nothing_changed() {
    if !gitcore::git_available() {
        return;
    }
    let d = tempfile::tempdir().unwrap();
    let (r, base) = repo(d.path());
    commit(d.path(), "new.txt", "tracked\n", "add new");
    std::fs::remove_file(d.path().join("new.txt")).unwrap();
    git(d.path(), &["add", "-A"]);
    git(d.path(), &["commit", "-q", "-m", "rm new"]);
    std::fs::write(d.path().join("new.txt"), "untracked\n").unwrap();
    let mut items = r.rebase_list(&base).unwrap();
    items.swap(0, 3); // rewrite everything: "add new" is replayed and needs new.txt
    let got = r.interactive_rebase(&base, &items);
    assert_eq!(
        r.operation_in_progress(),
        Some(Operation::Rebase),
        "{got:?}"
    );
    assert!(
        !matches!(got, Err(gitcore::GitError::WouldOverwrite { .. })),
        "{got:?}"
    );
}

#[test]
fn a_new_message_refused_by_a_hook_is_reported() {
    if !gitcore::git_available() {
        return;
    }
    let d = tempfile::tempdir().unwrap();
    let (r, base) = repo(d.path());
    let hook = d.path().join(".git/hooks/commit-msg");
    std::fs::write(
        &hook,
        "#!/bin/sh\ngrep -q renamed \"$1\" && exit 1\nexit 0\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let items = r.rebase_list(&base).unwrap();
    let mut plan = items.clone();
    plan[1].action = TodoAction::Reword("two, renamed".into());
    let got = r.interactive_rebase(&base, &plan);
    assert!(
        matches!(got, Err(gitcore::GitError::MessageRefused { .. })),
        "{got:?}"
    );
    assert_eq!(r.operation_in_progress(), Some(Operation::Rebase));
}
