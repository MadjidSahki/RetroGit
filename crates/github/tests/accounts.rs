#![allow(clippy::unwrap_used)]

use github::{Client, GithubError};
use mockito::Matcher;

fn client(server: &mockito::Server) -> Client {
    Client::with_bases(&server.url(), &server.url())
}

#[test]
fn organizations_of_an_account() {
    let mut server = mockito::Server::new();
    server
        .mock("GET", "/user/orgs")
        .match_query(Matcher::UrlEncoded("per_page".into(), "100".into()))
        .match_header("authorization", "Bearer t")
        .with_body(r#"[{"login":"Corp","id":1},{"login":"Lab","id":2}]"#)
        .create();
    assert_eq!(client(&server).user_orgs("t").unwrap(), ["Corp", "Lab"]);
}

#[test]
fn checking_access_to_a_repository() {
    let mut server = mockito::Server::new();
    server
        .mock("POST", "/graphql")
        .match_body(Matcher::PartialJson(
            serde_json::json!({ "variables": { "owner": "o", "name": "seen" } }),
        ))
        .with_body(r#"{"data":{"repository":{"id":"R_1"}}}"#)
        .create();
    server
        .mock("POST", "/graphql")
        .match_body(Matcher::PartialJson(serde_json::json!({ "variables": { "name": "hidden" } })))
        .with_body(r#"{"data":{"repository":null},"errors":[{"type":"NOT_FOUND","message":"Could not resolve to a Repository with the name 'o/hidden'."}]}"#)
        .create();
    let c = client(&server);
    assert_eq!(c.check_repo("t", "o", "seen"), Ok(()));
    let e = c.check_repo("t", "o", "hidden").unwrap_err();
    assert!(github::repository_missing(&e), "{e:?}");
    assert!(matches!(e, GithubError::NotFound(_)));
}
