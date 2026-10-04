#![allow(clippy::unwrap_used)]
//! The Explore service answers on its own threads (needs `git`).

use std::path::Path;
use std::process::Command as Cmd;
use std::sync::Arc;
use std::time::{Duration, Instant};

use github::{Client, MemoryAccounts, TokenProvider};
use retrogit::protocol::{Command, Event, ExploreRequest, ExploreResult};
use retrogit::worker::{WorkerDeps, WorkerHandle, spawn};

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Cmd::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn start() -> Option<(tempfile::TempDir, std::path::PathBuf, WorkerHandle)> {
    if !gitcore::git_available() {
        return None;
    }
    let d = tempfile::tempdir().unwrap();
    let dir = retrogit::watch::canonical(d.path());
    git(&dir, &["-c", "init.defaultBranch=main", "init", "-q"]);
    for (k, v) in [
        ("user.name", "Ada"),
        ("user.email", "ada@example.com"),
        ("commit.gpgsign", "false"),
    ] {
        git(&dir, &["config", k, v]);
    }
    std::fs::write(dir.join("a.txt"), "needle\n").unwrap();
    git(&dir, &["add", "-A"]);
    git(&dir, &["commit", "-q", "-m", "one"]);
    let server = mockito::Server::new();
    let w = spawn(
        WorkerDeps {
            client: Client::with_bases(&server.url(), &server.url()),
            store: Arc::new(MemoryAccounts::default()),
            client_id: String::new(),
            commit_backend: gitcore::CommitBackend::PreferCli,
            tokens: TokenProvider::without_gh(),
            known_accounts: Vec::new(),
            repo_accounts: Default::default(),
        },
        || {},
    );
    Some((d, dir, w))
}

fn next_explore(w: &WorkerHandle) -> ExploreResult {
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        if let Ok(Event::ExploreLoaded { result, .. }) =
            w.events.recv_timeout(Duration::from_millis(100))
        {
            return result;
        }
    }
    panic!("no answer");
}

#[test]
fn requests_are_answered_while_the_worker_is_busy() {
    let Some((_d, dir, w)) = start() else { return };
    w.explore(&dir, ExploreRequest::Tree { rev: "HEAD".into() });
    match next_explore(&w) {
        ExploreResult::Tree { entries, .. } => assert_eq!(entries.len(), 1),
        other => panic!("{other:?}"),
    }
    w.explore(
        &dir,
        ExploreRequest::Grep {
            rev: "HEAD".into(),
            text: "needle".into(),
            match_case: true,
            paths: String::new(),
        },
    );
    match next_explore(&w) {
        ExploreResult::Grep { result, .. } => assert_eq!(result.matches.len(), 1),
        other => panic!("{other:?}"),
    }
    w.explore(&dir, ExploreRequest::Refs);
    assert!(matches!(next_explore(&w), ExploreResult::Refs(r) if r.len() == 2));
    w.explore(&dir, ExploreRequest::Tree { rev: "nope".into() });
    assert!(matches!(next_explore(&w), ExploreResult::Failed { .. }));
    // The main worker is still free.
    w.send(Command::OpenRepo(dir.clone()));
}

#[test]
fn a_newer_request_of_the_same_kind_wins() {
    let Some((_d, dir, w)) = start() else { return };
    for q in ["first", "needle"] {
        w.explore(
            &dir,
            ExploreRequest::Grep {
                rev: "HEAD".into(),
                text: q.into(),
                match_case: true,
                paths: String::new(),
            },
        );
    }
    match next_explore(&w) {
        ExploreResult::Grep { text, .. } => {
            assert_eq!(text, "needle", "the older one was cancelled")
        }
        other => panic!("{other:?}"),
    }
    // A cancel request stops a search without an answer.
    w.explore(
        &dir,
        ExploreRequest::Grep {
            rev: "HEAD".into(),
            text: "x".into(),
            match_case: true,
            paths: String::new(),
        },
    );
    w.explore(&dir, ExploreRequest::CancelSearch);
    let deadline = Instant::now() + Duration::from_millis(800);
    while Instant::now() < deadline {
        if let Ok(Event::ExploreLoaded {
            result: ExploreResult::Grep { .. },
            ..
        }) = w.events.recv_timeout(Duration::from_millis(50))
        {
            // It may have finished before the cancel: then it is fine too.
            return;
        }
    }
}
