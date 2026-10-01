#![allow(clippy::unwrap_used)]

use github::{ChecksState, Client, PrState, ReviewState};
use mockito::Matcher;
use serde_json::json;

#[test]
fn watch_snapshot_parses_search_results() {
    let mut server = mockito::Server::new();
    let m = server
        .mock("POST", "/graphql")
        .match_body(Matcher::PartialJson(json!({
            "variables": { "q": "is:pr involves:@me updated:>=2026-09-24 sort:updated-desc" }
        })))
        .with_body(
            json!({ "data": { "search": { "nodes": [
                {
                    "number": 7, "title": "Fix login", "url": "https://github.com/o/r/pull/7",
                    "state": "OPEN", "repository": { "nameWithOwner": "o/r" },
                    "author": { "login": "me" }, "mergedBy": null,
                    "comments": { "totalCount": 3, "nodes": [ { "author": { "login": "ada" } } ] },
                    "reviews": { "totalCount": 1, "nodes": [
                        { "author": { "login": "bob" }, "state": "APPROVED" } ] },
                    "commits": { "nodes": [ { "commit": { "oid": "abc", "statusCheckRollup": {
                        "state": "FAILURE",
                        "contexts": { "nodes": [
                            { "__typename": "CheckRun", "status": "COMPLETED", "conclusion": "FAILURE" },
                            { "__typename": "CheckRun", "status": "COMPLETED", "conclusion": "SUCCESS" },
                            { "__typename": "StatusContext", "state": "ERROR" }
                        ] } } } } ] }
                },
                {
                    "number": 2, "title": "Old", "url": "u", "state": "MERGED",
                    "repository": { "nameWithOwner": "x/y" }, "author": null,
                    "mergedBy": { "login": "ada" },
                    "comments": { "totalCount": 0, "nodes": [] },
                    "reviews": { "totalCount": 0, "nodes": [] },
                    "commits": { "nodes": [ { "commit": { "oid": "def", "statusCheckRollup": null } } ] }
                },
                {}
            ] } },
            "errors": [ { "type": "FORBIDDEN", "message": "Although ... the `z` organization has enabled OAuth App access restrictions" } ] })
            .to_string(),
        )
        .create();
    let snaps = Client::with_bases(&server.url(), &server.url())
        .watch_snapshot("t", "2026-09-24")
        .unwrap();
    m.assert();
    assert_eq!(snaps.len(), 2);
    let a = &snaps[0];
    assert_eq!(a.key, "o/r#7");
    assert_eq!(a.checks, ChecksState::Failure);
    assert_eq!(a.failed_checks, 2);
    assert_eq!(a.head, "abc");
    assert_eq!(a.comment_count, 3);
    assert_eq!(a.last_commenter.as_deref(), Some("ada"));
    assert_eq!(a.review_count, 1);
    assert_eq!(a.last_review, Some(("bob".into(), ReviewState::Approved)));
    let b = &snaps[1];
    assert_eq!(b.state, PrState::Merged);
    assert_eq!(b.merged_by.as_deref(), Some("ada"));
    assert_eq!(b.checks, ChecksState::None);
    assert_eq!(b.last_review, None);
}
