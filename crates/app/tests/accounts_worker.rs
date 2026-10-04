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
    start_with(
        server,
        store,
        known,
        repo_accounts,
        TokenProvider::without_gh(),
    )
}

fn start_with(
    server: &mockito::Server,
    store: Arc<MemoryAccounts>,
    known: &[&str],
    repo_accounts: &[(&str, &str, bool)],
    tokens: TokenProvider,
) -> WorkerHandle {
    let deps = WorkerDeps {
        client: Client::with_bases(&server.url(), &server.url()),
        store,
        client_id: String::new(),
        commit_backend: gitcore::CommitBackend::Git2,
        tokens,
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
    assert_eq!(
        store.load_legacy().unwrap().as_deref(),
        Some("gho_old"),
        "kept until the login is saved in the configuration"
    );
    assert!(evs.iter().any(
        |e| matches!(e, Event::AccountsChanged(a) if a.len() == 1 && a[0].login == "ada" && a[0].valid)
    ));
}

#[test]
fn the_old_single_account_is_cleared_once_its_login_is_known() {
    let mut server = mockito::Server::new();
    user(&mut server, "gho_old", "ada", &[]);
    let store = Arc::new(MemoryAccounts::with_legacy("gho_old"));
    let w = start(&server, store.clone(), &["ada"], &[]);
    w.send(Command::ValidateToken);
    until(&w, |e| matches!(e, Event::SignedIn(u) if u.login == "ada"));
    assert_eq!(store.load("ada").unwrap().as_deref(), Some("gho_old"));
    assert_eq!(store.load_legacy().unwrap(), None);
}

#[test]
fn an_account_removed_after_migrating_does_not_come_back() {
    let mut server = mockito::Server::new();
    user(&mut server, "gho_old", "ada", &[]);
    let store = Arc::new(MemoryAccounts::with_legacy("gho_old"));
    let w = start(&server, store.clone(), &[], &[]);
    w.send(Command::ValidateToken);
    until(&w, |e| matches!(e, Event::SignedIn(u) if u.login == "ada"));
    w.send(Command::RemoveAccount("ada".into()));
    until(&w, |e| matches!(e, Event::SignedOut));
    assert_eq!(store.load_legacy().unwrap(), None, "the old slot goes too");
}

#[test]
fn a_newer_token_is_not_replaced_by_the_old_single_account() {
    let mut server = mockito::Server::new();
    user(&mut server, "gho_old", "ada", &[]);
    user(&mut server, "gho_new", "ada", &[]);
    let store = Arc::new(MemoryAccounts::with_legacy("gho_old"));
    store.save("ada", "gho_new").unwrap();
    let w = start(&server, store.clone(), &["ada"], &[]);
    w.send(Command::ValidateToken);
    until(&w, |e| matches!(e, Event::SignedIn(u) if u.login == "ada"));
    assert_eq!(store.load("ada").unwrap().as_deref(), Some("gho_new"));
    assert_eq!(store.load_legacy().unwrap(), None);
}

#[test]
fn accounts_back_online_sign_the_app_in() {
    let mut server = mockito::Server::new();
    let down = server
        .mock("GET", "/user")
        .match_header("authorization", "Bearer gho_ada")
        .with_status(500)
        .create();
    let store = Arc::new(MemoryAccounts::with("ada", "gho_ada"));
    let w = start(&server, store, &["ada"], &[]);
    w.send(Command::ValidateToken);
    until(&w, |e| matches!(e, Event::Offline));
    down.remove();
    user(&mut server, "gho_ada", "ada", &[]);
    list(&mut server, "gho_ada");
    let evs = load_pulls(&w, "ada", "app");
    if !evs
        .iter()
        .any(|e| matches!(e, Event::SignedIn(u) if u.login == "ada"))
    {
        until(&w, |e| matches!(e, Event::SignedIn(u) if u.login == "ada"));
    }
}

/// bob's `GET /user` fails at startup (offline), then answers with his name.
fn bob_offline_at_start(server: &mut mockito::Server) -> WorkerHandle {
    user(server, "gho_ada", "ada", &[]);
    let down = server
        .mock("GET", "/user")
        .match_header("authorization", "Bearer gho_bob")
        .with_status(500)
        .create();
    let store = Arc::new(MemoryAccounts::with("ada", "gho_ada"));
    store.save("bob", "gho_bob").unwrap();
    let w = start(server, store, &["ada", "bob"], &[]);
    w.send(Command::ValidateToken);
    until(&w, |e| matches!(e, Event::SignedIn(_)));
    down.remove();
    server
        .mock("GET", "/user")
        .match_header("authorization", "Bearer gho_bob")
        .with_body(json!({ "login": "bob", "name": "Bob Smith" }).to_string())
        .create();
    server
        .mock("GET", "/user/orgs")
        .match_query(Matcher::Any)
        .match_header("authorization", "Bearer gho_bob")
        .with_body("[]")
        .create();
    w
}

fn bob_named(e: &Event) -> bool {
    matches!(e, Event::AccountsChanged(a)
        if a.iter().any(|s| s.login == "bob" && s.name.as_deref() == Some("Bob Smith")))
}

#[test]
fn an_account_kept_offline_is_checked_again_on_validate() {
    let mut server = mockito::Server::new();
    let w = bob_offline_at_start(&mut server);
    w.send(Command::ValidateToken);
    until(&w, bob_named);
}

#[test]
fn an_account_kept_offline_is_checked_again_after_a_github_call_succeeds() {
    let mut server = mockito::Server::new();
    let w = bob_offline_at_start(&mut server);
    list(&mut server, "gho_ada");
    let evs = load_pulls(&w, "ada", "app");
    assert!(
        matches!(evs.last(), Some(Event::PullsLoaded { .. })),
        "{evs:?}"
    );
    if !evs.iter().any(bob_named) {
        until(&w, bob_named);
    }
}

#[test]
fn signing_in_tries_restricted_organizations_again() {
    let mut server = mockito::Server::new();
    user(&mut server, "gho_ada", "ada", &[]);
    let tokens = TokenProvider::without_gh();
    tokens.remember("ada", "corp");
    let store = Arc::new(MemoryAccounts::with("ada", "gho_ada"));
    let w = start_with(&server, store, &["ada"], &[], tokens.clone());
    w.send(Command::ValidateToken);
    until(&w, |e| matches!(e, Event::SignedIn(_)));
    assert!(!tokens.is_restricted("ada", "corp"));
    tokens.remember("ada", "corp");
    w.send(Command::SavePat("gho_ada".into()));
    until(&w, |e| matches!(e, Event::SignedIn(_)));
    assert!(!tokens.is_restricted("ada", "corp"));
}

#[test]
fn a_clone_no_account_can_see_names_the_accounts_tried() {
    use gitcore::GitError;
    use retrogit::strings as s;
    use retrogit::worker::clone_refused;
    let tried = ["ada".to_string(), "bob".to_string()];
    for e in [
        GitError::Auth("could not read Username".into()),
        GitError::AccessDenied("Repository not found".into()),
    ] {
        let err = clone_refused(true, &tried, &e).unwrap();
        assert_eq!(err.message, s::ERR_NO_ACCOUNT_SEES_REPO);
        assert_eq!(err.detail.as_deref(), Some("Tried: @ada, @bob"));
    }
    let auth = GitError::Auth("x".into());
    assert!(
        clone_refused(false, &tried, &auth).is_none(),
        "not github.com"
    );
    assert!(
        clone_refused(true, &[], &auth).is_none(),
        "nobody signed in"
    );
    assert!(clone_refused(true, &tried, &GitError::Network("x".into())).is_none());
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

/// A repository whose origin is `https://github.com/{slug}.git`, opened by `w`; returns
/// once the worker told which account it uses.
fn open_github_repo(w: &WorkerHandle, slug: &str) -> (tempfile::TempDir, Option<String>) {
    let d = tempfile::tempdir().unwrap();
    let r = git2::Repository::init(d.path()).unwrap();
    r.remote("origin", &format!("https://github.com/{slug}.git"))
        .unwrap();
    w.send(Command::OpenRepo(d.path().to_path_buf()));
    let evs = until(w, |e| matches!(e, Event::RepoAccount { .. }));
    let Some(Event::RepoAccount { login, .. }) = evs.last().cloned() else {
        unreachable!()
    };
    (d, login)
}

#[test]
fn signing_in_tells_the_account_of_the_open_repository() {
    let mut server = mockito::Server::new();
    user(&mut server, "ghp_ada", "ada", &["corp"]);
    let w = start(&server, Arc::new(MemoryAccounts::default()), &[], &[]);
    let (_d, before) = open_github_repo(&w, "corp/app");
    assert_eq!(before, None, "no account yet");
    w.send(Command::SavePat("ghp_ada".into()));
    until(&w, |e| matches!(e, Event::SignedIn(u) if u.login == "ada"));
    until(
        &w,
        |e| matches!(e, Event::RepoAccount { login: Some(l), .. } if l == "ada"),
    );
}

#[test]
fn a_rejected_token_tells_the_open_repository_its_account_changed() {
    let mut server = mockito::Server::new();
    let (w, _) = two_accounts(&mut server);
    let (_d, before) = open_github_repo(&w, "corp/x");
    assert_eq!(before.as_deref(), Some("ada"));
    server
        .mock("POST", "/graphql")
        .match_header("authorization", "Bearer gho_ada")
        .with_status(401)
        .create();
    w.send(Command::LoadPulls {
        slug: ("corp".into(), "x".into()),
        filter: PrFilter::Open,
    });
    until(&w, |e| matches!(e, Event::Error { .. }));
    let evs = until(&w, |e| matches!(e, Event::RepoAccount { .. }));
    assert!(
        !matches!(evs.last(), Some(Event::RepoAccount { login: Some(l), .. }) if l == "ada"),
        "ada must sign in again: {evs:?}"
    );
}
