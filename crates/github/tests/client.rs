#![allow(clippy::unwrap_used)]

use github::{Client, GithubError, PollResponse};
use mockito::Matcher;

fn client(server: &mockito::Server) -> Client {
    Client::with_bases(&server.url(), &server.url())
}

fn repo_json(i: usize) -> serde_json::Value {
    serde_json::json!({
        "full_name": format!("ExampleOrg/repo{i}"),
        "name": format!("repo{i}"),
        "owner": { "login": "ExampleOrg" },
        "private": i.is_multiple_of(2),
        "clone_url": format!("https://github.com/ExampleOrg/repo{i}.git"),
        "updated_at": "2026-09-30T10:00:00Z",
        "extra_field_we_ignore": true
    })
}

#[test]
fn current_user_sends_bearer_token() {
    let mut server = mockito::Server::new();
    let m = server
        .mock("GET", "/user")
        .match_header("authorization", "Bearer gho_abc")
        .match_header("user-agent", Matcher::Regex("^RetroGit/".into()))
        .with_body(r#"{"login":"ada","name":"Ada L","id":1}"#)
        .create();
    let u = client(&server).current_user("gho_abc").unwrap();
    assert_eq!(u.login, "ada");
    assert_eq!(u.name.as_deref(), Some("Ada L"));
    m.assert();
}

#[test]
fn unauthorized_maps_to_error() {
    let mut server = mockito::Server::new();
    server
        .mock("GET", "/user")
        .with_status(401)
        .with_body("{}")
        .create();
    assert_eq!(
        client(&server).current_user("bad").err(),
        Some(GithubError::Unauthorized)
    );
}

#[test]
fn sso_header_maps_to_sso_required() {
    let mut server = mockito::Server::new();
    server
        .mock("GET", "/user")
        .with_status(403)
        .with_header(
            "X-GitHub-SSO",
            "required; url=https://github.com/orgs/ExampleOrg/sso?authorization_request=abc",
        )
        .with_body("{}")
        .create();
    assert_eq!(
        client(&server).current_user("t").err(),
        Some(GithubError::SsoRequired {
            url: "https://github.com/orgs/ExampleOrg/sso?authorization_request=abc".into()
        })
    );
}

#[test]
fn rate_limit_maps_to_error() {
    let mut server = mockito::Server::new();
    server
        .mock("GET", "/user")
        .with_status(403)
        .with_header("x-ratelimit-remaining", "0")
        .with_body("{}")
        .create();
    assert_eq!(
        client(&server).current_user("t").err(),
        Some(GithubError::RateLimited)
    );
}

#[test]
fn other_status_maps_to_http() {
    let mut server = mockito::Server::new();
    server.mock("GET", "/user").with_status(500).create();
    assert_eq!(
        client(&server).current_user("t").err(),
        Some(GithubError::Http(500))
    );
}

#[test]
fn bad_json_maps_to_decode() {
    let mut server = mockito::Server::new();
    server.mock("GET", "/user").with_body("not json").create();
    assert!(matches!(
        client(&server).current_user("t"),
        Err(GithubError::Decode(_))
    ));
}

#[test]
fn unreachable_server_maps_to_network() {
    let c = Client::with_bases("http://127.0.0.1:9", "http://127.0.0.1:9");
    assert!(matches!(c.current_user("t"), Err(GithubError::Network(_))));
}

#[test]
fn list_repos_follows_pagination() {
    let mut server = mockito::Server::new();
    let page1: Vec<_> = (0..100).map(repo_json).collect();
    let page2: Vec<_> = (100..130).map(repo_json).collect();
    let next = format!(
        "<{}/user/repos?page=2>; rel=\"next\", <{}/user/repos?page=2>; rel=\"last\"",
        server.url(),
        server.url()
    );
    let m1 = server
        .mock("GET", "/user/repos")
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded(
                "affiliation".into(),
                "owner,collaborator,organization_member".into(),
            ),
            Matcher::UrlEncoded("sort".into(), "updated".into()),
            Matcher::UrlEncoded("per_page".into(), "100".into()),
        ]))
        .with_header("link", &next)
        .with_body(serde_json::to_string(&page1).unwrap())
        .create();
    let m2 = server
        .mock("GET", "/user/repos")
        .match_query(Matcher::UrlEncoded("page".into(), "2".into()))
        .with_body(serde_json::to_string(&page2).unwrap())
        .create();
    let listing = client(&server).list_repos("t").unwrap();
    assert!(listing.sso_hidden_orgs.is_empty());
    let repos = listing.repos;
    assert_eq!(repos.len(), 130);
    assert_eq!(repos[0].owner, "ExampleOrg");
    assert_eq!(repos[0].full_name, "ExampleOrg/repo0");
    assert!(repos[0].private);
    assert_eq!(repos[129].name, "repo129");
    m1.assert();
    m2.assert();
}

#[test]
fn device_code_request_and_poll() {
    let mut server = mockito::Server::new();
    server
        .mock("POST", "/login/device/code")
        .match_header("accept", "application/json")
        .match_body(Matcher::AllOf(vec![
            Matcher::UrlEncoded("client_id".into(), "Iv1.test".into()),
            Matcher::UrlEncoded("scope".into(), "repo read:org workflow".into()),
        ]))
        .with_body(r#"{"device_code":"dc1","user_code":"ABCD-1234","verification_uri":"https://github.com/login/device","expires_in":900,"interval":5}"#)
        .create();
    server
        .mock("POST", "/login/oauth/access_token")
        .match_body(Matcher::AllOf(vec![
            Matcher::UrlEncoded("device_code".into(), "dc1".into()),
            Matcher::UrlEncoded(
                "grant_type".into(),
                "urn:ietf:params:oauth:grant-type:device_code".into(),
            ),
        ]))
        .with_body(r#"{"error":"authorization_pending"}"#)
        .create();
    let c = client(&server);
    let code = c.request_device_code("Iv1.test", github::SCOPES).unwrap();
    assert_eq!(code.user_code, "ABCD-1234");
    assert_eq!(code.interval, 5);
    assert_eq!(
        c.poll_token("Iv1.test", "dc1").unwrap(),
        PollResponse::Pending
    );
}

#[test]
fn list_repos_reports_orgs_hidden_by_missing_sso_authorization() {
    let mut server = mockito::Server::new();
    server
        .mock("GET", "/user/repos")
        .match_query(Matcher::Any)
        .with_header(
            "X-GitHub-SSO",
            "partial-results; organizations=21955855,20582480",
        )
        .with_body(serde_json::to_string(&vec![repo_json(1)]).unwrap())
        .create();
    let listing = client(&server).list_repos("t").unwrap();
    assert_eq!(listing.repos.len(), 1);
    assert_eq!(
        listing.sso_hidden_orgs,
        vec!["21955855".to_string(), "20582480".to_string()]
    );
}
