#![allow(clippy::unwrap_used)]
//! Cherry-pick, revert and reset through `git` (needs `git`).
mod common;

use std::path::Path;

use common::remote::{configure, git};
use gitcore::{OpOutcome, Operation, Repo, ResetMode};

fn init(dir: &Path) -> Repo {
    git(dir, &["-c", "init.defaultBranch=main", "init", "-q"]);
    configure(dir);
    commit(dir, "a.txt", "a\n", "base");
    Repo::open(dir).unwrap()
}

fn commit(dir: &Path, file: &str, text: &str, msg: &str) -> String {
    std::fs::write(dir.join(file), text).unwrap();
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", msg]);
    git(dir, &["rev-parse", "HEAD"]).trim().to_string()
}

fn subjects(dir: &Path) -> String {
    git(dir, &["log", "--format=%s"])
}

fn tmp() -> Option<tempfile::TempDir> {
    gitcore::git_available().then(|| tempfile::tempdir().unwrap())
}

#[test]
fn a_cherry_pick_copies_a_commit_onto_the_current_branch() {
    let Some(d) = tmp() else { return };
    let r = init(d.path());
    git(d.path(), &["switch", "-q", "-c", "fix"]);
    let fix = commit(d.path(), "b.txt", "b\n", "the fix");
    git(d.path(), &["switch", "-q", "main"]);
    assert_eq!(r.cherry_pick(&fix).unwrap(), OpOutcome::Done);
    assert_eq!(subjects(d.path()), "the fix\nbase\n");
}

#[test]
fn a_conflicting_cherry_pick_waits_then_continues_or_skips_when_empty() {
    let Some(d) = tmp() else { return };
    let r = init(d.path());
    git(d.path(), &["switch", "-q", "-c", "other"]);
    let theirs = commit(d.path(), "a.txt", "theirs\n", "theirs");
    git(d.path(), &["switch", "-q", "main"]);
    commit(d.path(), "a.txt", "mine\n", "mine");
    assert_eq!(r.cherry_pick(&theirs).unwrap(), OpOutcome::Conflicts);
    assert_eq!(r.operation_in_progress(), Some(Operation::CherryPick));
    std::fs::write(d.path().join("a.txt"), "both\n").unwrap();
    git(d.path(), &["add", "a.txt"]);
    assert_eq!(r.continue_operation().unwrap(), OpOutcome::Done);
    assert_eq!(r.operation_in_progress(), None);
    assert_eq!(subjects(d.path()), "theirs\nmine\nbase\n");
    // A commit whose change is already here: empty, skipped.
    git(d.path(), &["switch", "-q", "other"]);
    let extra = commit(d.path(), "c.txt", "c\n", "extra");
    git(d.path(), &["switch", "-q", "main"]);
    assert_eq!(r.cherry_pick(&extra).unwrap(), OpOutcome::Done);
    assert_eq!(r.cherry_pick(&extra).unwrap(), OpOutcome::Empty);
    assert_eq!(r.operation_in_progress(), Some(Operation::CherryPick));
    r.skip_operation().unwrap();
    assert_eq!(r.operation_in_progress(), None);
}

#[test]
fn a_revert_adds_a_commit_that_undoes_it_and_merges_need_a_parent() {
    let Some(d) = tmp() else { return };
    let r = init(d.path());
    let bad = commit(d.path(), "a.txt", "broken\n", "bad change");
    assert_eq!(r.revert(&bad, None).unwrap(), OpOutcome::Done);
    assert_eq!(
        std::fs::read_to_string(d.path().join("a.txt")).unwrap(),
        "a\n"
    );
    assert!(subjects(d.path()).starts_with("Revert \"bad change\""));
    // A merge commit.
    git(d.path(), &["switch", "-q", "-c", "feature"]);
    commit(d.path(), "f.txt", "f\n", "feature");
    git(d.path(), &["switch", "-q", "main"]);
    git(
        d.path(),
        &["merge", "-q", "--no-ff", "-m", "merge feature", "feature"],
    );
    let merge = git(d.path(), &["rev-parse", "HEAD"]).trim().to_string();
    assert!(r.revert(&merge, None).is_err(), "a parent must be chosen");
    assert_eq!(r.operation_in_progress(), None, "nothing left behind");
    assert_eq!(r.revert(&merge, Some(1)).unwrap(), OpOutcome::Done);
    assert!(!d.path().join("f.txt").exists());
    assert!(r.is_merge(&merge).unwrap());
    assert!(!r.is_merge(&bad).unwrap());
}

#[test]
fn reset_soft_mixed_and_hard() {
    let Some(d) = tmp() else { return };
    let r = init(d.path());
    let base = git(d.path(), &["rev-parse", "HEAD"]).trim().to_string();
    commit(d.path(), "b.txt", "b\n", "second");
    r.reset(&base, ResetMode::Soft).unwrap();
    assert_eq!(
        git(d.path(), &["status", "--porcelain"]),
        "A  b.txt\n",
        "kept staged"
    );
    git(d.path(), &["commit", "-q", "-m", "second"]);
    r.reset(&base, ResetMode::Mixed).unwrap();
    assert_eq!(
        git(d.path(), &["status", "--porcelain"]),
        "?? b.txt\n",
        "kept unstaged"
    );
    git(d.path(), &["add", "-A"]);
    git(d.path(), &["commit", "-q", "-m", "second"]);
    std::fs::write(d.path().join("untracked.txt"), "keep\n").unwrap();
    r.reset(&base, ResetMode::Hard).unwrap();
    assert!(!d.path().join("b.txt").exists());
    assert!(
        d.path().join("untracked.txt").exists(),
        "untracked files stay"
    );
    assert_eq!(subjects(d.path()), "base\n");
}

#[test]
fn local_changes_in_the_way_are_reported_as_would_overwrite() {
    let Some(d) = tmp() else { return };
    let r = init(d.path());
    git(d.path(), &["switch", "-q", "-c", "other"]);
    let theirs = commit(d.path(), "a.txt", "theirs\n", "theirs");
    git(d.path(), &["switch", "-q", "main"]);
    std::fs::write(d.path().join("a.txt"), "dirty\n").unwrap();
    match r.cherry_pick(&theirs) {
        Err(gitcore::GitError::WouldOverwrite { files }) => assert_eq!(files, ["a.txt"]),
        other => panic!("{other:?}"),
    }
    assert_eq!(r.operation_in_progress(), None);
    // A rebase refuses any unstaged change.
    let base = git(d.path(), &["rev-parse", "HEAD"]).trim().to_string();
    commit(d.path(), "b.txt", "b\n", "b");
    std::fs::write(d.path().join("a.txt"), "dirty again\n").unwrap();
    let items = r.rebase_list(&base).unwrap();
    let got = r.interactive_rebase(&base, &items);
    assert!(
        matches!(got, Err(gitcore::GitError::WouldOverwrite { .. })),
        "{got:?}"
    );
    assert_eq!(
        std::fs::read_to_string(d.path().join("a.txt")).unwrap(),
        "dirty again\n"
    );
}
