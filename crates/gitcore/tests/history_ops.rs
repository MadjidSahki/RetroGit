#![allow(clippy::unwrap_used)]
//! Cherry-pick, revert and reset through `git` (needs `git`).
mod common;

use std::path::Path;

use common::remote::{Env, configure, git, no_cancel};
use gitcore::{NetAuth, OpOutcome, Operation, Repo, ResetMode};

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
    assert_eq!(r.cherry_pick(&fix, None).unwrap(), OpOutcome::Done);
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
    assert_eq!(r.cherry_pick(&theirs, None).unwrap(), OpOutcome::Conflicts);
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
    assert_eq!(r.cherry_pick(&extra, None).unwrap(), OpOutcome::Done);
    assert_eq!(r.cherry_pick(&extra, None).unwrap(), OpOutcome::Empty);
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
    match r.cherry_pick(&theirs, None) {
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

#[test]
fn untracked_files_a_hard_reset_would_replace_are_listed() {
    let Some(d) = tmp() else { return };
    let r = init(d.path());
    let with_notes = commit(d.path(), "notes.txt", "committed\n", "notes");
    std::fs::remove_file(d.path().join("notes.txt")).unwrap();
    git(d.path(), &["add", "-A"]);
    git(d.path(), &["commit", "-q", "-m", "rm notes"]);
    std::fs::write(d.path().join("notes.txt"), "my precious untracked\n").unwrap();
    std::fs::write(d.path().join("other.txt"), "kept\n").unwrap();
    assert_eq!(r.reset_overwrites_untracked(&with_notes), ["notes.txt"]);
    let head = git(d.path(), &["rev-parse", "HEAD"]).trim().to_string();
    assert!(r.reset_overwrites_untracked(&head).is_empty());
}

fn rev(dir: &Path, name: &str) -> String {
    git(dir, &["rev-parse", name]).trim().to_string()
}

#[test]
fn an_octopus_merge_can_be_reverted_or_cherry_picked_against_its_third_parent() {
    let Some(d) = tmp() else { return };
    let r = init(d.path());
    for side in ["x", "y"] {
        git(d.path(), &["switch", "-q", "-c", side, "main"]);
        commit(d.path(), &format!("{side}.txt"), "side\n", side);
    }
    git(d.path(), &["switch", "-q", "main"]);
    git(
        d.path(),
        &["merge", "-q", "--no-ff", "-m", "octopus", "x", "y"],
    );
    let octopus = rev(d.path(), "HEAD");
    assert_eq!(
        git(d.path(), &["rev-list", "--parents", "-n1", "HEAD"])
            .split(' ')
            .count(),
        4
    );
    // Keep parent 3 (y): what x brought in goes away.
    assert_eq!(r.revert(&octopus, Some(3)).unwrap(), OpOutcome::Done);
    assert!(!d.path().join("x.txt").exists());
    assert!(d.path().join("y.txt").exists());
    // Copy what the octopus brought in compared with y (that is, x) onto y.
    git(d.path(), &["switch", "-q", "y"]);
    assert_eq!(r.cherry_pick(&octopus, Some(3)).unwrap(), OpOutcome::Done);
    assert!(d.path().join("x.txt").exists());
    assert_eq!(r.operation_in_progress(), None);
}

#[test]
fn a_merge_is_cherry_picked_against_the_chosen_parent() {
    let Some(d) = tmp() else { return };
    let r = init(d.path());
    git(d.path(), &["switch", "-q", "-c", "other"]);
    git(d.path(), &["switch", "-q", "-c", "feature", "main"]);
    commit(d.path(), "f.txt", "f\n", "feature");
    git(d.path(), &["switch", "-q", "main"]);
    commit(d.path(), "m.txt", "m\n", "on main");
    git(
        d.path(),
        &["merge", "-q", "--no-ff", "-m", "merge feature", "feature"],
    );
    let merge = rev(d.path(), "HEAD");
    git(d.path(), &["switch", "-q", "other"]);
    assert_eq!(r.cherry_pick(&merge, Some(1)).unwrap(), OpOutcome::Done);
    assert!(
        d.path().join("f.txt").exists(),
        "the merged branch's change"
    );
    assert!(
        !d.path().join("m.txt").exists(),
        "not parent 1's own commits"
    );
    assert_eq!(subjects(d.path()), "merge feature\nbase\n");
}

#[test]
fn a_reset_warns_only_when_pushed_commits_leave_the_branch() {
    let Some(env) = Env::new() else { return };
    let r = env.repo();
    let work = env.work.as_path();
    let fetch = || r.fetch(&NetAuth::default(), |_| {}, &no_cancel()).unwrap();
    // Behind by 2: staying or moving forward drops nothing.
    env.remote_commit("r1.txt", "r1\n");
    env.remote_commit("r2.txt", "r2\n");
    fetch();
    let head = rev(work, "HEAD");
    assert!(!r.reset_drops_pushed(&head), "behind, reset to HEAD");
    assert!(
        !r.reset_drops_pushed(&rev(work, "origin/main")),
        "behind, reset to origin/main"
    );
    assert!(
        r.reset_drops_pushed(&rev(work, "HEAD~1")),
        "behind, below the pushed HEAD"
    );
    // Ahead by 1: dropping the local commit is fine, going below origin/main is not.
    git(work, &["merge", "-q", "--ff-only", "origin/main"]);
    env.local_commit("l1.txt", "l1\n");
    assert!(
        !r.reset_drops_pushed(&rev(work, "HEAD~1")),
        "ahead, to origin/main"
    );
    assert!(
        r.reset_drops_pushed(&rev(work, "HEAD~2")),
        "ahead, below origin/main"
    );
    // Diverged: below the merge base drops pushed commits.
    env.remote_commit("r3.txt", "r3\n");
    fetch();
    assert!(
        !r.reset_drops_pushed(&rev(work, "HEAD")),
        "diverged, reset to HEAD"
    );
    assert!(
        !r.reset_drops_pushed(&rev(work, "origin/main")),
        "diverged, to origin/main"
    );
    assert!(
        r.reset_drops_pushed(&rev(work, "HEAD~2")),
        "diverged, below the base"
    );
}
