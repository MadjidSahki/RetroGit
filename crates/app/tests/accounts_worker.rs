#![allow(clippy::unwrap_used)]
//! Several GitHub accounts in the worker, against a mock GitHub.

use std::sync::Arc;
use std::time::{Duration, Instant};

use github::{AccountStore, Client, MemoryAccounts, PrFilter, RepoAccount, TokenProvider};
use mockito::Matcher;
use retrogit::protocol::{Command, Event};
use retrogit::worker::{WorkerDeps, WorkerHandle, spawn};
use serde_json::json;

fn start(
    server: &mockito::Server,
    store: Arc<MemoryAccounts>,
    known: &[&str],
    repo_accounts: &[(&str, &str, bool)],
) -> WorkerHandle {
    let deps = WorkerDeps {
        client: Client::with_bases(&server.url(), &server.url()),
        store,
        client_id: String::new(),
        commit_backend: gitcore::CommitBackend::Git2,
        tokens: TokenProvider::without_gh(),
        known_accounts: known.iter().map(|k| k.to_string()).collect(),
        repo_accounts: repo_accounts
            .iter()
            .map(|(k, l, m)| {
                (
                    k.to_string(),
                    RepoAccount {
                        login: l.to_string(),
                        manual: *m,
                    },
                )
            })
            .collect(),
    };
    spawn(deps, || {})
}

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

/// `GET /user` and `GET /user/orgs` for `token`.
fn user(server: &mut mockito::Server, token: &str, login: &str, orgs: &[&str]) {
    server
        .mock("GET", "/user")
        .match_header("authorization", format!("Bearer {token}").as_str())
        .with_body(json!({ "login": login, "name": null }).to_string())
        .create();
    let orgs: Vec<_> = orgs.iter().map(|o| json!({ "login": o })).collect();
    server
        .mock("GET", "/user/orgs")
        .match_query(Matcher::Any)
        .match_header("authorization", format!("Bearer {token}").as_str())
        .with_body(serde_json::to_string(&orgs).unwrap())
        .create();
}

/// Accounts "ada" (member of "corp") and "bob", both valid.
fn two_accounts(server: &mut mockito::Server) -> (WorkerHandle, Arc<MemoryAccounts>) {
    user(server, "gho_ada", "ada", &["corp"]);
    user(server, "gho_bob", "bob", &[]);
    let store = Arc::new(MemoryAccounts::with("ada", "gho_ada"));
    store.save("bob", "gho_bob").unwrap();
    let w = start(server, store.clone(), &["ada", "bob"], &[]);
    w.send(Command::ValidateToken);
    until(&w, |e| matches!(e, Event::SignedIn(_)));
    (w, store)
}

