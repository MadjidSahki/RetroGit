#![allow(clippy::unwrap_used)]
#![recursion_limit = "256"]

use github::{
    CheckStatus, ChecksState, Client, DiffSide, GithubError, MergeMethod, Mergeable, PrFilter,
    PrState, ReviewDecision, ReviewState, TimelineItem, search_query,
};
use mockito::Matcher;
use serde_json::json;

fn client(server: &mockito::Server) -> Client {
    Client::with_bases(&server.url(), &server.url())
}

fn list_node(number: u64, rollup: Option<&str>) -> serde_json::Value {
    json!({
        "number": number,
        "title": format!("PR {number}"),
        "url": format!("https://github.com/o/r/pull/{number}"),
        "isDraft": number == 3,
        "state": "OPEN",
        "updatedAt": "2026-10-01T08:00:00Z",
        "author": { "login": "ada" },
        "headRefName": "feat/x",
        "baseRefName": "main",
        "reviewDecision": if number == 1 { json!("APPROVED") } else { json!(null) },
        "labels": { "nodes": [ { "name": "bug", "color": "d73a4a", "description": "Broken" } ] },
        "commits": { "nodes": [ { "commit": { "statusCheckRollup":
            rollup.map(|s| json!({ "state": s })).unwrap_or(json!(null)) } } ] }
    })
}

#[test]
fn list_filters_build_the_search_query() {
    assert_eq!(
        search_query("o", "r", PrFilter::Open),
        "repo:o/r is:pr is:open sort:updated-desc"
    );
    assert_eq!(
        search_query("o", "r", PrFilter::Mine),
        "repo:o/r is:pr is:open author:@me sort:updated-desc"
    );
    assert_eq!(
        search_query("o", "r", PrFilter::ReviewRequested),
        "repo:o/r is:pr is:open review-requested:@me sort:updated-desc"
    );
    assert_eq!(
        search_query("o", "r", PrFilter::Closed),
        "repo:o/r is:pr is:closed sort:updated-desc"
    );
}

#[test]
fn list_pulls_parses_checks_reviews_and_labels() {
    let mut server = mockito::Server::new();
    let m = server
        .mock("POST", "/graphql")
        .match_body(Matcher::PartialJson(json!({
            // `owner`/`name` let GitHub report an OAuth restriction (search alone hides it).
            "variables": { "q": "repo:o/r is:pr is:open author:@me sort:updated-desc",
                           "owner": "o", "name": "r" }
        })))
        .with_body(
            json!({ "data": { "search": { "nodes": [
                list_node(1, Some("SUCCESS")),
                list_node(2, Some("FAILURE")),
                list_node(3, Some("PENDING")),
                list_node(4, None),
                {}
            ] } } })
            .to_string(),
        )
        .create();
    let list = client(&server)
        .list_pulls("t", "o", "r", PrFilter::Mine)
        .unwrap();
    m.assert();
    assert_eq!(list.len(), 4, "non-PR search results are skipped");
    let checks: Vec<_> = list.iter().map(|p| p.checks).collect();
    assert_eq!(
        checks,
        [
            ChecksState::Success,
            ChecksState::Failure,
            ChecksState::Pending,
            ChecksState::None
        ]
    );
    assert_eq!(list[0].review_decision, ReviewDecision::Approved);
    assert_eq!(list[1].review_decision, ReviewDecision::None);
    assert!(list[2].draft);
    assert_eq!(list[0].labels[0].name, "bug");
    assert_eq!(list[0].labels[0].color, [0xd7, 0x3a, 0x4a]);
    assert_eq!(list[0].labels[0].description.as_deref(), Some("Broken"));
    assert_eq!(list[0].author, "ada");
    assert_eq!(
        (list[0].head.as_str(), list[0].base.as_str()),
        ("feat/x", "main")
    );
    assert_eq!(list[0].state, PrState::Open);
}

