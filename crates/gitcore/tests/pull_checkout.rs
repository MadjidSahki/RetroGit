#![allow(clippy::unwrap_used)]
//! Pull request heads (`refs/pull/N/head`) fetched into `pr/N` (needs `git`).
mod common;

use common::remote::{Env, configure, git, no_cancel};
use gitcore::{GitError, NetAuth};

/// Another clone pushes a commit to `refs/pull/<n>/head` only (like a fork's PR), and
/// returns its commit id.
fn push_pull_head(env: &Env, n: u64, file: &str, force_from_main: bool) -> String {
    let other = env.root.join(format!("fork-{n}-{file}"));
    git(
        &env.root,
        &[
            "-c",
            "core.autocrlf=false",
            "clone",
            "-q",
            "origin.git",
            other.to_str().unwrap(),
        ],
    );
    configure(&other);
    let refspec = format!("refs/pull/{n}/head");
    if !force_from_main && git(&other, &["ls-remote", "origin", &refspec]).contains("refs/pull") {
        git(&other, &["fetch", "-q", "origin", &format!("{refspec}:pr")]);
        git(&other, &["switch", "-q", "pr"]);
    }
    std::fs::write(other.join(file), "fork\n").unwrap();
    git(&other, &["add", file]);
    git(&other, &["commit", "-q", "-m", &format!("fork {file}")]);
    git(
        &other,
        &["push", "-q", "-f", "origin", &format!("HEAD:{refspec}")],
    );
    git(&other, &["rev-parse", "HEAD"]).trim().to_string()
}

fn rev(env: &Env, name: &str) -> String {
    git(&env.work, &["rev-parse", name]).trim().to_string()
}

#[test]
fn fetch_pull_creates_a_local_branch_from_refs_pull() {
    let Some(env) = Env::new() else { return };
    let head = push_pull_head(&env, 1, "a.txt", false);
    let r = env.repo();
    let (branch, behind) = r
        .fetch_pull(1, &NetAuth::default(), |_| {}, &no_cancel())
        .unwrap();
    assert_eq!(branch, "pr/1");
    assert!(!behind);
    assert_eq!(rev(&env, "pr/1"), head);
    // Not switched: the user decides (local changes may need a stash).
    assert_eq!(r.current_branch().unwrap().name, "main");
}

#[test]
fn fetch_pull_moves_an_existing_branch_forward_even_when_checked_out() {
    let Some(env) = Env::new() else { return };
    push_pull_head(&env, 2, "a.txt", false);
    let r = env.repo();
    r.fetch_pull(2, &NetAuth::default(), |_| {}, &no_cancel())
        .unwrap();
    r.switch_branch("pr/2").unwrap();
    let newer = push_pull_head(&env, 2, "b.txt", false);
    let (_, behind) = r
        .fetch_pull(2, &NetAuth::default(), |_| {}, &no_cancel())
        .unwrap();
    assert!(!behind, "moved forward");
    assert_eq!(rev(&env, "pr/2"), newer);
    assert!(env.work.join("b.txt").exists(), "working tree follows");
}

#[test]
fn a_checked_out_pull_request_branch_blocked_by_local_changes_is_a_distinct_error() {
    let Some(env) = Env::new() else { return };
    push_pull_head(&env, 4, "a.txt", false);
    let r = env.repo();
    r.fetch_pull(4, &NetAuth::default(), |_| {}, &no_cancel())
        .unwrap();
    r.switch_branch("pr/4").unwrap();
    let before = rev(&env, "pr/4");
    // A local file the new head also brings.
    std::fs::write(env.work.join("b.txt"), "local\n").unwrap();
    let newer = push_pull_head(&env, 4, "b.txt", false);
    let err = r
        .fetch_pull(4, &NetAuth::default(), |_| {}, &no_cancel())
        .unwrap_err();
    assert!(
        matches!(&err, GitError::WouldOverwrite { files } if files == &["b.txt".to_string()]),
        "{err:?}"
    );
    assert_eq!(rev(&env, "pr/4"), before, "not moved");
    assert_eq!(rev(&env, "refs/retrogit/pull/4"), newer, "but fetched");
    assert_eq!(
        std::fs::read_to_string(env.work.join("b.txt")).unwrap(),
        "local\n"
    );
}

