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
