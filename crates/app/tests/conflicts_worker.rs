#![allow(clippy::unwrap_used)]
//! Loading and resolving conflicts through the worker (needs `git`).

use std::path::Path;
use std::process::Command as Cmd;
use std::sync::Arc;
use std::time::{Duration, Instant};

use github::{Client, MemoryAccounts, TokenProvider};
use retrogit::protocol::{Command, Event};
use retrogit::worker::{WorkerDeps, WorkerHandle, spawn};

fn git(dir: &Path, args: &[&str]) {
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
}

/// `main` and `feature` both change `a.txt` and `b.txt`; `git merge feature` conflicts.
fn merge_conflict(dir: &Path) {
    git(dir, &["-c", "init.defaultBranch=main", "init", "-q"]);
    for (k, v) in [
        ("user.name", "Ada"),
        ("user.email", "ada@example.com"),
        ("commit.gpgsign", "false"),
        ("core.hooksPath", ".git/hooks"),
        ("core.autocrlf", "false"),
    ] {
        git(dir, &["config", k, v]);
    }
    let write = |f: &str, t: &str| std::fs::write(dir.join(f), t).unwrap();
    write("a.txt", "a\n");
    write("b.txt", "b\n");
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", "base"]);
    git(dir, &["switch", "-q", "-c", "feature"]);
    write("a.txt", "a-theirs\n");
    write("b.txt", "b-theirs\n");
    git(dir, &["commit", "-qam", "theirs"]);
    git(dir, &["switch", "-q", "main"]);
    write("a.txt", "a-mine\n");
    write("b.txt", "b-mine\n");
    git(dir, &["commit", "-qam", "mine"]);
    let out = Cmd::new("git")
        .current_dir(dir)
        .args(["merge", "feature"])
        .output()
        .unwrap();
    assert!(!out.status.success());
}

fn start() -> WorkerHandle {
    let server = mockito::Server::new();
    spawn(
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
    )
}

fn until(w: &WorkerHandle, done: impl Fn(&Event) -> bool) -> Vec<Event> {
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut seen = Vec::new();
    while Instant::now() < deadline {
        if let Ok(ev) = w.events.recv_timeout(Duration::from_millis(100)) {
            let stop = done(&ev);
            seen.push(ev);
            if stop {
                return seen;
            }
        }
    }
    panic!("timed out; events so far: {seen:?}");
}

#[test]
fn conflicts_are_loaded_resolved_and_the_merge_committed() {
    if !gitcore::git_available() {
        return;
    }
    let d = tempfile::tempdir().unwrap();
    let dir = retrogit::watch::canonical(d.path());
    merge_conflict(&dir);
    let w = start();
    w.send(Command::OpenRepo(dir.clone()));
    until(&w, |e| matches!(e, Event::StatusLoaded(_)));
    w.send(Command::LoadConflict("a.txt".into()));
    let evs = until(&w, |e| matches!(e, Event::ConflictLoaded(_)));
    let Some(Event::ConflictLoaded(file)) = evs.last() else {
        unreachable!()
    };
    assert_eq!(file.mine.as_deref(), Some("a-mine\n"));
    assert_eq!(gitcore::conflict_count(file.working.as_deref().unwrap()), 1);
    // A refresh re-sends the open conflict (the editor sees changes made on disk).
    w.send(Command::RefreshStatus);
    until(&w, |e| matches!(e, Event::ConflictLoaded(_)));
    w.send(Command::ResolveConflict {
        path: "a.txt".into(),
        content: "a-both\n".into(),
    });
    let evs = until(
        &w,
        |e| matches!(e, Event::ConflictResolved(p) if p == "a.txt"),
    );
    let status = evs.iter().rev().find_map(|e| match e {
        Event::StatusLoaded(f) => Some(f.clone()),
        _ => None,
    });
    let conflicted: Vec<String> = status
        .unwrap()
        .into_iter()
        .filter(|f| f.unstaged == Some(gitcore::Change::Conflicted))
        .map(|f| f.path)
        .collect();
    assert_eq!(
        conflicted,
        ["b.txt"],
        "status refreshed before ConflictResolved"
    );
    w.send(Command::ResolveConflictWith {
        path: "b.txt".into(),
        pick: gitcore::Pick::Theirs,
    });
    until(
        &w,
        |e| matches!(e, Event::ConflictResolved(p) if p == "b.txt"),
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("a.txt")).unwrap(),
        "a-both\n"
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("b.txt")).unwrap(),
        "b-theirs\n"
    );
    w.send(Command::Commit {
        message: "Merge feature".into(),
        amend: false,
    });
    until(&w, |e| matches!(e, Event::Committed(_)));
    w.send(Command::RefreshStatus);
    let evs = until(&w, |e| matches!(e, Event::OperationChanged(_)));
    assert!(
        matches!(evs.last(), Some(Event::OperationChanged(None))),
        "merge done"
    );
}

#[test]
fn a_failed_resolution_keeps_the_file_conflicted() {
    if !gitcore::git_available() {
        return;
    }
    let d = tempfile::tempdir().unwrap();
    let dir = retrogit::watch::canonical(d.path());
    merge_conflict(&dir);
    let w = start();
    w.send(Command::OpenRepo(dir.clone()));
    until(&w, |e| matches!(e, Event::StatusLoaded(_)));
    w.send(Command::ResolveConflict {
        path: "missing/dir/a.txt".into(),
        content: "x\n".into(),
    });
    let evs = until(&w, |e| matches!(e, Event::Error { .. }));
    assert!(!evs.iter().any(|e| matches!(e, Event::ConflictResolved(_))));
}
