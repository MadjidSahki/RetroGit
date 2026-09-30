#![allow(clippy::unwrap_used)]
mod common;

use std::path::Path;
use std::sync::atomic::AtomicBool;

use gitcore::{CommitBackend, GitError, Repo, Selection, classify_commit_failure, git_available};

/// Repo isolated from the developer's global config (identity, signing, hooks path).
fn repo_with_identity(commits: usize) -> (tempfile::TempDir, Repo) {
    let d = tempfile::tempdir().unwrap();
    let repo = common::make_repo(d.path(), commits);
    let mut cfg = repo.config().unwrap();
    cfg.set_str("user.name", "Ada").unwrap();
    cfg.set_str("user.email", "ada@example.com").unwrap();
    cfg.set_bool("commit.gpgsign", false).unwrap();
    cfg.set_str("core.hooksPath", ".git/hooks").unwrap();
    let r = Repo::open(d.path()).unwrap();
    (d, r)
}

fn stage_new_file(d: &Path, r: &Repo, name: &str) {
    std::fs::write(d.join(name), "content\n").unwrap();
    r.stage(name, &Selection::All, None).unwrap();
}

#[test]
fn git2_commit_and_amend() {
    let (d, r) = repo_with_identity(1);
    stage_new_file(d.path(), &r, "a.txt");
    let out = r
        .commit("Add a\n\nBody", false, CommitBackend::Git2)
        .unwrap();
    assert!(!out.used_cli);
    assert_eq!(out.commit.summary, "Add a");
    assert_eq!(
        r.last_commit_message().unwrap().as_deref(),
        Some("Add a\n\nBody")
    );
    assert!(r.status().unwrap().is_empty());

    let amended = r
        .commit("Add a (fixed)", true, CommitBackend::Git2)
        .unwrap();
    assert_eq!(amended.commit.summary, "Add a (fixed)");
    assert_ne!(amended.commit.short_id, out.commit.short_id);
    let repo = git2::Repository::open(d.path()).unwrap();
    let head = repo.head().unwrap().peel_to_commit().unwrap();
    assert_eq!(head.parent(0).unwrap().summary().unwrap(), Some("commit 0"));
}

#[test]
fn git2_first_commit_of_an_empty_repo() {
    let (d, r) = repo_with_identity(0);
    stage_new_file(d.path(), &r, "a.txt");
    assert_eq!(
        r.commit("first", false, CommitBackend::Git2)
            .unwrap()
            .commit
            .summary,
        "first"
    );
    assert!(matches!(
        repo_with_identity(0)
            .1
            .commit("x", true, CommitBackend::Git2),
        Err(GitError::Unsupported(_))
    ));
}

#[test]
fn cli_commit_runs_hooks_and_reports_their_output() {
    if !git_available() {
        return;
    }
    let (d, r) = repo_with_identity(1);
    stage_new_file(d.path(), &r, "a.txt");
    let ok = r
        .commit("via cli", false, CommitBackend::PreferCli)
        .unwrap();
    assert!(ok.used_cli);
    assert_eq!(ok.commit.summary, "via cli");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let hook = d.path().join(".git/hooks/pre-commit");
        std::fs::create_dir_all(hook.parent().unwrap()).unwrap();
        std::fs::write(&hook, "#!/bin/sh\necho 'lint: 3 errors' >&2\nexit 1\n").unwrap();
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
        stage_new_file(d.path(), &r, "b.txt");
        match r.commit("blocked", false, CommitBackend::PreferCli) {
            Err(GitError::CommitRejected { output }) => {
                assert!(output.contains("lint: 3 errors"), "{output}")
            }
            other => panic!("expected CommitRejected, got {other:?}"),
        }
        assert_eq!(r.summary().unwrap().last_commit.unwrap().summary, "via cli");
    }
}

#[test]
fn cli_amend_replaces_head() {
    if !git_available() {
        return;
    }
    let (d, r) = repo_with_identity(1);
    stage_new_file(d.path(), &r, "a.txt");
    r.commit("first try", false, CommitBackend::PreferCli)
        .unwrap();
    let out = r
        .commit("second try", true, CommitBackend::PreferCli)
        .unwrap();
    assert_eq!(out.commit.summary, "second try");
    let repo = git2::Repository::open(d.path()).unwrap();
    assert_eq!(
        repo.head()
            .unwrap()
            .peel_to_commit()
            .unwrap()
            .parent_count(),
        1
    );
}

#[test]
fn failure_classification() {
    assert_eq!(
        classify_commit_failure("*** Please tell me who you are.\n\nRun git config"),
        GitError::MissingIdentity
    );
    assert_eq!(
        classify_commit_failure("  error: gpg failed to sign the data\n"),
        GitError::CommitRejected {
            output: "error: gpg failed to sign the data".into()
        }
    );
    let long = "é".repeat(15_000);
    match classify_commit_failure(&long) {
        GitError::CommitRejected { output } => {
            assert!(output.len() <= 20_010 && output.ends_with("[...]"))
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn head_is_pushed_tracks_the_upstream() {
    let (src, _) = repo_with_identity(2);
    let out = tempfile::tempdir().unwrap();
    let dest = out.path().join("clone");
    let url = common::file_url(src.path());
    let req = gitcore::CloneRequest {
        url,
        dest: dest.clone(),
        credentials: None,
    };
    let cloned = gitcore::clone(&req, |_| {}, &AtomicBool::new(false)).unwrap();
    assert!(cloned.head_is_pushed().unwrap());
    let repo = git2::Repository::open(&dest).unwrap();
    let mut cfg = repo.config().unwrap();
    cfg.set_str("user.name", "Ada").unwrap();
    cfg.set_str("user.email", "ada@example.com").unwrap();
    stage_new_file(&dest, &cloned, "local.txt");
    cloned
        .commit("local only", false, CommitBackend::Git2)
        .unwrap();
    assert!(!cloned.head_is_pushed().unwrap());
    let (_d, no_upstream) = repo_with_identity(1);
    assert!(!no_upstream.head_is_pushed().unwrap());
}

#[test]
fn add_to_gitignore_creates_appends_and_dedupes() {
    let (d, r) = repo_with_identity(1);
    r.add_to_gitignore("*.log").unwrap();
    assert_eq!(
        std::fs::read_to_string(d.path().join(".gitignore")).unwrap(),
        "*.log\n"
    );
    std::fs::write(d.path().join(".gitignore"), "target/").unwrap();
    r.add_to_gitignore("*.log").unwrap();
    r.add_to_gitignore(" *.log ").unwrap();
    assert_eq!(
        std::fs::read_to_string(d.path().join(".gitignore")).unwrap(),
        "target/\n*.log\n"
    );
    assert!(matches!(
        r.add_to_gitignore("  "),
        Err(GitError::Unsupported(_))
    ));
}

#[test]
fn cli_commit_keeps_lines_starting_with_hash() {
    if !git_available() {
        return;
    }
    let (d, r) = repo_with_identity(1);
    stage_new_file(d.path(), &r, "a.txt");
    let message = "#42 fix login\n\n## Changes\n- a";
    let out = r.commit(message, false, CommitBackend::PreferCli).unwrap();
    assert_eq!(out.commit.summary, "#42 fix login");
    assert_eq!(r.last_commit_message().unwrap().as_deref(), Some(message));
}
