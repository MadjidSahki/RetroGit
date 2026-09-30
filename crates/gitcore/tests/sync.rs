#![allow(clippy::unwrap_used)]
//! fetch / pull / push against a local bare remote (needs `git`).
mod common;

use std::sync::atomic::AtomicBool;

use common::remote::{Env, configure, git, no_cancel};
use gitcore::{
    CommitBackend, GitError, NetAuth, Operation, PullMode, PullOutcome, PushMode, Selection,
};

#[test]
fn checkout_of_a_remote_branch_creates_a_tracking_branch() {
    let Some(env) = Env::new() else { return };
    let other = env.root.join("pusher");
    git(
        &env.root,
        &["clone", "-q", "origin.git", other.to_str().unwrap()],
    );
    configure(&other);
    git(&other, &["switch", "-q", "-c", "topic"]);
    git(&other, &["push", "-q", "-u", "origin", "topic"]);
    let r = env.repo();
    r.fetch(&NetAuth::default(), |_| {}, &no_cancel()).unwrap();
    r.checkout_remote_branch("origin/topic").unwrap();
    let cur = r.current_branch().unwrap();
    assert_eq!(
        (cur.name.as_str(), cur.upstream.as_deref()),
        ("topic", Some("origin/topic"))
    );
}

#[test]
fn pull_fast_forward_divergence_merge_and_rebase() {
    let Some(env) = Env::new() else { return };
    let r = env.repo();
    let auth = NetAuth::default();
    assert_eq!(
        r.pull(&auth, PullMode::FastForwardOnly, |_| {}, &no_cancel())
            .unwrap(),
        PullOutcome::UpToDate
    );
    env.remote_commit("r1.txt", "r1\n");
    assert_eq!(
        r.pull(&auth, PullMode::FastForwardOnly, |_| {}, &no_cancel())
            .unwrap(),
        PullOutcome::FastForwarded
    );
    assert!(env.work.join("r1.txt").exists());

    env.remote_commit("r2.txt", "r2\n");
    env.local_commit("l1.txt", "l1\n");
    assert_eq!(
        r.pull(&auth, PullMode::FastForwardOnly, |_| {}, &no_cancel()),
        Err(GitError::Diverged {
            ahead: 1,
            behind: 1
        })
    );
    assert_eq!(
        r.pull(&auth, PullMode::Rebase, |_| {}, &no_cancel())
            .unwrap(),
        PullOutcome::Rebased
    );
    let b = r.current_branch().unwrap();
    assert_eq!((b.ahead, b.behind), (1, 0));

    env.remote_commit("r3.txt", "r3\n");
    env.local_commit("l2.txt", "l2\n");
    assert_eq!(
        r.pull(&auth, PullMode::Merge, |_| {}, &no_cancel())
            .unwrap(),
        PullOutcome::Merged
    );
    assert_eq!(r.log(0, 1).unwrap()[0].parents.len(), 2);
}

#[test]
fn pull_conflicts_leave_an_operation_that_can_be_aborted() {
    let Some(env) = Env::new() else { return };
    let r = env.repo();
    env.remote_commit("file0.txt", "theirs\n");
    env.local_commit("file0.txt", "ours\n");
    assert_eq!(
        r.pull(&NetAuth::default(), PullMode::Merge, |_| {}, &no_cancel())
            .unwrap(),
        PullOutcome::Conflicts
    );
    assert_eq!(r.operation_in_progress(), Some(Operation::Merge));
    assert!(
        r.status()
            .unwrap()
            .iter()
            .any(|f| f.unstaged == Some(gitcore::Change::Conflicted))
    );
    r.abort_operation().unwrap();
    assert_eq!(r.operation_in_progress(), None);

    assert_eq!(
        r.pull(&NetAuth::default(), PullMode::Rebase, |_| {}, &no_cancel())
            .unwrap(),
        PullOutcome::Conflicts
    );
    assert_eq!(r.operation_in_progress(), Some(Operation::Rebase));
    r.abort_operation().unwrap();
    assert_eq!(r.operation_in_progress(), None);
    assert_eq!(
        std::fs::read_to_string(env.work.join("file0.txt")).unwrap(),
        "ours\n"
    );
}

#[test]
fn rebase_can_continue_after_resolving() {
    let Some(env) = Env::new() else { return };
    let r = env.repo();
    env.remote_commit("file0.txt", "theirs\n");
    env.local_commit("file0.txt", "ours\n");
    assert_eq!(
        r.pull(&NetAuth::default(), PullMode::Rebase, |_| {}, &no_cancel())
            .unwrap(),
        PullOutcome::Conflicts
    );
    std::fs::write(env.work.join("file0.txt"), "resolved\n").unwrap();
    r.stage("file0.txt", &Selection::All, None).unwrap();
    r.continue_rebase().unwrap();
    assert_eq!(r.operation_in_progress(), None);
    assert_eq!(r.log(0, 1).unwrap()[0].summary, "local file0.txt");
}

#[test]
fn push_normal_rejected_publish_and_force_with_lease() {
    let Some(env) = Env::new() else { return };
    let r = env.repo();
    let auth = NetAuth::default();
    let mut phases = Vec::new();
    env.local_commit("p.txt", "p\n");
    r.push(&auth, PushMode::Normal, |p| phases.push(p), &no_cancel())
        .unwrap();
    assert_eq!(r.current_branch().unwrap().ahead, 0);

    env.remote_commit("q.txt", "q\n");
    env.local_commit("mine.txt", "m\n");
    assert_eq!(
        r.push(&auth, PushMode::Normal, |_| {}, &no_cancel()),
        Err(GitError::PushRejected)
    );

    r.create_branch("new-topic", true).unwrap();
    r.push(&auth, PushMode::SetUpstream, |_| {}, &no_cancel())
        .unwrap();
    assert_eq!(
        r.current_branch().unwrap().upstream.as_deref(),
        Some("origin/new-topic")
    );

    std::fs::write(env.work.join("mine.txt"), "amended\n").unwrap();
    r.stage("mine.txt", &Selection::All, None).unwrap();
    r.commit("amended", true, CommitBackend::PreferCli).unwrap();
    assert_eq!(
        r.push(&auth, PushMode::Normal, |_| {}, &no_cancel()),
        Err(GitError::PushRejected)
    );
    r.push(&auth, PushMode::ForceWithLease, |_| {}, &no_cancel())
        .unwrap();
}

#[test]
fn cancelled_fetch_returns_cancelled() {
    let Some(env) = Env::new() else { return };
    let cancel = AtomicBool::new(true);
    assert_eq!(
        env.repo().fetch(&NetAuth::default(), |_| {}, &cancel),
        Err(GitError::Cancelled)
    );
}
