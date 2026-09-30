#![allow(clippy::unwrap_used)]
//! Branches, stash and signing policy (needs `git`).
mod common;

use common::remote::{Env, git};
use gitcore::{CommitBackend, GitError, Selection};

#[test]
fn branches_create_switch_rename_delete() {
    let Some(env) = Env::new() else { return };
    let r = env.repo();
    let b = r.branches().unwrap();
    assert!(
        b.iter()
            .any(|b| b.name == "main" && b.is_head && b.upstream.as_deref() == Some("origin/main"))
    );
    assert!(b.iter().any(|b| b.name == "origin/main" && b.remote));
    assert!(b.iter().all(|b| b.name != "origin/HEAD"));

    assert!(r.validate_branch_name("bad name").is_err());
    assert!(r.validate_branch_name("main").is_err());
    assert!(r.validate_branch_name("feat/ok").is_ok());
    r.create_branch("feat/ok", true).unwrap();
    assert_eq!(r.current_branch().unwrap().name, "feat/ok");
    env.local_commit("x.txt", "x\n");
    r.switch_branch("main").unwrap();
    r.rename_branch("feat/ok", "feat/renamed").unwrap();
    assert_eq!(
        r.delete_branch("feat/renamed", false),
        Err(GitError::NotMerged("feat/renamed".into()))
    );
    r.delete_branch("feat/renamed", true).unwrap();
    assert!(
        r.branches()
            .unwrap()
            .iter()
            .all(|b| b.name != "feat/renamed")
    );
}

#[test]
fn switching_with_conflicting_changes_reports_files_then_stash_switch_pop_works() {
    let Some(env) = Env::new() else { return };
    let r = env.repo();
    r.create_branch("other", true).unwrap();
    env.local_commit("file0.txt", "changed on other\n");
    r.switch_branch("main").unwrap();
    std::fs::write(env.work.join("file0.txt"), "local edit\n").unwrap();
    match r.switch_branch("other") {
        Err(GitError::WouldOverwrite { files }) => assert_eq!(files, vec!["file0.txt".to_string()]),
        other => panic!("{other:?}"),
    }
    // Non-conflicting edits simply follow.
    std::fs::write(env.work.join("file0.txt"), "content 0\n".repeat(50)).unwrap();
    std::fs::write(env.work.join("file1.txt"), "follows me\n").unwrap();
    r.switch_branch("other").unwrap();
    assert_eq!(
        std::fs::read_to_string(env.work.join("file1.txt")).unwrap(),
        "follows me\n"
    );
    r.switch_branch("main").unwrap();

    std::fs::write(env.work.join("file0.txt"), "local edit\n").unwrap();
    assert!(r.stash_push("RetroGit: switch to other").unwrap());
    r.switch_branch("other").unwrap();
    assert_eq!(r.stash_pop(), Err(GitError::StashConflict));
    assert!(
        !git(&env.work, &["stash", "list"]).is_empty(),
        "the stash is kept on conflict"
    );
}

#[test]
fn stash_push_reports_when_there_is_nothing() {
    let Some(env) = Env::new() else { return };
    assert!(!env.repo().stash_push("nothing").unwrap());
    std::fs::write(env.work.join("u.txt"), "untracked\n").unwrap();
    let r = env.repo();
    assert!(r.stash_push("with untracked").unwrap());
    assert!(!env.work.join("u.txt").exists());
    r.stash_pop().unwrap();
    assert!(env.work.join("u.txt").exists());
}

#[test]
fn unsigned_fallback_is_refused_when_signing_is_required() {
    let Some(env) = Env::new() else { return };
    git(&env.work, &["config", "commit.gpgsign", "true"]);
    git(&env.work, &["config", "user.signingkey", "ABCDEF"]);
    let r = env.repo();
    let s = r.signing_config().unwrap();
    assert!(s.enabled);
    assert_eq!(s.key.as_deref(), Some("ABCDEF"));
    assert_eq!(s.format, gitcore::SigningFormat::Gpg);
    std::fs::write(env.work.join("s.txt"), "s\n").unwrap();
    r.stage("s.txt", &Selection::All, None).unwrap();
    assert_eq!(
        r.commit("x", false, CommitBackend::Git2),
        Err(GitError::SigningRequiresGit)
    );
}