fn list(server: &mut mockito::Server, token: &str) -> mockito::Mock {
    server
        .mock("POST", "/graphql")
        .match_header("authorization", format!("Bearer {token}").as_str())
        .match_body(Matcher::Regex("ListPulls".into()))
        .with_body(r#"{"data":{"repository":{"id":"R"},"search":{"nodes":[]}}}"#)
        .create()
}

fn load_pulls(w: &WorkerHandle, owner: &str, repo: &str) -> Vec<Event> {
    w.send(Command::LoadPulls {
        slug: (owner.into(), repo.into()),
        filter: PrFilter::Open,
    });
    until(w, |e| {
        matches!(e, Event::PullsLoaded { .. } | Event::Error { .. })
    })
}

#[test]
fn the_single_account_of_older_versions_is_moved_to_its_login() {
    let mut server = mockito::Server::new();
    user(&mut server, "gho_old", "ada", &[]);
    let store = Arc::new(MemoryAccounts::with_legacy("gho_old"));
    let w = start(&server, store.clone(), &[], &[]);
    w.send(Command::ValidateToken);
    let evs = until(&w, |e| matches!(e, Event::SignedIn(u) if u.login == "ada"));
    assert_eq!(store.load("ada").unwrap().as_deref(), Some("gho_old"));
    assert_eq!(store.load_legacy().unwrap(), None);
    assert!(evs.iter().any(
        |e| matches!(e, Event::AccountsChanged(a) if a.len() == 1 && a[0].login == "ada" && a[0].valid)
    ));
}

#[test]
fn a_revoked_account_is_listed_to_sign_in_again_without_touching_the_others() {
    let mut server = mockito::Server::new();
    user(&mut server, "gho_ada", "ada", &[]);
    server
        .mock("GET", "/user")
        .match_header("authorization", "Bearer gho_dead")
        .with_status(401)
        .create();
    let store = Arc::new(MemoryAccounts::with("ada", "gho_ada"));
    store.save("bob", "gho_dead").unwrap();
    let w = start(&server, store.clone(), &["ada", "bob", "carol"], &[]);
    w.send(Command::ValidateToken);
    let evs = until(&w, |e| matches!(e, Event::SignedIn(_)));
    let Some(Event::AccountsChanged(accounts)) =
        evs.iter().find(|e| matches!(e, Event::AccountsChanged(_)))
    else {
        unreachable!()
    };
    let states: Vec<(&str, bool)> = accounts
        .iter()
        .map(|a| (a.login.as_str(), a.valid))
        .collect();
    assert_eq!(states, [("ada", true), ("bob", false), ("carol", false)]);
    assert_eq!(store.load("bob").unwrap(), None, "revoked token removed");
}

#[test]
fn adding_an_account_keeps_the_others() {
    let mut server = mockito::Server::new();
    user(&mut server, "gho_ada", "ada", &[]);
    user(&mut server, "ghp_bob", "bob", &[]);
    let store = Arc::new(MemoryAccounts::with("ada", "gho_ada"));
    let w = start(&server, store.clone(), &["ada"], &[]);
    w.send(Command::ValidateToken);
    until(&w, |e| matches!(e, Event::SignedIn(_)));
    w.send(Command::SavePat("ghp_bob".into()));
    let evs = until(&w, |e| matches!(e, Event::SignedIn(u) if u.login == "bob"));
    assert!(
        evs.iter()
            .any(|e| matches!(e, Event::AccountsChanged(a) if a.len() == 2))
    );
    assert_eq!(store.load("ada").unwrap().as_deref(), Some("gho_ada"));
    assert_eq!(store.load("bob").unwrap().as_deref(), Some("ghp_bob"));
}

#[test]
fn each_repository_is_read_with_its_owners_account() {
    let mut server = mockito::Server::new();
    let (w, _) = two_accounts(&mut server);
    let ada = list(&mut server, "gho_ada");
    let bob = list(&mut server, "gho_bob");
    assert!(matches!(
        load_pulls(&w, "corp", "x").last(),
        Some(Event::PullsLoaded { .. })
    ));
    assert!(matches!(
        load_pulls(&w, "Bob", "y").last(),
        Some(Event::PullsLoaded { .. })
    ));
    ada.assert();
    bob.assert();
}

#[test]
fn an_unknown_owner_is_tried_with_each_account_and_remembered() {
    let mut server = mockito::Server::new();
    let (w, _) = two_accounts(&mut server);
    let hidden = server
        .mock("POST", "/graphql")
        .match_header("authorization", "Bearer gho_ada")
        .match_body(Matcher::PartialJson(json!({ "variables": { "owner": "third", "name": "app" } })))
        .with_body(r#"{"data":{"repository":null},"errors":[{"type":"NOT_FOUND","message":"Could not resolve to a Repository with the name 'third/app'."}]}"#)
        .expect(1)
        .create();
    let seen = server
        .mock("POST", "/graphql")
        .match_header("authorization", "Bearer gho_bob")
        .match_body(Matcher::Regex(
            "repository\\(owner: \\$owner, name: \\$name\\) \\{ id \\} \\}".into(),
        ))
        .with_body(r#"{"data":{"repository":{"id":"R"}}}"#)
        .expect(1)
        .create();
    let bob = list(&mut server, "gho_bob").expect(2);
    let evs = load_pulls(&w, "third", "app");
    assert!(
        matches!(evs.last(), Some(Event::PullsLoaded { .. })),
        "{evs:?}"
    );
    assert!(evs.iter().any(|e| matches!(
        e,
        Event::RepoAccountLearned { key, account: Some(a) } if key == "third/app" && a.login == "bob" && !a.manual
    )));
    // Remembered: no second search.
    load_pulls(&w, "third", "app");
    hidden.assert();
    seen.assert();
    bob.assert();
}

#[test]
fn the_users_choice_wins_and_is_remembered() {
    let mut server = mockito::Server::new();
    let (w, _) = two_accounts(&mut server);
    w.send(Command::SetRepoAccount {
        slug: ("corp".into(), "x".into()),
        login: Some("bob".into()),
    });
    let evs = until(&w, |e| matches!(e, Event::RepoAccountLearned { .. }));
    assert!(matches!(
        evs.last(),
        Some(Event::RepoAccountLearned { key, account: Some(a) }) if key == "corp/x" && a.login == "bob" && a.manual
    ));
    let bob = list(&mut server, "gho_bob");
    load_pulls(&w, "corp", "x");
    bob.assert();
    w.send(Command::SetRepoAccount {
        slug: ("corp".into(), "x".into()),
        login: None,
    });
    let evs = until(&w, |e| matches!(e, Event::RepoAccountLearned { .. }));
    assert!(matches!(
        evs.last(),
        Some(Event::RepoAccountLearned { account: None, .. })
    ));
}

#[test]
fn a_learned_account_that_lost_access_is_replaced() {
    let mut server = mockito::Server::new();
    user(&mut server, "gho_ada", "ada", &[]);
    user(&mut server, "gho_bob", "bob", &[]);
    let store = Arc::new(MemoryAccounts::with("ada", "gho_ada"));
    store.save("bob", "gho_bob").unwrap();
    // Remembered from an earlier session: ada.
    let w = start(
        &server,
        store,
        &["ada", "bob"],
        &[("lab/app", "ada", false)],
    );
    w.send(Command::ValidateToken);
    until(&w, |e| matches!(e, Event::SignedIn(_)));
    server
        .mock("POST", "/graphql")
        .match_header("authorization", "Bearer gho_ada")
        .with_body(r#"{"data":{"repository":null},"errors":[{"type":"NOT_FOUND","message":"Could not resolve to a Repository with the name 'lab/app'."}]}"#)
        .create();
    server
        .mock("POST", "/graphql")
        .match_header("authorization", "Bearer gho_bob")
        .match_body(Matcher::Regex("\\{ id \\} \\}".into()))
        .with_body(r#"{"data":{"repository":{"id":"R"}}}"#)
        .create();
    let bob = list(&mut server, "gho_bob");
    let evs = load_pulls(&w, "lab", "app");
    assert!(
        matches!(evs.last(), Some(Event::PullsLoaded { .. })),
        "{evs:?}"
    );
    assert!(evs.iter().any(
        |e| matches!(e, Event::RepoAccountLearned { account: Some(a), .. } if a.login == "bob")
    ));
    bob.assert();
}

#[test]
fn a_401_signs_out_only_that_account() {
    let mut server = mockito::Server::new();
    let (w, store) = two_accounts(&mut server);
    server
        .mock("POST", "/graphql")
        .match_header("authorization", "Bearer gho_ada")
        .with_status(401)
        .create();
    w.send(Command::LoadPulls {
        slug: ("corp".into(), "x".into()),
        filter: PrFilter::Open,
    });
    let evs = until(&w, |e| matches!(e, Event::Error { .. }));
    assert!(evs.iter().any(|e| matches!(
        e,
        Event::AccountsChanged(a) if a.iter().any(|s| s.login == "ada" && !s.valid) && a.iter().any(|s| s.login == "bob" && s.valid)
    )));
    assert!(!evs.iter().any(|e| matches!(e, Event::SignedOut)));
    assert_eq!(store.load("ada").unwrap(), None);
    assert!(store.load("bob").unwrap().is_some());
}

#[test]
fn removing_an_account_forgets_its_repositories() {
    let mut server = mockito::Server::new();
    user(&mut server, "gho_ada", "ada", &[]);
    let store = Arc::new(MemoryAccounts::with("ada", "gho_ada"));
    let w = start(
        &server,
        store.clone(),
        &["ada"],
        &[("lab/app", "ada", true)],
    );
    w.send(Command::ValidateToken);
    until(&w, |e| matches!(e, Event::SignedIn(_)));
    w.send(Command::RemoveAccount("ada".into()));
    let evs = until(&w, |e| matches!(e, Event::SignedOut));
    assert!(evs.iter().any(
        |e| matches!(e, Event::RepoAccountLearned { key, account: None } if key == "lab/app")
    ));
    assert!(
        evs.iter()
            .any(|e| matches!(e, Event::AccountsChanged(a) if a.is_empty()))
    );
    assert_eq!(store.load("ada").unwrap(), None);
}

#[test]
fn repositories_of_every_account_are_listed_together() {
    let mut server = mockito::Server::new();
    let (w, _) = two_accounts(&mut server);
    let repo = |full: &str| {
        let (o, n) = full.split_once('/').unwrap();
        json!({ "full_name": full, "name": n, "owner": { "login": o }, "private": true,
                "clone_url": format!("https://github.com/{full}.git"), "updated_at": "2026-09-30T10:00:00Z" })
    };
    server
        .mock("GET", "/user/repos")
        .match_query(Matcher::Any)
        .match_header("authorization", "Bearer gho_ada")
        .with_body(json!([repo("corp/x"), repo("shared/s")]).to_string())
        .create();
    server
        .mock("GET", "/user/repos")
        .match_query(Matcher::Any)
        .match_header("authorization", "Bearer gho_bob")
        .with_body(json!([repo("bob/y"), repo("shared/s")]).to_string())
        .create();
    w.send(Command::ListRepos);
    let evs = until(&w, |e| matches!(e, Event::ReposLoaded(_)));
    let Some(Event::ReposLoaded(repos)) = evs.last() else {
        unreachable!()
    };
    let shared = repos.iter().find(|r| r.full_name == "shared/s").unwrap();
    assert_eq!(shared.accounts, ["ada", "bob"]);
    assert_eq!(repos.len(), 3);
}

/// `check_repo` answers for `token` on `owner/name`: seen, or GitHub's NOT_FOUND.
fn probe(
    server: &mut mockito::Server,
    token: &str,
    owner: &str,
    name: &str,
    seen: bool,
) -> mockito::Mock {
    let body = if seen {
        r#"{"data":{"repository":{"id":"R"}}}"#.to_string()
    } else {
        format!(
            r#"{{"data":{{"repository":null}},"errors":[{{"type":"NOT_FOUND","message":"Could not resolve to a Repository with the name '{owner}/{name}'."}}]}}"#
        )
    };
    server
        .mock("POST", "/graphql")
        .match_header("authorization", format!("Bearer {token}").as_str())
        .match_body(Matcher::AllOf(vec![
            Matcher::Regex("\\{ id \\} \\}".into()),
            Matcher::PartialJson(json!({ "variables": { "owner": owner, "name": name } })),
        ]))
        .with_body(body)
        .create()
}

#[test]
fn among_members_of_an_organization_the_one_with_access_is_found() {
    let mut server = mockito::Server::new();
    user(&mut server, "gho_ada", "ada", &["corp"]);
    user(&mut server, "gho_bob", "bob", &["corp"]);
    let store = Arc::new(MemoryAccounts::with("ada", "gho_ada"));
    store.save("bob", "gho_bob").unwrap();
    let w = start(&server, store, &["ada", "bob"], &[]);
    w.send(Command::ValidateToken);
    until(&w, |e| matches!(e, Event::SignedIn(_)));
    // ada is a member but cannot see corp/x; bob can.
    server
        .mock("POST", "/graphql")
        .match_header("authorization", "Bearer gho_ada")
        .match_body(Matcher::Regex("ListPulls".into()))
        .with_body(r#"{"data":{"repository":null},"errors":[{"type":"NOT_FOUND","message":"Could not resolve to a Repository with the name 'corp/x'."}]}"#)
        .create();
    probe(&mut server, "gho_bob", "corp", "x", true);
    let bob = list(&mut server, "gho_bob");
    let evs = load_pulls(&w, "corp", "x");
    assert!(
        matches!(evs.last(), Some(Event::PullsLoaded { .. })),
        "{evs:?}"
    );
    assert!(evs.iter().any(
        |e| matches!(e, Event::RepoAccountLearned { account: Some(a), .. } if a.login == "bob")
    ));
    bob.assert();
}

#[test]
fn a_network_error_while_probing_is_not_remembered_as_no_access() {
    let mut server = mockito::Server::new();
    let (w, _) = two_accounts(&mut server);
    let down = server
        .mock("POST", "/graphql")
        .match_body(Matcher::Regex("\\{ id \\} \\}".into()))
        .with_status(502)
        .expect_at_least(1)
        .create();
    load_pulls(&w, "third", "app");
    down.remove();
    probe(&mut server, "gho_ada", "third", "app", true);
    list(&mut server, "gho_ada");
    let evs = load_pulls(&w, "third", "app");
    assert!(
        matches!(evs.last(), Some(Event::PullsLoaded { .. })),
        "tried again: {evs:?}"
    );
}

#[test]
fn a_chosen_account_that_must_sign_in_again_is_not_replaced_silently() {
    let mut server = mockito::Server::new();
    user(&mut server, "gho_ada", "ada", &["corp"]);
    let store = Arc::new(MemoryAccounts::with("ada", "gho_ada"));
    // bob was chosen for corp/x, but his token is gone.
    let w = start(&server, store, &["ada", "bob"], &[("corp/x", "bob", true)]);
    w.send(Command::ValidateToken);
    until(&w, |e| matches!(e, Event::SignedIn(_)));
    let probes = server.mock("POST", "/graphql").expect(0).create();
    let evs = load_pulls(&w, "corp", "x");
    assert!(
        matches!(
            evs.last(),
            Some(Event::Error { error, .. }) if error.message.contains("@bob")
        ),
        "{evs:?}"
    );
    probes.assert();
}

#[test]
fn repositories_seen_only_with_gh_mark_their_owner_as_restricted() {
    let mut server = mockito::Server::new();
    user(&mut server, "gho_ada", "ada", &[]);
    let store = Arc::new(MemoryAccounts::with("ada", "gho_ada"));
    let tokens = TokenProvider::new(Arc::new(|_: &str| Some("gho_cli".to_string())));
    let deps = WorkerDeps {
        client: Client::with_bases(&server.url(), &server.url()),
        store,
        client_id: String::new(),
        commit_backend: gitcore::CommitBackend::Git2,
        tokens: tokens.clone(),
        known_accounts: vec!["ada".into()],
        repo_accounts: Default::default(),
    };
    let w = spawn(deps, || {});
    w.send(Command::ValidateToken);
    until(&w, |e| matches!(e, Event::SignedIn(_)));
    let repo = |full: &str| {
        let (o, n) = full.split_once('/').unwrap();
        json!({ "full_name": full, "name": n, "owner": { "login": o }, "private": true,
                "clone_url": format!("https://github.com/{full}.git"), "updated_at": "2026-09-30T10:00:00Z" })
    };
    server
        .mock("GET", "/user/repos")
        .match_query(Matcher::Any)
        .match_header("authorization", "Bearer gho_ada")
        .with_body(json!([repo("ada/mine")]).to_string())
        .create();
    server
        .mock("GET", "/user/repos")
        .match_query(Matcher::Any)
        .match_header("authorization", "Bearer gho_cli")
        .with_body(json!([repo("ada/mine"), repo("Corp/x")]).to_string())
        .create();
    w.send(Command::ListRepos);
    until(&w, |e| matches!(e, Event::ReposLoaded(_)));
    // Cloning corp/x will then use gh's token straight away.
    assert!(tokens.is_restricted("ada", "corp"));
    assert!(!tokens.is_restricted("ada", "ada"));
}
