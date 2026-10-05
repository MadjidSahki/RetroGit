#![allow(clippy::unwrap_used)]
#![recursion_limit = "256"]

use github::{
    Client, DiffSide, GithubError, LineComment, ReviewState, diff_lists, suggestion_block,
    suggestions,
};
use mockito::Matcher;
use serde_json::json;

fn client(server: &mockito::Server) -> Client {
    Client::with_bases(&server.url(), &server.url())
}

#[test]
fn suggestions_are_found_in_comments() {
    let body = "Better:\n```suggestion\nlet b = 3;\nlet c = 4;\n```\nand\n```suggestion\n```\n```rust\nnot one\n```";
    assert_eq!(suggestions(body), ["let b = 3;\nlet c = 4;\n", ""]);
    assert_eq!(suggestions("```suggestion\r\nx\r\n```"), ["x\n"]);
    assert!(suggestions("no code here").is_empty());
    assert_eq!(
        suggestion_block(&["  a\n".to_string(), "b".to_string()]),
        "```suggestion\n  a\nb\n```\n"
    );
}

#[test]
fn list_changes_are_computed_without_case_noise() {
    let (add, remove) = diff_lists(
        &["ada".to_string(), "Bob".to_string()],
        &["bob".to_string(), "carol".to_string()],
    );
    assert_eq!(add, ["carol"]);
    assert_eq!(remove, ["ada"]);
}

#[test]
fn title_and_description_are_updated() {
    let mut server = mockito::Server::new();
    let m = server
        .mock("PATCH", "/repos/o/r/pulls/7")
        .match_body(Matcher::Json(json!({ "title": "New", "body": "Text" })))
        .with_body("{}")
        .create();
    client(&server)
        .update_pull("t", "o", "r", 7, "New", "Text")
        .unwrap();
    m.assert();
}

#[test]
fn reviewers_and_assignees_are_added_and_removed() {
    let mut server = mockito::Server::new();
    let add_r = server
        .mock("POST", "/repos/o/r/pulls/7/requested_reviewers")
        .match_body(Matcher::Json(json!({ "reviewers": ["carol"] })))
        .with_status(201)
        .with_body("{}")
        .create();
    let rm_r = server
        .mock("DELETE", "/repos/o/r/pulls/7/requested_reviewers")
        .match_body(Matcher::Json(json!({ "reviewers": ["ada"] })))
        .with_body("{}")
        .create();
    let add_a = server
        .mock("POST", "/repos/o/r/issues/7/assignees")
        .match_body(Matcher::Json(json!({ "assignees": ["bob"] })))
        .with_status(201)
        .with_body("{}")
        .create();
    let c = client(&server);
    c.set_reviewers("t", "o", "r", 7, &["carol".into()], &["ada".into()])
        .unwrap();
    c.set_assignees("t", "o", "r", 7, &["bob".into()], &[])
        .unwrap();
    add_r.assert();
    rm_r.assert();
    add_a.assert();
    // Nothing to change: no request at all.
    c.set_assignees("t", "o", "r", 7, &[], &[]).unwrap();
}