#[test]
fn a_force_pushed_pull_request_is_followed_unless_there_is_local_work() {
    let Some(env) = Env::new() else { return };
    push_pull_head(&env, 3, "a.txt", false);
    let r = env.repo();
    r.fetch_pull(3, &NetAuth::default(), |_| {}, &no_cancel())
        .unwrap();
    // Force-push: history rewritten, nothing local on pr/3 -> follow.
    let rewritten = push_pull_head(&env, 3, "c.txt", true);
    let (_, behind) = r
        .fetch_pull(3, &NetAuth::default(), |_| {}, &no_cancel())
        .unwrap();
    assert!(!behind, "followed");
    assert_eq!(rev(&env, "pr/3"), rewritten);
    // Local commit on pr/3, then another force-push: the local work is kept.
    r.switch_branch("pr/3").unwrap();
    env.local_commit("mine.txt", "mine\n");
    let mine = rev(&env, "pr/3");
    // Not moved yet: local commits on top of the pull request are not "behind".
    let (_, behind) = r
        .fetch_pull(3, &NetAuth::default(), |_| {}, &no_cancel())
        .unwrap();
    assert!(!behind, "nothing new on the pull request");
    push_pull_head(&env, 3, "d.txt", true);
    let (_, behind) = r
        .fetch_pull(3, &NetAuth::default(), |_| {}, &no_cancel())
        .unwrap();
    assert!(behind, "local commits kept pr/3 from following");
    assert_eq!(rev(&env, "pr/3"), mine);
}

#[test]
fn a_missing_pull_request_is_an_error() {
    let Some(env) = Env::new() else { return };
    assert!(
        env.repo()
            .fetch_pull(99, &NetAuth::default(), |_| {}, &no_cancel())
            .is_err()
    );
}

#[test]
fn github_slug_reads_origin() {
    let Some(env) = Env::new() else { return };
    assert_eq!(env.repo().github_slug(), None, "local remote");
    git(
        &env.work,
        &["remote", "set-url", "origin", "git@github.com:o/r.git"],
    );
    assert_eq!(env.repo().github_slug(), Some(("o".into(), "r".into())));
}

/// origin/main one commit ahead of the local main (fetched, not merged).
fn remote_ahead(env: &Env, file: &str) -> String {
    env.remote_commit(file, "remote\n");
    git(&env.work, &["fetch", "-q", "origin"]);
    rev(env, "origin/main")
}

#[test]
fn fast_forward_moves_a_branch_that_is_behind() {
    let Some(env) = Env::new() else { return };
    let target = remote_ahead(&env, "new.txt");
    assert!(env.repo().fast_forward("origin/main").unwrap());
    assert_eq!(rev(&env, "HEAD"), target);
    assert!(env.work.join("new.txt").exists(), "working tree follows");
}

#[test]
fn fast_forward_onto_the_same_commit_does_nothing() {
    let Some(env) = Env::new() else { return };
    let head = rev(&env, "HEAD");
    assert!(!env.repo().fast_forward("origin/main").unwrap());
    assert_eq!(rev(&env, "HEAD"), head);
}

#[test]
fn fast_forward_leaves_a_diverged_branch_alone() {
    let Some(env) = Env::new() else { return };
    remote_ahead(&env, "theirs.txt");
    env.local_commit("mine.txt", "mine\n");
    let head = rev(&env, "HEAD");
    assert!(!env.repo().fast_forward("origin/main").unwrap());
    assert_eq!(rev(&env, "HEAD"), head);
    assert!(!env.work.join("theirs.txt").exists());
}

#[test]
fn fast_forward_onto_an_unknown_ref_does_nothing() {
    let Some(env) = Env::new() else { return };
    let head = rev(&env, "HEAD");
    assert!(!env.repo().fast_forward("origin/no-such-branch").unwrap());
    assert_eq!(rev(&env, "HEAD"), head);
}

#[test]
fn fast_forward_over_a_conflicting_local_file_is_an_error() {
    let Some(env) = Env::new() else { return };
    remote_ahead(&env, "new.txt");
    // Not committed here, and the incoming commit creates it: git refuses to overwrite.
    std::fs::write(env.work.join("new.txt"), "local work\n").unwrap();
    let head = rev(&env, "HEAD");
    assert!(env.repo().fast_forward("origin/main").is_err());
    assert_eq!(rev(&env, "HEAD"), head);
    assert_eq!(
        std::fs::read_to_string(env.work.join("new.txt")).unwrap(),
        "local work\n"
    );
}
