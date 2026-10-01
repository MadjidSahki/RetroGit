#![allow(clippy::unwrap_used)]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use github::{Client, GithubError, TokenProvider};
use mockito::Matcher;
use serde_json::json;

fn client(server: &mockito::Server) -> Client {
    Client::with_bases(&server.url(), &server.url())
}

const RESTRICTED: &str = "Although you appear to have the correct authorization credentials, the \
    `ExampleOrg` organization has enabled OAuth App access restrictions, meaning that data \
    access to third-parties is limited.";

#[test]
fn graphql_returns_data() {
    let mut server = mockito::Server::new();
    let m = server
        .mock("POST", "/graphql")
        .match_header("authorization", "Bearer t")
        .match_body(Matcher::PartialJson(json!({
            "query": "query { viewer { login } }",
            "variables": { "n": 1 }
        })))
        .with_body(r#"{"data":{"viewer":{"login":"ada"}}}"#)
        .create();
    let data = client(&server)
        .graphql("t", "query { viewer { login } }", json!({ "n": 1 }))
        .unwrap();
    assert_eq!(data["viewer"]["login"], "ada");
    m.assert();
}

#[test]
fn graphql_oauth_restriction_is_typed() {
    let mut server = mockito::Server::new();
    server
        .mock("POST", "/graphql")
        .with_body(
            json!({
                "data": { "repository": null },
                "errors": [{ "type": "FORBIDDEN", "message": RESTRICTED }]
            })
            .to_string(),
        )
        .create();
    assert_eq!(
        client(&server).graphql("t", "q", json!({})).err(),
        Some(GithubError::OAuthRestricted {
            org: Some("ExampleOrg".into())
        })
    );
}

#[test]
fn graphql_rate_limit_is_typed() {
    let mut server = mockito::Server::new();
    server
        .mock("POST", "/graphql")
        .with_body(r#"{"errors":[{"type":"RATE_LIMITED","message":"API rate limit exceeded"}]}"#)
        .create();
    assert_eq!(
        client(&server).graphql("t", "q", json!({})).err(),
        Some(GithubError::RateLimited)
    );
}

#[test]
fn graphql_partial_keeps_data_and_errors() {
    let mut server = mockito::Server::new();
    server
        .mock("POST", "/graphql")
        .with_body(
            json!({
                "data": { "search": { "nodes": [ { "number": 1 } ] } },
                "errors": [{ "type": "FORBIDDEN", "message": RESTRICTED }]
            })
            .to_string(),
        )
        .create();
    let r = client(&server)
        .graphql_partial("t", "q", json!({}))
        .unwrap();
    assert_eq!(r.data["search"]["nodes"][0]["number"], 1);
    assert_eq!(r.errors.len(), 1);
}

#[test]
fn rest_403_oauth_restriction_is_typed() {
    let mut server = mockito::Server::new();
    server
        .mock("GET", "/user")
        .with_status(403)
        .with_body(json!({ "message": RESTRICTED }).to_string())
        .create();
    assert_eq!(
        client(&server).current_user("t").err(),
        Some(GithubError::OAuthRestricted {
            org: Some("ExampleOrg".into())
        })
    );
}

fn provider(gh: Option<&'static str>, calls: Arc<AtomicUsize>) -> TokenProvider {
    TokenProvider::new(Arc::new(move |_login: Option<&str>| {
        calls.fetch_add(1, Ordering::SeqCst);
        gh.map(str::to_string)
    }))
}

fn restricted() -> GithubError {
    GithubError::OAuthRestricted {
        org: Some("ExampleOrg".into()),
    }
}

#[test]
fn provider_falls_back_to_gh_and_remembers_the_org() {
    let gh_calls = Arc::new(AtomicUsize::new(0));
    let p = provider(Some("gho_cli"), gh_calls.clone());
    let used = std::sync::Mutex::new(Vec::new());
    let call = |t: &str| {
        used.lock().unwrap().push(t.to_string());
        if t == "gho_app" {
            Err(restricted())
        } else {
            Ok(t.len())
        }
    };
    assert_eq!(p.with_token(Some("gho_app"), "ExampleOrg", call), Ok(7));
    assert_eq!(*used.lock().unwrap(), vec!["gho_app", "gho_cli"]);
    // Remembered (case-insensitive): gh straight away for that owner.
    used.lock().unwrap().clear();
    assert_eq!(p.with_token(Some("gho_app"), "exampleorg", call), Ok(7));
    assert_eq!(*used.lock().unwrap(), vec!["gho_cli"]);
    // Other owners still use RetroGit's token first.
    used.lock().unwrap().clear();
    let _ = p.with_token(Some("gho_app"), "ada", |t: &str| {
        used.lock().unwrap().push(t.to_string());
        Ok::<_, GithubError>(())
    });
    assert_eq!(*used.lock().unwrap(), vec!["gho_app"]);
}

#[test]
fn provider_keeps_the_first_error_without_gh() {
    let p = provider(None, Arc::new(AtomicUsize::new(0)));
    let r: Result<(), _> = p.with_token(Some("gho_app"), "ExampleOrg", |_| Err(restricted()));
    assert_eq!(r, Err(restricted()));
    // gh refused too: the restriction (the actionable message) is reported.
    let p = provider(Some("gho_cli"), Arc::new(AtomicUsize::new(0)));
    let r: Result<(), _> = p.with_token(Some("gho_app"), "ExampleOrg", |t| {
        if t == "gho_app" {
            Err(restricted())
        } else {
            Err(GithubError::Unauthorized)
        }
    });
    assert_eq!(r, Err(restricted()));
}

#[test]
fn provider_does_not_use_gh_for_other_errors() {
    let gh_calls = Arc::new(AtomicUsize::new(0));
    let p = provider(Some("gho_cli"), gh_calls.clone());
    let r: Result<(), _> = p.with_token(Some("gho_app"), "o", |_| Err(GithubError::Http(500)));
    assert_eq!(r, Err(GithubError::Http(500)));
    let r: Result<(), _> = p.with_token(Some("gho_app"), "o", |_| Err(GithubError::Unauthorized));
    assert_eq!(r, Err(GithubError::Unauthorized));
    assert_eq!(gh_calls.load(Ordering::SeqCst), 0);
}

#[test]
fn provider_without_retrogit_token_uses_gh() {
    let p = provider(Some("gho_cli"), Arc::new(AtomicUsize::new(0)));
    assert_eq!(
        p.with_token(None, "o", |t| Ok(t.to_string())),
        Ok("gho_cli".into())
    );
    let p = TokenProvider::without_gh();
    assert_eq!(
        p.with_token(None, "o", |t| Ok(t.to_string())),
        Err(GithubError::Unauthorized)
    );
}

#[test]
fn provider_asks_gh_for_the_signed_in_account() {
    let asked = Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen = asked.clone();
    let p = TokenProvider::new(Arc::new(move |login: Option<&str>| {
        seen.lock().unwrap().push(login.map(str::to_string));
        Some("gho_cli".to_string())
    }));
    let _ = p.gh_token();
    p.set_login(Some("ada"));
    let _ = p.gh_token();
    assert_eq!(*asked.lock().unwrap(), vec![None, Some("ada".to_string())]);
}

#[test]
fn gh_token_output_is_parsed() {
    assert_eq!(
        github::parse_gh_token("gho_abc\n").as_deref(),
        Some("gho_abc")
    );
    assert_eq!(
        github::parse_gh_token("\n  gho_abc  \n").as_deref(),
        Some("gho_abc")
    );
    assert_eq!(github::parse_gh_token("no oauth token found"), None);
    assert_eq!(github::parse_gh_token(""), None);
}

#[cfg(unix)]
#[test]
fn gh_auth_token_runs_the_cli_without_the_callers_token() {
    use std::os::unix::fs::PermissionsExt;
    let d = tempfile::tempdir().unwrap();
    let gh = d.path().join("gh");
    // Prints its arguments and whether GH_TOKEN leaked through.
    std::fs::write(
        &gh,
        "#!/bin/sh\nif [ -n \"$GH_TOKEN\" ]; then echo leaked; exit 0; fi\n\
         case \"$*\" in *--user\\ ada*) echo gho_for_ada;; *--user*) exit 1;; *) echo gho_active;; esac\n",
    )
    .unwrap();
    std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!("{}:/usr/bin:/bin", d.path().display());
    assert_eq!(
        github::gh_auth_token(Some(&path), Some("ada")).as_deref(),
        Some("gho_for_ada")
    );
    assert_eq!(
        github::gh_auth_token(Some(&path), Some("bob")).as_deref(),
        Some("gho_active")
    );
    assert_eq!(
        github::gh_auth_token(Some(&path), None).as_deref(),
        Some("gho_active")
    );
    let empty = tempfile::tempdir().unwrap();
    assert_eq!(
        github::gh_auth_token(Some(&empty.path().display().to_string()), None),
        None
    );
}
