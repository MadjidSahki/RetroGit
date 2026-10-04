#![allow(clippy::unwrap_used)]
//! History operations, stashes and tags through the worker (needs `git`).

use std::path::Path;
use std::process::Command as Cmd;
use std::sync::Arc;
use std::time::{Duration, Instant};

use github::{Client, MemoryAccounts, TokenProvider};
use retrogit::protocol::{Command, Event};
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

fn commit(dir: &Path, file: &str, text: &str, msg: &str) -> String {
    std::fs::write(dir.join(file), text).unwrap();
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", msg]);
    git(dir, &["rev-parse", "HEAD"]).trim().to_string()
}

fn repo() -> Option<(tempfile::TempDir, std::path::PathBuf)> {
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
        ("core.hooksPath", ".git/hooks"),
        ("core.autocrlf", "false"),
    ] {
        git(&dir, &["config", k, v]);
    }
    commit(&dir, "a.txt", "a\n", "base");
    Some((d, dir))
}

fn start(dir: &Path) -> WorkerHandle {
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
    w.send(Command::OpenRepo(dir.to_path_buf()));
    until(&w, |e| matches!(e, Event::StatusLoaded(_)));
    w
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
fn cherry_pick_revert_and_reset_reload_the_history() {
    let Some((_d, dir)) = repo() else { return };
    git(&dir, &["switch", "-q", "-c", "fix"]);
    let fix = commit(&dir, "b.txt", "b\n", "the fix");
    git(&dir, &["switch", "-q", "main"]);
    let w = start(&dir);
    w.send(Command::CherryPick(fix));
    let evs = until(&w, |e| matches!(e, Event::OpFinished { .. }));
    assert!(
        evs.iter().any(|e| matches!(e, Event::LogLoaded { .. })),
        "history reloaded"
    );
    let head = git(&dir, &["rev-parse", "HEAD"]).trim().to_string();
    w.send(Command::Revert {
        id: head,
        mainline: None,
    });
    until(&w, |e| matches!(e, Event::OpFinished { .. }));
    assert!(!dir.join("b.txt").exists());
    let base = git(&dir, &["rev-parse", "HEAD~2"]).trim().to_string();
    w.send(Command::Reset {
        id: base,
        mode: gitcore::ResetMode::Hard,
    });
    until(&w, |e| matches!(e, Event::OpFinished { .. }));
    assert_eq!(git(&dir, &["log", "--format=%s"]), "base\n");
}

#[test]
fn a_cherry_pick_that_conflicts_says_so() {
    let Some((_d, dir)) = repo() else { return };
    git(&dir, &["switch", "-q", "-c", "other"]);
    let theirs = commit(&dir, "a.txt", "theirs\n", "theirs");
    git(&dir, &["switch", "-q", "main"]);
    commit(&dir, "a.txt", "mine\n", "mine");
    let w = start(&dir);
    w.send(Command::CherryPick(theirs));
    let evs = until(&w, |e| matches!(e, Event::OpFinished { .. }));
    assert!(matches!(
        evs.last(),
        Some(Event::OpFinished {
            outcome: gitcore::OpOutcome::Conflicts,
            ..
        })
    ));
    assert!(evs.iter().any(|e| matches!(
        e,
        Event::OperationChanged(Some(gitcore::Operation::CherryPick))
    )));
}

#[test]
fn the_rebase_list_starts_after_the_chosen_commit_and_the_rebase_runs() {
    let Some((_d, dir)) = repo() else { return };
    let base = git(&dir, &["rev-parse", "HEAD"]).trim().to_string();
    commit(&dir, "b.txt", "b\n", "one");
    commit(&dir, "c.txt", "c\n", "two");
    let w = start(&dir);
    w.send(Command::LoadRebaseList(Some(base.clone())));
    let evs = until(&w, |e| matches!(e, Event::RebaseListLoaded { .. }));
    let Some(Event::RebaseListLoaded {
        base: b,
        items,
        pushed,
    }) = evs.last()
    else {
        unreachable!()
    };
    assert_eq!(b, &base);
    assert_eq!(items.len(), 2);
    assert_eq!(*pushed, 0);
    let mut plan = items.clone();
    plan[1].action = gitcore::TodoAction::Squash(Some("one and two".into()));
    w.send(Command::InteractiveRebase { base, items: plan });
    until(&w, |e| matches!(e, Event::OpFinished { .. }));
    assert_eq!(git(&dir, &["log", "--format=%s"]), "one and two\nbase\n");
    // Without an upstream, no default start.
    w.send(Command::LoadRebaseList(None));
    let evs = until(&w, |e| matches!(e, Event::Error { .. }));
    assert!(
        matches!(evs.last(), Some(Event::Error { error, .. }) if error.message == retrogit::strings::ERR_REBASE_NO_BASE)
    );
}

#[test]
fn stashes_and_tags_are_listed_after_each_change() {
    let Some((_d, dir)) = repo() else { return };
    let w = start(&dir);
    std::fs::write(dir.join("a.txt"), "work\n").unwrap();
    w.send(Command::StashSave {
        message: "wip".into(),
        untracked: false,
    });
    let evs = until(&w, |e| matches!(e, Event::StashesLoaded(_)));
    assert!(
        matches!(evs.last(), Some(Event::StashesLoaded(l)) if l.len() == 1 && l[0].message == "wip")
    );
    w.send(Command::LoadStashFiles(0));
    let evs = until(&w, |e| matches!(e, Event::StashFilesLoaded { .. }));
    assert!(
        matches!(evs.last(), Some(Event::StashFilesLoaded { index: 0, files }) if files[0].path == "a.txt")
    );
    let id = git(&dir, &["rev-parse", "stash@{0}"]).trim().to_string();
    w.send(Command::StashPop { index: 0, id });
    let evs = until(&w, |e| matches!(e, Event::StashesLoaded(_)));
    assert!(matches!(evs.last(), Some(Event::StashesLoaded(l)) if l.is_empty()));
    assert_eq!(
        std::fs::read_to_string(dir.join("a.txt")).unwrap(),
        "work\n"
    );
    let head = git(&dir, &["rev-parse", "HEAD"]).trim().to_string();
    w.send(Command::CreateTag {
        name: "v1".into(),
        id: head,
        message: Some("one".into()),
    });
    let evs = until(&w, |e| matches!(e, Event::TagsLoaded(_)));
    assert!(matches!(evs.last(), Some(Event::TagsLoaded(t)) if t.len() == 1 && t[0].annotated));
    w.send(Command::DeleteTag {
        name: "v1".into(),
        remote: false,
    });
    let evs = until(&w, |e| matches!(e, Event::TagsLoaded(_)));
    assert!(matches!(evs.last(), Some(Event::TagsLoaded(t)) if t.is_empty()));
}

#[test]
fn an_operation_blocked_by_local_changes_offers_stash_and_retry() {
    let Some((_d, dir)) = repo() else { return };
    git(&dir, &["switch", "-q", "-c", "other"]);
    let theirs = commit(&dir, "a.txt", "theirs\n", "theirs");
    git(&dir, &["switch", "-q", "main"]);
    std::fs::write(dir.join("a.txt"), "dirty\n").unwrap();
    let w = start(&dir);
    let pick = Command::CherryPick(theirs);
    w.send(pick.clone());
    let evs = until(&w, |e| matches!(e, Event::OpBlocked { .. }));
    match evs.last() {
        Some(Event::OpBlocked { retry, files }) => {
            assert_eq!(**retry, pick);
            assert_eq!(files, &["a.txt"]);
        }
        other => panic!("{other:?}"),
    }
    w.send(Command::StashAndRetry {
        retry: Box::new(pick),
        files: vec!["a.txt".into()],
    });
    until(&w, |e| matches!(e, Event::OpFinished { .. }));
    assert_eq!(git(&dir, &["log", "-1", "--format=%s"]), "theirs\n");
    assert_eq!(
        git(&dir, &["stash", "list"]).lines().count(),
        1,
        "changes kept in a stash"
    );
}

/// Events until the next error, then whatever follows within half a second.
fn until_error(w: &WorkerHandle) -> Vec<Event> {
    let mut evs = until(w, |e| matches!(e, Event::Error { .. }));
    while let Ok(ev) = w.events.recv_timeout(Duration::from_millis(500)) {
        evs.push(ev);
    }
    evs
}

#[test]
fn stash_and_retry_with_nothing_stashable_says_so_and_does_not_ask_again() {
    let Some((_d, dir)) = repo() else { return };
    git(&dir, &["switch", "-q", "-c", "other"]);
    let theirs = commit(&dir, "a.txt", "theirs\n", "theirs");
    git(&dir, &["switch", "-q", "main"]);
    // A skip-worktree file blocks the cherry-pick but `git stash` does not see it.
    git(&dir, &["update-index", "--skip-worktree", "a.txt"]);
    std::fs::write(dir.join("a.txt"), "dirty\n").unwrap();
    let w = start(&dir);
    let pick = Command::CherryPick(theirs);
    w.send(pick.clone());
    let evs = until(&w, |e| matches!(e, Event::OpBlocked { .. }));
    let Some(Event::OpBlocked { files, .. }) = evs.last() else {
        panic!("{evs:?}")
    };
    assert_eq!(files, &["a.txt"]);
    w.send(Command::StashAndRetry {
        retry: Box::new(pick),
        files: files.clone(),
    });
    let evs = until_error(&w);
    assert!(
        !evs.iter().any(|e| matches!(e, Event::OpBlocked { .. })),
        "{evs:?}"
    );
    let Some(Event::Error { error, .. }) = evs.iter().find(|e| matches!(e, Event::Error { .. }))
    else {
        unreachable!()
    };
    assert!(
        error
            .message
            .starts_with(retrogit::strings::ERR_NOTHING_TO_STASH),
        "{error:?}"
    );
    assert_eq!(
        error.message,
        retrogit::strings::nothing_to_stash(&["a.txt".into()])
    );
    assert_eq!(git(&dir, &["log", "-1", "--format=%s"]), "base\n");
    assert_eq!(
        std::fs::read_to_string(dir.join("a.txt")).unwrap(),
        "dirty\n"
    );
}

#[test]
fn stash_and_retry_blocked_again_gives_an_error_and_keeps_the_stash() {
    let Some((_d, dir)) = repo() else { return };
    commit(&dir, "b.txt", "b\n", "add b");
    git(&dir, &["switch", "-q", "-c", "other"]);
    std::fs::write(dir.join("a.txt"), "theirs\n").unwrap();
    let theirs = commit(&dir, "b.txt", "theirs\n", "theirs");
    git(&dir, &["switch", "-q", "main"]);
    // a.txt can be stashed, b.txt (skip-worktree) cannot.
    std::fs::write(dir.join("a.txt"), "dirty\n").unwrap();
    git(&dir, &["update-index", "--skip-worktree", "b.txt"]);
    std::fs::write(dir.join("b.txt"), "dirty\n").unwrap();
    let w = start(&dir);
    let pick = Command::CherryPick(theirs);
    w.send(pick.clone());
    let evs = until(&w, |e| matches!(e, Event::OpBlocked { .. }));
    let Some(Event::OpBlocked { files, .. }) = evs.last() else {
        panic!("{evs:?}")
    };
    w.send(Command::StashAndRetry {
        retry: Box::new(pick),
        files: files.clone(),
    });
    let evs = until_error(&w);
    assert!(
        !evs.iter().any(|e| matches!(e, Event::OpBlocked { .. })),
        "{evs:?}"
    );
    assert!(
        evs.iter().any(|e| matches!(e, Event::Error { error, .. }
            if error.message == retrogit::strings::ERR_WOULD_OVERWRITE)),
        "{evs:?}"
    );
    assert_eq!(
        git(&dir, &["stash", "list"]).lines().count(),
        1,
        "stash kept"
    );
    assert_eq!(git(&dir, &["log", "-1", "--format=%s"]), "add b\n");
}

#[test]
fn a_stash_popped_with_conflicts_is_reported_as_kept() {
    let Some((_d, dir)) = repo() else { return };
    std::fs::write(dir.join("a.txt"), "stashed\n").unwrap();
    git(&dir, &["stash", "push", "-q", "-m", "wip"]);
    commit(&dir, "a.txt", "committed\n", "change a");
    let id = git(&dir, &["rev-parse", "stash@{0}"]).trim().to_string();
    let w = start(&dir);
    w.send(Command::StashPop { index: 0, id });
    let evs = until(&w, |e| matches!(e, Event::OpFinished { .. }));
    assert!(
        matches!(
            evs.last(),
            Some(Event::OpFinished {
                outcome: gitcore::OpOutcome::Conflicts,
                stash_kept: true,
                ..
            })
        ),
        "{evs:?}"
    );
    assert_eq!(git(&dir, &["stash", "list"]).lines().count(), 1);
}

#[test]
fn stash_actions_refuse_a_stash_list_that_changed() {
    let Some((_d, dir)) = repo() else { return };
    std::fs::write(dir.join("a.txt"), "first\n").unwrap();
    git(&dir, &["stash", "push", "-q", "-m", "first"]);
    let first = git(&dir, &["rev-parse", "stash@{0}"]).trim().to_string();
    let w = start(&dir);
    // Another stash arrives (terminal, pull autostash): `first` is now stash@{1}.
    std::fs::write(dir.join("a.txt"), "second\n").unwrap();
    git(&dir, &["stash", "push", "-q", "-m", "second"]);
    w.send(Command::StashDrop {
        index: 0,
        id: first.clone(),
    });
    let evs = until(&w, |e| matches!(e, Event::Error { .. }));
    assert!(
        matches!(evs.last(), Some(Event::Error { error, .. }) if error.message == retrogit::strings::ERR_STASH_LIST_CHANGED)
    );
    assert_eq!(
        git(&dir, &["stash", "list"]).lines().count(),
        2,
        "nothing dropped"
    );
    until(&w, |e| matches!(e, Event::StashesLoaded(l) if l.len() == 2));
    // History operations reload the stash list too.
    let head = git(&dir, &["rev-parse", "HEAD"]).trim().to_string();
    w.send(Command::Reset {
        id: head,
        mode: gitcore::ResetMode::Mixed,
    });
    until(&w, |e| matches!(e, Event::StashesLoaded(_)));
}

#[test]
fn a_rebase_from_a_commit_of_another_branch_is_refused() {
    let Some((_d, dir)) = repo() else { return };
    git(&dir, &["switch", "-q", "-c", "other"]);
    let elsewhere = commit(&dir, "b.txt", "b\n", "elsewhere");
    git(&dir, &["switch", "-q", "main"]);
    commit(&dir, "c.txt", "c\n", "mine");
    let w = start(&dir);
    w.send(Command::LoadRebaseList(Some(elsewhere)));
    let evs = until(&w, |e| {
        matches!(e, Event::Error { .. } | Event::RebaseListLoaded { .. })
    });
    assert!(
        matches!(evs.last(), Some(Event::Error { error, .. }) if error.message == retrogit::strings::ERR_REBASE_NOT_ANCESTOR),
        "{:?}",
        evs.last()
    );
}

#[test]
fn pushing_a_tag_reports_in_the_tags_window() {
    let Some((d, dir)) = repo() else { return };
    let bare = d.path().join("origin.git");
    git(d.path(), &["init", "-q", "--bare", bare.to_str().unwrap()]);
    git(&dir, &["remote", "add", "origin", bare.to_str().unwrap()]);
    git(&dir, &["tag", "v1"]);
    let w = start(&dir);
    w.send(Command::PushTags(Some("v1".into())));
    let evs = until(&w, |e| matches!(e, Event::TagsStatus(_)));
    assert!(
        matches!(evs.last(), Some(Event::TagsStatus(t)) if t.contains("v1")),
        "{:?}",
        evs.last()
    );
    assert!(git(&bare, &["tag"]).contains("v1"));
}