#[test]
fn a_removal_failing_after_an_addition_says_which_half() {
    let mut server = mockito::Server::new();
    for (path, key) in [
        ("/repos/o/r/pulls/7/requested_reviewers", "reviewers"),
        ("/repos/o/r/issues/7/assignees", "assignees"),
    ] {
        server
            .mock("POST", path)
            .match_body(Matcher::Json(json!({ key: ["carol"] })))
            .with_status(201)
            .with_body("{}")
            .create();
        server
            .mock("POST", path)
            .match_body(Matcher::Json(json!({ key: ["zed"] })))
            .with_status(422)
            .with_body(r#"{"message":"Validation Failed"}"#)
            .create();
        server
            .mock("DELETE", path)
            .with_status(422)
            .with_body(r#"{"message":"Validation Failed"}"#)
            .create();
    }
    let c = client(&server);
    let both = [
        c.set_reviewers(
            "t",
            "o",
            "r",
            7,
            &["carol".into()],
            &["ada".into(), "dan".into()],
        ),
        c.set_assignees(
            "t",
            "o",
            "r",
            7,
            &["carol".into()],
            &["ada".into(), "dan".into()],
        ),
    ];
    for r in both {
        match r {
            Err(GithubError::PeopleHalf { not_removed, .. }) => {
                assert_eq!(not_removed, ["ada", "dan"]);
            }
            other => panic!("{other:?}"),
        }
    }
    // The addition fails: nothing done, the usual error.
    let first = c.set_reviewers("t", "o", "r", 7, &["zed".into()], &["ada".into()]);
    assert!(
        matches!(first, Err(GithubError::Rejected { status: 422, .. })),
        "{first:?}"
    );
    // Only a removal, refused: nothing done either.
    let only = c.set_assignees("t", "o", "r", 7, &[], &["ada".into()]);
    assert!(
        matches!(only, Err(GithubError::Rejected { status: 422, .. })),
        "{only:?}"
    );
}

#[test]
fn drafts_are_marked_ready_and_back() {
    let mut server = mockito::Server::new();
    let ready = server
        .mock("POST", "/graphql")
        .match_body(Matcher::AllOf(vec![
            Matcher::Regex("markPullRequestReadyForReview".into()),
            Matcher::PartialJson(json!({ "variables": { "id": "PR_1" } })),
        ]))
        .with_body(
            r#"{"data":{"markPullRequestReadyForReview":{"pullRequest":{"isDraft":false}}}}"#,
        )
        .create();
    let draft = server
        .mock("POST", "/graphql")
        .match_body(Matcher::Regex("convertPullRequestToDraft".into()))
        .with_body(r#"{"data":{"convertPullRequestToDraft":{"pullRequest":{"isDraft":true}}}}"#)
        .create();
    let c = client(&server);
    c.set_draft("t", "PR_1", false).unwrap();
    c.set_draft("t", "PR_1", true).unwrap();
    ready.assert();
    draft.assert();
}

#[test]
fn range_comments_send_their_first_line() {
    let mut server = mockito::Server::new();
    let m = server
        .mock("POST", "/repos/o/r/pulls/7/comments")
        .match_body(Matcher::Json(json!({
            "commit_id": "abc", "path": "a.rs", "line": 14, "side": "RIGHT",
            "start_line": 12, "start_side": "RIGHT", "body": "these three"
        })))
        .with_status(201)
        .with_body("{}")
        .create();
    client(&server)
        .add_line_comment(
            "t",
            "o",
            "r",
            7,
            "abc",
            &LineComment {
                path: "a.rs".into(),
                line: 14,
                side: DiffSide::Right,
                start: Some((12, DiffSide::Right)),
                body: "these three".into(),
            },
        )
        .unwrap();
    m.assert();
}

#[test]
fn assignable_users_are_listed() {
    let mut server = mockito::Server::new();
    server
        .mock("POST", "/graphql")
        .match_body(Matcher::PartialJson(json!({ "variables": { "owner": "o", "name": "r", "q": "ca" } })))
        .with_body(r#"{"data":{"repository":{"assignableUsers":{"nodes":[{"login":"carol","name":"C"},{"login":"cathy","name":null}]}}}}"#)
        .create();
    assert_eq!(
        client(&server)
            .assignable_users("t", "o", "r", "ca")
            .unwrap(),
        ["carol", "cathy"]
    );
}

#[test]
fn the_detail_gives_reviewers_assignees_and_ranges() {
    let mut server = mockito::Server::new();
    let body = json!({ "data": {
        "viewer": { "login": "ada" },
        "repository": {
            "mergeCommitAllowed": true, "squashMergeAllowed": false, "rebaseMergeAllowed": false,
            "viewerPermission": "WRITE", "labels": { "nodes": [] },
            "pullRequest": {
                "id": "PR_9", "viewerCanUpdate": true,
                "reviewRequests": { "nodes": [
                    { "requestedReviewer": { "__typename": "User", "login": "bob" } },
                    { "requestedReviewer": { "__typename": "Team", "slug": "core", "organization": { "login": "o" } } }
                ] },
                "latestReviews": { "nodes": [
                    { "author": { "login": "carol" }, "state": "APPROVED" },
                    { "author": { "login": "ada" }, "state": "COMMENTED" }
                ] },
                "assignees": { "nodes": [ { "login": "ada" } ] },
                "number": 9, "title": "T", "url": "u", "isDraft": true, "state": "OPEN",
                "updatedAt": "", "body": "", "author": { "login": "ada" },
                "headRefName": "x", "baseRefName": "main", "headRefOid": "abc",
                "isCrossRepository": false, "headRepository": null,
                "reviewDecision": null, "mergeable": "MERGEABLE", "mergeStateStatus": "DRAFT",
                "viewerDidAuthor": true, "labels": { "nodes": [] },
                "commits": { "totalCount": 0, "nodes": [] }, "lastCommit": { "nodes": [] },
                "comments": { "nodes": [] }, "reviews": { "nodes": [] },
                "reviewThreads": { "nodes": [ {
                    "id": "T1", "viewerCanResolve": true, "viewerCanUnresolve": false,
                    "isResolved": false, "isOutdated": false, "path": "a.rs",
                    "line": 14, "originalLine": 14, "diffSide": "RIGHT",
                    "startLine": 12, "startDiffSide": "RIGHT",
                    "comments": { "nodes": [] } } ] }
            }
        }
    } });
    let mut triage = body.clone();
    triage["data"]["repository"]["viewerPermission"] = json!("TRIAGE");
    let mut read = body.clone();
    read["data"]["repository"]["viewerPermission"] = json!("READ");
    server
        .mock("POST", "/graphql")
        .with_body(body.to_string())
        .create();
    let d = client(&server).pull_detail("t", "o", "r", 9).unwrap();
    assert!(
        d.viewer_can_write && d.viewer_can_triage,
        "write includes triage"
    );
    server.reset();
    server
        .mock("POST", "/graphql")
        .with_body(triage.to_string())
        .create();
    let t = client(&server).pull_detail("t", "o", "r", 9).unwrap();
    assert!(t.viewer_can_triage && !t.viewer_can_write, "triage only");
    server.reset();
    server
        .mock("POST", "/graphql")
        .with_body(read.to_string())
        .create();
    let r = client(&server).pull_detail("t", "o", "r", 9).unwrap();
    assert!(!r.viewer_can_triage && !r.viewer_can_write, "read only");
    assert_eq!(d.id, "PR_9");
    assert!(d.viewer_can_update);
    let reviewers: Vec<(String, Option<ReviewState>)> = d
        .reviewers
        .iter()
        .map(|r| (r.login.clone(), r.state))
        .collect();
    assert_eq!(
        reviewers,
        [
            ("bob".to_string(), None),
            ("carol".to_string(), Some(ReviewState::Approved))
        ],
        "requested people, then those who reviewed, not the author (ada)"
    );
    assert_eq!(d.team_reviewers, ["o/core"]);
    assert_eq!(d.assignees, ["ada"]);
    assert_eq!(d.threads[0].start_line, Some(12));
    assert_eq!(d.threads[0].start_side, Some(DiffSide::Right));
}

#[test]
fn suggestion_fences_follow_markdown() {
    // A fence with an info string cannot close the block: it is part of the code (a bare
    // fence does close it, as in Markdown).
    let body = "```suggestion\nSee:\n```rust\nlet x = 1;\n```\nend\n";
    assert_eq!(suggestions(body), ["See:\n```rust\nlet x = 1;\n"]);
    // Longer fences (for code that contains fences) and tildes.
    assert_eq!(suggestions("````suggestion\n```\n````"), ["```\n"]);
    assert_eq!(suggestions("~~~suggestion  \nx\n~~~"), ["x\n"]);
    // An unclosed block runs to the end, like Markdown.
    assert_eq!(suggestions("```suggestion\nx\ny"), ["x\ny\n"]);
}
