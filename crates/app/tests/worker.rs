#![allow(clippy::unwrap_used)]

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use github::{Client, MemoryStore, TokenStore};
use mockito::Matcher;
use retrogit::protocol::{Command, Event, Op};
use retrogit::worker::{WorkerDeps, WorkerHandle, spawn};

fn start(server: &mockito::Server, store: Arc<MemoryStore>, client_id: &str) -> WorkerHandle {
    let deps = WorkerDeps {
        client: Client::with_bases(&server.url(), &server.url()),
        store,
        client_id: client_id.into(),
        commit_backend: gitcore::CommitBackend::Git2,
    };
    spawn(deps, || {})
}

/// Collect events until `done` matches one (or panic after 10 s).
fn until(w: &WorkerHandle, done: impl Fn(&Event) -> bool) -> Vec<Event> {
    let deadline = Instant::now() + Duration::from_secs(10);
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

fn mock_user(server: &mut mockito::Server, token: &str) -> mockito::Mock {
    server
        .mock("GET", "/user")
        .match_header("authorization", format!("Bearer {token}").as_str())
        .with_body(r#"{"login":"ada","name":null}"#)
        .create()
}

#[test]
fn validate_without_token_signs_out() {
    let server = mockito::Server::new();
    let w = start(&server, Arc::new(MemoryStore::default()), "");
    w.send(Command::ValidateToken);
    until(&w, |e| matches!(e, Event::SignedOut));
}

#[test]
fn validate_with_good_token_signs_in() {
    let mut server = mockito::Server::new();
    let _m = mock_user(&mut server, "gho_good");
    let w = start(&server, Arc::new(MemoryStore::with_token("gho_good")), "");
    w.send(Command::ValidateToken);
    until(&w, |e| matches!(e, Event::SignedIn(u) if u.login == "ada"));
}

#[test]
fn validate_with_revoked_token_clears_it() {
    let mut server = mockito::Server::new();
    server.mock("GET", "/user").with_status(401).create();
    let store = Arc::new(MemoryStore::with_token("gho_old"));
    let w = start(&server, store.clone(), "");
    w.send(Command::ValidateToken);
    until(&w, |e| matches!(e, Event::SignedOut));
    assert_eq!(store.load(), Ok(None));
}

#[test]
fn pat_is_validated_then_stored() {
    let mut server = mockito::Server::new();
    let _m = mock_user(&mut server, "ghp_pat");
    let store = Arc::new(MemoryStore::default());
    let w = start(&server, store.clone(), "");
    w.send(Command::SavePat("  ghp_pat \n".into()));
    until(&w, |e| matches!(e, Event::SignedIn(_)));
    assert_eq!(store.load(), Ok(Some("ghp_pat".into())));
}

#[test]
fn rejected_pat_is_not_stored() {
    let mut server = mockito::Server::new();
    server.mock("GET", "/user").with_status(401).create();
    let store = Arc::new(MemoryStore::default());
    let w = start(&server, store.clone(), "");
    w.send(Command::SavePat("ghp_bad".into()));
    until(&w, |e| {
        matches!(
            e,
            Event::Error {
                during: Op::Auth,
                ..
            }
        )
    });
    assert_eq!(store.load(), Ok(None));
}

#[test]
fn device_flow_without_client_id_explains_pat_fallback() {
    let server = mockito::Server::new();
    let w = start(&server, Arc::new(MemoryStore::default()), "");
    w.send(Command::StartDeviceFlow);
    until(
        &w,
        |e| matches!(e, Event::Error { during: Op::Auth, error } if error.message.contains("Advanced")),
    );
}

fn mock_device_code(server: &mut mockito::Server) -> mockito::Mock {
    server
        .mock("POST", "/login/device/code")
        .with_body(r#"{"device_code":"dc1","user_code":"ABCD-1234","verification_uri":"https://github.com/login/device","expires_in":900,"interval":1}"#)
        .create()
}

#[test]
fn device_flow_happy_path_stores_token() {
    let mut server = mockito::Server::new();
    let _c = mock_device_code(&mut server);
    let _t = server
        .mock("POST", "/login/oauth/access_token")
        .match_body(Matcher::UrlEncoded("device_code".into(), "dc1".into()))
        .with_body(r#"{"access_token":"gho_new","token_type":"bearer","scope":"repo,read:org"}"#)
        .create();
    let _u = mock_user(&mut server, "gho_new");
    let store = Arc::new(MemoryStore::default());
    let w = start(&server, store.clone(), "Iv1.test");
    w.send(Command::StartDeviceFlow);
    let evs = until(&w, |e| matches!(e, Event::SignedIn(_)));
    assert!(matches!(&evs[0], Event::DeviceCode { user_code, .. } if user_code == "ABCD-1234"));
    assert_eq!(store.load(), Ok(Some("gho_new".into())));
}

#[test]
fn device_flow_can_be_cancelled_while_waiting() {
    let mut server = mockito::Server::new();
    let _c = mock_device_code(&mut server);
    let _t = server
        .mock("POST", "/login/oauth/access_token")
        .with_body(r#"{"error":"authorization_pending"}"#)
        .create();
    let w = start(&server, Arc::new(MemoryStore::default()), "Iv1.test");
    w.send(Command::StartDeviceFlow);
    until(&w, |e| matches!(e, Event::DeviceCode { .. }));
    w.cancel_device_flow();
    until(&w, |e| matches!(e, Event::DeviceFlowCancelled));
}

#[test]
fn network_failure_while_polling_reports_auth_error() {
    let mut server = mockito::Server::new();
    let _c = mock_device_code(&mut server);
    let _t = server
        .mock("POST", "/login/oauth/access_token")
        .with_status(502)
        .create();
    let w = start(&server, Arc::new(MemoryStore::default()), "Iv1.test");
    w.send(Command::StartDeviceFlow);
    until(&w, |e| {
        matches!(
            e,
            Event::Error {
                during: Op::Auth,
                ..
            }
        )
    });
}

#[test]
fn shutdown_interrupts_a_running_device_flow() {
    let mut server = mockito::Server::new();
    let _c = mock_device_code(&mut server);
    let _t = server
        .mock("POST", "/login/oauth/access_token")
        .with_body(r#"{"error":"authorization_pending"}"#)
        .create();
    let w = start(&server, Arc::new(MemoryStore::default()), "Iv1.test");
    w.send(Command::StartDeviceFlow);
    until(&w, |e| matches!(e, Event::DeviceCode { .. }));
    assert!(w.shutdown(Duration::from_secs(2)));
}

#[test]
fn token_revoked_while_running_signs_out_on_next_call() {
    let mut server = mockito::Server::new();
    let _u = mock_user(&mut server, "ghp_pat");
    let _r = server
        .mock("GET", "/user/repos")
        .match_query(Matcher::Any)
        .with_status(401)
        .create();
    let store = Arc::new(MemoryStore::default());
    let w = start(&server, store.clone(), "");
    w.send(Command::SavePat("ghp_pat".into()));
    until(&w, |e| matches!(e, Event::SignedIn(_)));
    w.send(Command::ListRepos);
    let evs = until(&w, |e| matches!(e, Event::SignedOut));
    assert!(evs.iter().any(|e| matches!(
        e,
        Event::Error {
            during: Op::Repos,
            ..
        }
    )));
    assert_eq!(store.load(), Ok(None));
}

#[test]
fn list_repos_requires_sign_in() {
    let server = mockito::Server::new();
    let w = start(&server, Arc::new(MemoryStore::default()), "");
    w.send(Command::ListRepos);
    until(&w, |e| matches!(e, Event::SignedOut));
}

fn make_source_repo(dir: &Path) {
    let mut opts = git2::RepositoryInitOptions::new();
    opts.initial_head("main");
    let repo = git2::Repository::init_opts(dir, &opts).unwrap();
    std::fs::write(dir.join("README.md"), "hello\n").unwrap();
    let mut idx = repo.index().unwrap();
    idx.add_path(Path::new("README.md")).unwrap();
    let tree = repo.find_tree(idx.write_tree().unwrap()).unwrap();
    let sig = git2::Signature::now("Ada", "ada@example.com").unwrap();
    repo.commit(Some("HEAD"), &sig, &sig, "init", &tree, &[])
        .unwrap();
}

fn file_url(p: &Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    if s.starts_with('/') {
        format!("file://{s}")
    } else {
        format!("file:///{s}")
    }
}

#[test]
fn clone_then_open_report_summaries() {
    let server = mockito::Server::new();
    let src = tempfile::tempdir().unwrap();
    make_source_repo(src.path());
    let out = tempfile::tempdir().unwrap();
    let dest = out.path().join("demo");
    let w = start(&server, Arc::new(MemoryStore::default()), "");
    w.send(Command::Clone {
        url: file_url(src.path()),
        dest: dest.clone(),
    });
    let evs = until(&w, |e| matches!(e, Event::CloneDone(_)));
    assert!(evs.iter().any(|e| matches!(e, Event::CloneProgress(_))));
    w.send(Command::OpenRepo(dest.clone()));
    until(
        &w,
        |e| matches!(e, Event::RepoOpened(s) if s.name == "demo"),
    );
    w.send(Command::OpenRepo(out.path().to_path_buf()));
    until(&w, |e| {
        matches!(
            e,
            Event::Error {
                during: Op::Open(_),
                ..
            }
        )
    });
}

fn signed_in_with_pat(
    server: &mut mockito::Server,
    store: Arc<MemoryStore>,
    client_id: &str,
) -> (WorkerHandle, mockito::Mock) {
    let user = mock_user(server, "ghp_pat");
    let w = start(server, store, client_id);
    w.send(Command::SavePat("ghp_pat".into()));
    until(&w, |e| matches!(e, Event::SignedIn(_)));
    (w, user)
}

#[test]
fn repos_hidden_by_sso_are_listed_with_a_warning_and_link() {
    let mut server = mockito::Server::new();
    let _r = server
        .mock("GET", "/user/repos")
        .match_query(Matcher::Any)
        .with_header("X-GitHub-SSO", "partial-results; organizations=42")
        .with_body("[]")
        .create();
    let (w, _u) = signed_in_with_pat(&mut server, Arc::new(MemoryStore::default()), "Ov23test");
    w.send(Command::ListRepos);
    let evs = until(&w, |e| {
        matches!(
            e,
            Event::Error {
                during: Op::Repos,
                ..
            }
        )
    });
    assert!(evs.iter().any(|e| matches!(e, Event::ReposLoaded(_))));
    let Some(Event::Error { error, .. }) = evs.last() else {
        unreachable!()
    };
    assert_eq!(
        error.link.as_deref(),
        Some("https://github.com/settings/connections/applications/Ov23test")
    );
}

#[test]
fn offline_start_keeps_the_token_for_later_calls() {
    let mut server = mockito::Server::new();
    let _u = server.mock("GET", "/user").with_status(503).create();
    let _r = server
        .mock("GET", "/user/repos")
        .match_query(Matcher::Any)
        .match_header("authorization", "Bearer gho_kept")
        .with_body("[]")
        .create();
    let store = Arc::new(MemoryStore::with_token("gho_kept"));
    let w = start(&server, store.clone(), "");
    w.send(Command::ValidateToken);
    until(&w, |e| matches!(e, Event::Offline));
    assert_eq!(store.load(), Ok(Some("gho_kept".into())));
    w.send(Command::ListRepos);
    until(&w, |e| matches!(e, Event::ReposLoaded(_)));
}

fn git_401(server: &mut mockito::Server) -> mockito::Mock {
    server
        .mock("GET", Matcher::Regex("^/org/demo.git/".into()))
        .with_status(401)
        .with_header("WWW-Authenticate", "Basic realm=\"GitHub\"")
        .create()
}

#[test]
fn clone_auth_failure_with_revoked_token_signs_out() {
    let mut server = mockito::Server::new();
    let store = Arc::new(MemoryStore::default());
    let (w, user_ok) = signed_in_with_pat(&mut server, store.clone(), "");
    user_ok.remove();
    let _u = server.mock("GET", "/user").with_status(401).create();
    let _g = git_401(&mut server);
    let out = tempfile::tempdir().unwrap();
    w.send(Command::Clone {
        url: format!("{}/org/demo.git", server.url()),
        dest: out.path().join("demo"),
    });
    let evs = until(&w, |e| matches!(e, Event::SignedOut));
    assert!(evs.iter().any(|e| matches!(e, Event::Error { during: Op::Clone, error } if error.message == retrogit::strings::ERR_UNAUTHORIZED)));
    assert_eq!(store.load(), Ok(None));
}

#[test]
fn clone_auth_failure_with_valid_token_points_to_sso() {
    let mut server = mockito::Server::new();
    let store = Arc::new(MemoryStore::default());
    let (w, _user_ok) = signed_in_with_pat(&mut server, store.clone(), "");
    let _g = git_401(&mut server);
    let out = tempfile::tempdir().unwrap();
    w.send(Command::Clone {
        url: format!("{}/org/demo.git", server.url()),
        dest: out.path().join("demo"),
    });
    let evs = until(&w, |e| {
        matches!(
            e,
            Event::Error {
                during: Op::Clone,
                ..
            }
        )
    });
    let Some(Event::Error { error, .. }) = evs.last() else {
        unreachable!()
    };
    assert_eq!(error.message, retrogit::strings::ERR_GIT_AUTH);
    assert_eq!(
        error.link.as_deref(),
        Some("https://github.com/settings/tokens")
    );
    assert_eq!(store.load(), Ok(Some("ghp_pat".into())));
}

fn repo_for_changes() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    make_source_repo(d.path());
    let repo = git2::Repository::open(d.path()).unwrap();
    // make_source_repo commits without writing the index file: sync it with HEAD.
    let head = repo.head().unwrap().peel_to_commit().unwrap();
    repo.reset(head.as_object(), git2::ResetType::Mixed, None)
        .unwrap();
    let mut cfg = repo.config().unwrap();
    cfg.set_str("user.name", "Ada").unwrap();
    cfg.set_str("user.email", "ada@example.com").unwrap();
    d
}

#[test]
fn open_stage_commit_flow() {
    let server = mockito::Server::new();
    let d = repo_for_changes();
    std::fs::write(d.path().join("README.md"), "hello\nworld\n").unwrap();
    let w = start(&server, Arc::new(MemoryStore::default()), "");
    w.send(Command::OpenRepo(d.path().to_path_buf()));
    let evs = until(&w, |e| matches!(e, Event::StatusLoaded(_)));
    assert!(evs.iter().any(|e| matches!(e, Event::RepoOpened(_))));
    let Some(Event::StatusLoaded(files)) = evs.last() else {
        unreachable!()
    };
    assert_eq!(files.len(), 1);
    assert!(
        files[0].unstaged.is_some() && files[0].staged.is_none(),
        "{files:?}"
    );

    w.send(Command::LoadDiff {
        path: "README.md".into(),
        side: gitcore::Side::Unstaged,
    });
    let evs = until(&w, |e| matches!(e, Event::DiffLoaded(_)));
    let Some(Event::DiffLoaded(diff)) = evs.last() else {
        unreachable!()
    };
    assert_eq!(diff.hunks.len(), 1);

    w.send(Command::Stage {
        path: "README.md".into(),
        selection: gitcore::Selection::All,
        shown: None,
    });
    let evs = until(&w, |e| matches!(e, Event::DiffLoaded(_)));
    let status = evs.iter().find_map(|e| match e {
        Event::StatusLoaded(f) => Some(f.clone()),
        _ => None,
    });
    assert!(status.unwrap()[0].staged.is_some());

    w.send(Command::Commit {
        message: "Say world".into(),
        amend: false,
    });
    let evs = until(&w, |e| matches!(e, Event::StatusLoaded(_)));
    assert!(
        evs.iter()
            .any(|e| matches!(e, Event::Committed(o) if o.commit.summary == "Say world"))
    );
    assert!(evs.iter().any(|e| matches!(e, Event::RepoOpened(s) if s.last_commit.as_ref().unwrap().summary == "Say world")));
    let Some(Event::StatusLoaded(files)) = evs.last() else {
        unreachable!()
    };
    assert!(files.is_empty());

    w.send(Command::LoadAmendInfo);
    until(
        &w,
        |e| matches!(e, Event::AmendInfo { message: Some(m), pushed: false } if m == "Say world"),
    );
}

#[test]
fn stale_selection_reports_and_resyncs() {
    let server = mockito::Server::new();
    let d = repo_for_changes();
    std::fs::write(d.path().join("README.md"), "hello\nnew\n").unwrap();
    let w = start(&server, Arc::new(MemoryStore::default()), "");
    w.send(Command::OpenRepo(d.path().to_path_buf()));
    until(&w, |e| matches!(e, Event::StatusLoaded(_)));
    w.send(Command::LoadDiff {
        path: "README.md".into(),
        side: gitcore::Side::Unstaged,
    });
    let evs = until(&w, |e| matches!(e, Event::DiffLoaded(_)));
    let Some(Event::DiffLoaded(shown)) = evs.last().cloned() else {
        unreachable!()
    };
    std::fs::write(d.path().join("README.md"), "hello\nnew\nmore\n").unwrap();
    w.send(Command::Stage {
        path: "README.md".into(),
        selection: gitcore::Selection::Lines(vec![(0, 1)]),
        shown: Some(shown),
    });
    let evs = until(&w, |e| matches!(e, Event::DiffLoaded(_)));
    assert!(evs.iter().any(|e| matches!(e, Event::Error { during: Op::Changes, error } if error.message == retrogit::strings::INFO_STALE_SELECTION)));
}

#[test]
fn changes_commands_without_an_open_repo_do_nothing() {
    let server = mockito::Server::new();
    let w = start(&server, Arc::new(MemoryStore::default()), "");
    w.send(Command::RefreshStatus);
    w.send(Command::ValidateToken);
    // The first event is the reply to ValidateToken: RefreshStatus was ignored.
    let evs = until(&w, |e| matches!(e, Event::SignedOut));
    assert_eq!(evs.len(), 1);
}

#[test]
fn refresh_requests_are_deduplicated_while_one_is_pending() {
    let mut server = mockito::Server::new();
    let _c = mock_device_code(&mut server);
    let _t = server
        .mock("POST", "/login/oauth/access_token")
        .with_body(r#"{"error":"authorization_pending"}"#)
        .create();
    let d = repo_for_changes();
    let w = start(&server, Arc::new(MemoryStore::default()), "Iv1.test");
    w.send(Command::OpenRepo(d.path().to_path_buf()));
    until(&w, |e| matches!(e, Event::StatusLoaded(_)));
    // Keep the worker busy (waiting for authorization) while refreshes pile up.
    w.send(Command::StartDeviceFlow);
    until(&w, |e| matches!(e, Event::DeviceCode { .. }));
    let refresh = w.refresher();
    for _ in 0..10 {
        refresh();
        w.send(Command::RefreshStatus);
    }
    w.cancel_device_flow();
    w.send(Command::ValidateToken); // marker: replies SignedOut
    let evs = until(&w, |e| matches!(e, Event::SignedOut));
    let refreshes = evs
        .iter()
        .filter(|e| matches!(e, Event::StatusLoaded(_)))
        .count();
    assert_eq!(refreshes, 1, "{evs:?}");
}

#[test]
fn stage_all_is_one_operation_with_one_refresh() {
    let server = mockito::Server::new();
    let d = repo_for_changes();
    for i in 0..20 {
        std::fs::write(d.path().join(format!("f{i}.txt")), "x\n").unwrap();
    }
    let w = start(&server, Arc::new(MemoryStore::default()), "");
    w.send(Command::OpenRepo(d.path().to_path_buf()));
    until(&w, |e| matches!(e, Event::StatusLoaded(_)));
    let paths: Vec<String> = (0..20).map(|i| format!("f{i}.txt")).collect();
    w.send(Command::StageFiles(paths.clone()));
    w.send(Command::ValidateToken); // marker
    let evs = until(&w, |e| matches!(e, Event::SignedOut));
    let statuses: Vec<_> = evs
        .iter()
        .filter_map(|e| match e {
            Event::StatusLoaded(f) => Some(f),
            _ => None,
        })
        .collect();
    assert_eq!(statuses.len(), 1);
    assert_eq!(
        statuses[0].iter().filter(|f| f.staged.is_some()).count(),
        20
    );
    w.send(Command::UnstageFiles(paths));
    let evs = until(&w, |e| matches!(e, Event::StatusLoaded(_)));
    let Some(Event::StatusLoaded(files)) = evs.last() else {
        unreachable!()
    };
    assert!(files.iter().all(|f| f.staged.is_none()));
}

#[test]
fn discard_commands_revert_the_working_tree_and_refresh() {
    let server = mockito::Server::new();
    let d = repo_for_changes();
    std::fs::write(d.path().join("README.md"), "hello\nnoise\n").unwrap();
    let w = start(&server, Arc::new(MemoryStore::default()), "");
    w.send(Command::OpenRepo(d.path().to_path_buf()));
    until(&w, |e| matches!(e, Event::StatusLoaded(_)));
    w.send(Command::Discard {
        path: "README.md".into(),
        selection: gitcore::Selection::Hunks(vec![0]),
        shown: None,
    });
    let evs = until(&w, |e| matches!(e, Event::StatusLoaded(_)));
    let Some(Event::StatusLoaded(files)) = evs.last() else {
        unreachable!()
    };
    assert!(files.is_empty(), "{files:?}");
    assert_eq!(
        std::fs::read_to_string(d.path().join("README.md")).unwrap(),
        "hello\n"
    );
    std::fs::write(d.path().join("README.md"), "changed\n").unwrap();
    w.send(Command::DiscardFiles(vec!["README.md".into()]));
    until(&w, |e| matches!(e, Event::StatusLoaded(f) if f.is_empty()));
    assert_eq!(
        std::fs::read_to_string(d.path().join("README.md")).unwrap(),
        "hello\n"
    );
}

mod sync {
    use super::*;
    use retrogit::protocol::SyncOp;
    use std::process::Command as Cmd;

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

    fn configure(dir: &Path) {
        for (k, v) in [
            ("user.name", "Ada"),
            ("user.email", "ada@example.com"),
            ("commit.gpgsign", "false"),
            ("core.hooksPath", ".git/hooks"),
        ] {
            git(dir, &["config", k, v]);
        }
    }

    /// Bare remote + a clone tracking it; `None` when git is not installed.
    fn remote_env() -> Option<(tempfile::TempDir, std::path::PathBuf)> {
        if !gitcore::git_available() {
            return None;
        }
        let tmp = tempfile::tempdir().unwrap();
        let root = retrogit::watch::canonical(tmp.path());
        let seed = root.join("seed");
        make_source_repo(&seed);
        git(
            &root,
            &[
                "clone",
                "-q",
                "--bare",
                seed.to_str().unwrap(),
                "origin.git",
            ],
        );
        git(&root, &["clone", "-q", "origin.git", "work"]);
        let work = root.join("work");
        configure(&work);
        Some((tmp, work))
    }

    fn push_from_other(work: &Path, file: &str) {
        let root = work.parent().unwrap();
        let other = root.join(format!("other-{file}"));
        git(
            root,
            &["clone", "-q", "origin.git", other.to_str().unwrap()],
        );
        configure(&other);
        std::fs::write(other.join(file), "remote\n").unwrap();
        git(&other, &["add", file]);
        git(&other, &["commit", "-q", "-m", "remote change"]);
        git(&other, &["push", "-q"]);
    }

    #[test]
    fn opening_a_repo_sends_branches_history_and_signing() {
        let Some((_tmp, work)) = remote_env() else {
            return;
        };
        let server = mockito::Server::new();
        let w = start(&server, Arc::new(MemoryStore::default()), "");
        w.send(Command::OpenRepo(work.clone()));
        let evs = until(&w, |e| matches!(e, Event::LogLoaded { .. }));
        assert!(
            evs.iter()
                .any(|e| matches!(e, Event::SigningLoaded(Some(_))))
        );
        assert!(evs.iter().any(|e| matches!(e, Event::BranchesLoaded(b) if b.iter().any(|b| b.name == "main" && b.is_head))));
        assert!(
            evs.iter()
                .any(|e| matches!(e, Event::OperationChanged(None)))
        );
        let Some(Event::LogLoaded { skip: 0, entries }) = evs.last() else {
            unreachable!()
        };
        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn diverged_pull_asks_then_rebase_succeeds() {
        let Some((_tmp, work)) = remote_env() else {
            return;
        };
        push_from_other(&work, "theirs.txt");
        std::fs::write(work.join("mine.txt"), "mine\n").unwrap();
        git(&work, &["add", "mine.txt"]);
        git(&work, &["commit", "-q", "-m", "mine"]);
        let server = mockito::Server::new();
        let w = start(&server, Arc::new(MemoryStore::default()), "");
        w.send(Command::OpenRepo(work.clone()));
        until(&w, |e| {
            matches!(
                e,
                Event::SyncFinished {
                    op: SyncOp::Fetch,
                    ..
                }
            )
        });
        w.send(Command::Pull(gitcore::PullMode::FastForwardOnly));
        until(&w, |e| {
            matches!(
                e,
                Event::Diverged {
                    ahead: 1,
                    behind: 1
                }
            )
        });
        w.send(Command::Pull(gitcore::PullMode::Rebase));
        let evs = until(&w, |e| matches!(e, Event::Pulled(_)));
        assert!(matches!(
            evs.last(),
            Some(Event::Pulled(gitcore::PullOutcome::Rebased))
        ));
        assert!(work.join("theirs.txt").exists());
    }

    #[test]
    fn rejected_push_and_switch_with_stash() {
        let Some((_tmp, work)) = remote_env() else {
            return;
        };
        let server = mockito::Server::new();
        let w = start(&server, Arc::new(MemoryStore::default()), "");
        w.send(Command::OpenRepo(work.clone()));
        until(&w, |e| {
            matches!(
                e,
                Event::SyncFinished {
                    op: SyncOp::Fetch,
                    ..
                }
            )
        });
        push_from_other(&work, "theirs.txt");
        std::fs::write(work.join("mine.txt"), "mine\n").unwrap();
        git(&work, &["add", "mine.txt"]);
        git(&work, &["commit", "-q", "-m", "mine"]);
        w.send(Command::Push(gitcore::PushMode::Normal));
        until(&w, |e| matches!(e, Event::PushRejected { .. }));

        w.send(Command::CreateBranch {
            name: "side".into(),
            switch: false,
        });
        until(
            &w,
            |e| matches!(e, Event::BranchesLoaded(b) if b.iter().any(|b| b.name == "side")),
        );
        git(&work, &["switch", "-q", "side"]);
        std::fs::write(work.join("README.md"), "on side\n").unwrap();
        git(&work, &["commit", "-q", "-am", "side edit"]);
        git(&work, &["switch", "-q", "main"]);
        std::fs::write(work.join("README.md"), "local edit\n").unwrap();
        w.send(Command::SwitchBranch {
            name: "side".into(),
            stash: false,
        });
        until(
            &w,
            |e| matches!(e, Event::WouldOverwrite { files, .. } if files == &vec!["README.md".to_string()]),
        );
        std::fs::write(work.join("new-untracked.txt"), "keep me\n").unwrap();
        std::fs::write(work.join("README.md"), "hello\n").unwrap(); // non-conflicting now
        w.send(Command::SwitchBranch {
            name: "side".into(),
            stash: true,
        });
        until(
            &w,
            |e| matches!(e, Event::RepoOpened(s) if s.head == gitcore::Head::Branch("side".into())),
        );
        assert_eq!(
            std::fs::read_to_string(work.join("new-untracked.txt")).unwrap(),
            "keep me\n"
        );
    }

    #[test]
    fn an_old_cancel_does_not_kill_the_next_automatic_fetch() {
        let Some((_tmp, work)) = remote_env() else {
            return;
        };
        let server = mockito::Server::new();
        let w = start(&server, Arc::new(MemoryStore::default()), "");
        w.cancel_network(); // e.g. the user cancelled a push earlier
        w.send(Command::OpenRepo(work.clone()));
        until(&w, |e| {
            matches!(
                e,
                Event::SyncFinished {
                    op: SyncOp::Fetch,
                    ok: true
                }
            )
        });
    }

    #[test]
    fn force_push_is_offered_only_for_the_branch_whose_pushed_commit_was_amended() {
        let Some((_tmp, work)) = remote_env() else {
            return;
        };
        let server = mockito::Server::new();
        let w = start(&server, Arc::new(MemoryStore::default()), "");
        w.send(Command::OpenRepo(work.clone()));
        until(&w, |e| {
            matches!(
                e,
                Event::SyncFinished {
                    op: SyncOp::Fetch,
                    ..
                }
            )
        });
        std::fs::write(work.join("README.md"), "amended\n").unwrap();
        w.send(Command::StageFiles(vec!["README.md".into()]));
        w.send(Command::Commit {
            message: "init (amended)".into(),
            amend: true,
        });
        until(&w, |e| matches!(e, Event::Committed(_)));
        w.send(Command::Push(gitcore::PushMode::Normal));
        until(&w, |e| matches!(e, Event::PushRejected { can_force: true }));
        w.send(Command::ForcePush);
        until(&w, |e| {
            matches!(
                e,
                Event::SyncFinished {
                    op: SyncOp::Push,
                    ok: true
                }
            )
        });

        // Another branch with a rejected push: no force offered (nothing was amended there).
        push_from_other(&work, "theirs.txt");
        w.send(Command::Fetch { background: false });
        until(&w, |e| {
            matches!(
                e,
                Event::SyncFinished {
                    op: SyncOp::Fetch,
                    ..
                }
            )
        });
        std::fs::write(work.join("mine.txt"), "mine\n").unwrap();
        w.send(Command::StageFiles(vec!["mine.txt".into()]));
        w.send(Command::Commit {
            message: "mine".into(),
            amend: false,
        });
        until(&w, |e| matches!(e, Event::Committed(_)));
        w.send(Command::Push(gitcore::PushMode::Normal));
        until(&w, |e| {
            matches!(e, Event::PushRejected { can_force: false })
        });
    }
}