fn detail_json() -> serde_json::Value {
    json!({ "data": {
        "viewer": { "login": "bob" },
        "repository": {
            "mergeCommitAllowed": false,
            "squashMergeAllowed": true,
            "rebaseMergeAllowed": true,
            "viewerPermission": "WRITE",
            "labels": { "nodes": [
                { "name": "bug", "color": "d73a4a", "description": null },
                { "name": "docs", "color": "0075ca", "description": "" }
            ] },
            "pullRequest": {
                "number": 7, "title": "Fix login", "url": "https://github.com/o/r/pull/7",
                "isDraft": false, "state": "OPEN", "updatedAt": "2026-10-01T09:00:00Z",
                "body": "Fixes **login**.",
                "author": { "login": "ada" },
                "headRefName": "fix/login", "baseRefName": "main",
                "headRefOid": "abc123",
                "isCrossRepository": true,
                "headRepository": { "name": "r", "owner": { "login": "ada" } },
                "reviewDecision": "CHANGES_REQUESTED",
                "mergeable": "CONFLICTING",
                "mergeStateStatus": "DIRTY",
                "viewerDidAuthor": false,
                "labels": { "nodes": [ { "name": "bug", "color": "d73a4a", "description": null } ] },
                "commits": { "totalCount": 2, "nodes": [
                    { "commit": { "oid": "c1", "abbreviatedOid": "c1", "messageHeadline": "First",
                        "committedDate": "2026-10-01T07:00:00Z",
                        "author": { "name": "Ada L", "user": { "login": "ada" } } } },
                    { "commit": { "oid": "c2", "abbreviatedOid": "c2", "messageHeadline": "Second",
                        "committedDate": "2026-10-01T07:30:00Z",
                        "author": { "name": "Someone", "user": null } } }
                ] },
                "lastCommit": { "nodes": [ { "commit": { "statusCheckRollup": {
                    "state": "FAILURE",
                    "contexts": { "nodes": [
                        { "__typename": "CheckRun", "name": "build (macOS)", "status": "COMPLETED",
                          "conclusion": "SUCCESS", "detailsUrl": "https://x/1",
                          "startedAt": "2026-10-01T07:31:00Z", "completedAt": "2026-10-01T07:33:00Z" },
                        { "__typename": "CheckRun", "name": "build (Windows)", "status": "COMPLETED",
                          "conclusion": "FAILURE", "detailsUrl": "https://x/2",
                          "startedAt": null, "completedAt": null },
                        { "__typename": "CheckRun", "name": "release", "status": "COMPLETED",
                          "conclusion": "SKIPPED", "detailsUrl": null,
                          "startedAt": null, "completedAt": null },
                        { "__typename": "CheckRun", "name": "lint", "status": "IN_PROGRESS",
                          "conclusion": null, "detailsUrl": null,
                          "startedAt": null, "completedAt": null },
                        { "__typename": "StatusContext", "context": "ci/legacy", "state": "PENDING",
                          "targetUrl": "https://x/3" }
                    ] }
                } } } ] },
                "comments": { "nodes": [
                    { "author": { "login": "carol" }, "body": "Later comment", "createdAt": "2026-10-01T08:30:00Z" },
                    { "author": null, "body": "From a deleted account", "createdAt": "2026-10-01T07:40:00Z" }
                ] },
                "reviews": { "nodes": [
                    { "author": { "login": "bob" }, "state": "CHANGES_REQUESTED", "body": "Please fix",
                      "submittedAt": "2026-10-01T08:00:00Z" },
                    { "author": { "login": "bob" }, "state": "PENDING", "body": "",
                      "submittedAt": null }
                ] },
                "reviewThreads": { "nodes": [
                    { "isResolved": false, "isOutdated": false, "path": "src/a.rs",
                      "line": 12, "originalLine": 12, "diffSide": "RIGHT",
                      "comments": { "nodes": [
                        { "databaseId": 991, "author": { "login": "bob" }, "body": "Why?",
                          "createdAt": "2026-10-01T08:00:00Z" },
                        { "databaseId": 992, "author": { "login": "ada" }, "body": "Because.",
                          "createdAt": "2026-10-01T08:10:00Z" }
                      ] } },
                    { "isResolved": true, "isOutdated": true, "path": "src/b.rs",
                      "line": null, "originalLine": 3, "diffSide": "LEFT",
                      "comments": { "nodes": [] } }
                ] }
            }
        }
    } })
}

#[test]
fn a_restricted_repository_is_reported_by_the_list() {
    let mut server = mockito::Server::new();
    server
        .mock("POST", "/graphql")
        .match_body(Matcher::Regex("repository\\(owner: \\$owner, name: \\$name\\)".into()))
        .with_body(r#"{"data":{"repository":null,"search":{"nodes":[]}},"errors":[{"type":"FORBIDDEN","path":["repository"],"message":"the `o` organization has enabled OAuth App access restrictions"}]}"#)
        .create();
    assert_eq!(
        client(&server)
            .list_pulls("t", "o", "r", PrFilter::Open)
            .err(),
        Some(GithubError::OAuthRestricted {
            org: Some("o".into())
        })
    );
}

#[test]
fn pull_detail_parses_timeline_threads_and_merge_options() {
    let mut server = mockito::Server::new();
    let m = server
        .mock("POST", "/graphql")
        .match_body(Matcher::PartialJson(json!({
            "variables": { "owner": "o", "name": "r", "number": 7 }
        })))
        .with_body(detail_json().to_string())
        .create();
    let d = client(&server).pull_detail("t", "o", "r", 7).unwrap();
    m.assert();
    assert_eq!(d.summary.number, 7);
    assert_eq!(d.summary.checks, ChecksState::Failure);
    assert_eq!(d.summary.review_decision, ReviewDecision::ChangesRequested);
    assert_eq!(d.body, "Fixes **login**.");
    assert_eq!(d.head_sha, "abc123");
    assert_eq!(d.head_repo, Some(("ada".into(), "r".into())));
    assert!(d.cross_repository);
    assert_eq!(d.commit_count, 2);
    assert_eq!(d.commits[0].author, "ada");
    assert_eq!(d.commits[1].author, "Someone", "no GitHub user: Git name");
    let statuses: Vec<_> = d.check_runs.iter().map(|c| c.status).collect();
    assert_eq!(
        statuses,
        [
            CheckStatus::Success,
            CheckStatus::Failure,
            CheckStatus::Neutral,
            CheckStatus::Pending,
            CheckStatus::Pending
        ]
    );
    assert_eq!(d.check_runs[4].name, "ci/legacy");
    assert_eq!(d.check_runs[0].url.as_deref(), Some("https://x/1"));
    // Comments and submitted reviews, oldest first; the pending review is left out.
    assert_eq!(d.timeline.len(), 3);
    assert!(matches!(&d.timeline[0], TimelineItem::Comment { author, .. } if author == "ghost"));
    assert!(matches!(
        &d.timeline[1],
        TimelineItem::Review { state: ReviewState::ChangesRequested, body, .. } if body == "Please fix"
    ));
    assert!(matches!(&d.timeline[2], TimelineItem::Comment { author, .. } if author == "carol"));
    assert_eq!(d.threads.len(), 2);
    assert_eq!(d.threads[0].line, Some(12));
    assert_eq!(d.threads[0].side, DiffSide::Right);
    assert_eq!(d.threads[0].comments[1].id, 992);
    assert!(d.threads[1].outdated && d.threads[1].resolved);
    assert_eq!(d.threads[1].line, None);
    assert_eq!(d.threads[1].side, DiffSide::Left);
    assert_eq!(d.mergeable, Mergeable::Conflicting);
    assert_eq!(d.merge_state, "DIRTY");
    assert_eq!(
        d.allowed_methods,
        [MergeMethod::Squash, MergeMethod::Rebase]
    );
    assert_eq!(d.viewer, "bob");
    assert!(!d.viewer_is_author);
    assert!(d.viewer_can_write);
    assert_eq!(d.repo_labels.len(), 2);
    assert_eq!(
        d.repo_labels[1].description, None,
        "empty description is none"
    );
}

#[test]
fn a_missing_pull_request_is_an_error() {
    let mut server = mockito::Server::new();
    server
        .mock("POST", "/graphql")
        .with_body(r#"{"data":{"viewer":{"login":"bob"},"repository":{"pullRequest":null}}}"#)
        .create();
    assert!(matches!(
        client(&server).pull_detail("t", "o", "r", 9),
        Err(GithubError::Decode(_))
    ));
}

#[test]
fn pull_files_follow_pagination_and_keep_missing_patches() {
    let mut server = mockito::Server::new();
    let next = format!(
        "<{}/repos/o/r/pulls/7/files?per_page=100&page=2>; rel=\"next\"",
        server.url()
    );
    let file = |i: usize| {
        json!({ "filename": format!("f{i}.rs"), "status": "modified", "additions": 1,
                "deletions": 2, "patch": "@@ -1 +1 @@\n-a\n+b" })
    };
    let page1: Vec<_> = (0..100).map(file).collect();
    let m1 = server
        .mock("GET", "/repos/o/r/pulls/7/files")
        .match_query(Matcher::UrlEncoded("per_page".into(), "100".into()))
        .with_header("link", &next)
        .with_body(serde_json::to_string(&page1).unwrap())
        .create();
    let m2 = server
        .mock("GET", "/repos/o/r/pulls/7/files")
        .match_query(Matcher::UrlEncoded("page".into(), "2".into()))
        .with_body(
            json!([
                { "filename": "logo.png", "status": "added", "additions": 0, "deletions": 0 },
                { "filename": "new.rs", "previous_filename": "old.rs", "status": "renamed",
                  "additions": 0, "deletions": 0, "patch": "" }
            ])
            .to_string(),
        )
        .create();
    let files = client(&server).pull_files("t", "o", "r", 7).unwrap();
    m1.assert();
    m2.assert();
    assert_eq!(files.len(), 102);
    assert_eq!(files[0].patch.as_deref(), Some("@@ -1 +1 @@\n-a\n+b"));
    assert_eq!(files[100].patch, None, "binary or too large: no patch");
    assert_eq!(files[101].previous_path.as_deref(), Some("old.rs"));
    assert_eq!((files[0].additions, files[0].deletions), (1, 2));
}
