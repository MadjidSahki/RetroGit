#![allow(clippy::unwrap_used)]

use github::{
    Client, DiffSide, GithubError, LineComment, Merge, MergeMethod, NewPull, Review, ReviewEvent,
};
use mockito::Matcher;
use serde_json::json;

fn client(server: &mockito::Server) -> Client {
    Client::with_bases(&server.url(), &server.url())
}

#[test]
fn create_pull_then_sets_its_labels() {
    let mut server = mockito::Server::new();
    let create = server
        .mock("POST", "/repos/o/r/pulls")
        .match_header("authorization", "Bearer t")
        .match_body(Matcher::Json(json!({
            "title": "Fix", "body": "Why", "head": "feat/x", "base": "main", "draft": true
        })))
        .with_status(201)
        .with_body(r#"{"number": 12, "html_url": "https://github.com/o/r/pull/12"}"#)
        .create();
    let labels = server
        .mock("PUT", "/repos/o/r/issues/12/labels")
        .match_body(Matcher::Json(
            json!({ "labels": ["bug", "good first issue"] }),
        ))
        .with_body("[]")
        .create();
    let n = client(&server)
        .create_pull(
            "t",
            "o",
            "r",
            &NewPull {
                title: "Fix".into(),
                body: "Why".into(),
                head: "feat/x".into(),
                base: "main".into(),
                draft: true,
                labels: vec!["bug".into(), "good first issue".into()],
            },
        )
        .unwrap();
    assert_eq!(n, 12);
    create.assert();
    labels.assert();
}

#[test]
fn existing_pull_is_reported_and_can_be_found() {
    let mut server = mockito::Server::new();
    server
        .mock("POST", "/repos/o/r/pulls")
        .with_status(422)
        .with_body(
            json!({ "message": "Validation Failed", "errors": [
                { "resource": "PullRequest", "code": "custom",
                  "message": "A pull request already exists for o:feat/x." }
            ] })
            .to_string(),
        )
        .create();
    let pull = NewPull {
        title: "t".into(),
        body: String::new(),
        head: "feat/x".into(),
        base: "main".into(),
        draft: false,
        labels: vec![],
    };
    assert_eq!(
        client(&server).create_pull("t", "o", "r", &pull).err(),
        Some(GithubError::Rejected {
            status: 422,
            message: "Validation Failed: A pull request already exists for o:feat/x.".into()
        })
    );
    let find = server
        .mock("GET", "/repos/o/r/pulls")
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded("state".into(), "open".into()),
            Matcher::UrlEncoded("head".into(), "o:feat/x".into()),
        ]))
        .with_body(r#"[{"number": 4}]"#)
        .create();
    assert_eq!(
        client(&server)
            .find_open_pull("t", "o", "r", "o", "feat/x")
            .unwrap(),
        Some(4)
    );
    find.assert();
}

#[test]
fn review_is_pinned_to_the_commit_with_line_comments() {
    let mut server = mockito::Server::new();
    let m = server
        .mock("POST", "/repos/o/r/pulls/7/reviews")
        .match_body(Matcher::Json(json!({
            "commit_id": "abc123",
            "event": "REQUEST_CHANGES",
            "body": "See comments",
            "comments": [
                { "path": "src/a.rs", "line": 12, "side": "RIGHT", "body": "Why?" },
                { "path": "src/b.rs", "line": 3, "side": "LEFT", "body": "Keep this" }
            ]
        })))
        .with_body("{}")
        .create();
    client(&server)
        .submit_review(
            "t",
            "o",
            "r",
            7,
            &Review {
                commit_id: "abc123".into(),
                event: ReviewEvent::RequestChanges,
                body: "See comments".into(),
                comments: vec![
                    LineComment {
                        path: "src/a.rs".into(),
                        line: 12,
                        side: DiffSide::Right,
                        body: "Why?".into(),
                    },
                    LineComment {
                        path: "src/b.rs".into(),
                        line: 3,
                        side: DiffSide::Left,
                        body: "Keep this".into(),
                    },
                ],
            },
        )
        .unwrap();
    m.assert();
}

#[test]
fn an_approval_without_text_sends_no_body() {
    let mut server = mockito::Server::new();
    let m = server
        .mock("POST", "/repos/o/r/pulls/7/reviews")
        .match_body(Matcher::Json(json!({
            "commit_id": "abc", "event": "APPROVE", "comments": []
        })))
        .with_body("{}")
        .create();
    client(&server)
        .submit_review(
            "t",
            "o",
            "r",
            7,
            &Review {
                commit_id: "abc".into(),
                event: ReviewEvent::Approve,
                body: "  ".into(),
                comments: vec![],
            },
        )
        .unwrap();
    m.assert();
}

#[test]
fn replies_and_conversation_comments() {
    let mut server = mockito::Server::new();
    let reply = server
        .mock("POST", "/repos/o/r/pulls/7/comments/991/replies")
        .match_body(Matcher::Json(json!({ "body": "Done" })))
        .with_status(201)
        .with_body("{}")
        .create();
    let comment = server
        .mock("POST", "/repos/o/r/issues/7/comments")
        .match_body(Matcher::Json(json!({ "body": "Thanks!" })))
        .with_status(201)
        .with_body("{}")
        .create();
    let c = client(&server);
    c.reply_to_thread("t", "o", "r", 7, 991, "Done").unwrap();
    c.add_issue_comment("t", "o", "r", 7, "Thanks!").unwrap();
    reply.assert();
    comment.assert();
}

#[test]
fn merge_sends_the_expected_head() {
    let mut server = mockito::Server::new();
    let squash = server
        .mock("PUT", "/repos/o/r/pulls/7/merge")
        .match_body(Matcher::Json(json!({
            "merge_method": "squash", "sha": "abc",
            "commit_title": "Fix login (#7)", "commit_message": "details"
        })))
        .with_body(r#"{"merged": true}"#)
        .create();
    let c = client(&server);
    c.merge_pull(
        "t",
        "o",
        "r",
        7,
        &Merge {
            method: MergeMethod::Squash,
            title: "Fix login (#7)".into(),
            message: "details".into(),
            sha: "abc".into(),
        },
    )
    .unwrap();
    squash.assert();
}

#[test]
fn merge_refused_is_typed() {
    let mut server = mockito::Server::new();
    server
        .mock("PUT", "/repos/o/r/pulls/7/merge")
        .match_body(Matcher::Json(
            json!({ "merge_method": "rebase", "sha": "old" }),
        ))
        .with_status(409)
        .with_body(r#"{"message": "Head branch was modified. Review and try the merge again."}"#)
        .create();
    server
        .mock("PUT", "/repos/o/r/pulls/8/merge")
        .with_status(405)
        .with_body(r#"{"message": "Pull Request is not mergeable"}"#)
        .create();
    let c = client(&server);
    let merge = |sha: &str| Merge {
        method: MergeMethod::Rebase,
        title: "ignored".into(),
        message: "ignored".into(),
        sha: sha.into(),
    };
    assert_eq!(
        c.merge_pull("t", "o", "r", 7, &merge("old")).err(),
        Some(GithubError::Rejected {
            status: 409,
            message: "Head branch was modified. Review and try the merge again.".into()
        })
    );
    assert!(matches!(
        c.merge_pull("t", "o", "r", 8, &merge("x")),
        Err(GithubError::Rejected { status: 405, .. })
    ));
}

#[test]
fn delete_branch_encodes_the_name() {
    let mut server = mockito::Server::new();
    let m = server
        .mock("DELETE", "/repos/o/r/git/refs/heads/feat/a%231")
        .with_status(204)
        .create();
    client(&server)
        .delete_branch("t", "o", "r", "feat/a#1")
        .unwrap();
    m.assert();
}

#[test]
fn write_errors_keep_auth_sso_and_restriction_types() {
    let mut server = mockito::Server::new();
    server
        .mock("POST", "/repos/o/r/issues/1/comments")
        .with_status(401)
        .create();
    server
        .mock("POST", "/repos/o/r/issues/2/comments")
        .with_status(403)
        .with_header(
            "x-github-sso",
            "required; url=https://github.com/orgs/o/sso",
        )
        .create();
    server
        .mock("POST", "/repos/o/r/issues/3/comments")
        .with_status(403)
        .with_body(
            r#"{"message":"the `o` organization has enabled OAuth App access restrictions"}"#,
        )
        .create();
    server
        .mock("POST", "/repos/o/r/issues/4/comments")
        .with_status(500)
        .create();
    let c = client(&server);
    let post = |n| c.add_issue_comment("t", "o", "r", n, "x").err();
    assert_eq!(post(1), Some(GithubError::Unauthorized));
    assert_eq!(
        post(2),
        Some(GithubError::SsoRequired {
            url: "https://github.com/orgs/o/sso".into()
        })
    );
    assert_eq!(
        post(3),
        Some(GithubError::OAuthRestricted {
            org: Some("o".into())
        })
    );
    assert_eq!(post(4), Some(GithubError::Http(500)));
}

#[test]
fn repo_meta_reads_default_branch_and_all_labels() {
    let mut server = mockito::Server::new();
    server
        .mock("GET", "/repos/o/r")
        .with_body(r#"{"default_branch": "develop"}"#)
        .create();
    let next = format!(
        "<{}/repos/o/r/labels?per_page=100&page=2>; rel=\"next\"",
        server.url()
    );
    server
        .mock("GET", "/repos/o/r/labels")
        .match_query(Matcher::UrlEncoded("per_page".into(), "100".into()))
        .with_header("link", &next)
        .with_body(r#"[{"name":"bug","color":"d73a4a","description":null}]"#)
        .create();
    server
        .mock("GET", "/repos/o/r/labels")
        .match_query(Matcher::UrlEncoded("page".into(), "2".into()))
        .with_body(r#"[{"name":"docs","color":"0075ca","description":"Docs"}]"#)
        .create();
    let meta = client(&server).repo_meta("t", "o", "r").unwrap();
    assert_eq!(meta.default_branch, "develop");
    let names: Vec<_> = meta.labels.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(names, ["bug", "docs"]);
}

#[test]
fn a_single_line_comment_is_posted_at_once() {
    let mut server = mockito::Server::new();
    let m = server
        .mock("POST", "/repos/o/r/pulls/7/comments")
        .match_body(Matcher::Json(json!({
            "commit_id": "abc", "path": "src/a.rs", "line": 12, "side": "LEFT", "body": "Why?"
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
                path: "src/a.rs".into(),
                line: 12,
                side: DiffSide::Left,
                body: "Why?".into(),
            },
        )
        .unwrap();
    m.assert();
}

#[test]
fn threads_are_resolved_and_unresolved_with_graphql() {
    let mut server = mockito::Server::new();
    let resolve = server
        .mock("POST", "/graphql")
        .match_body(Matcher::AllOf(vec![
            Matcher::Regex("resolveReviewThread\\(input: \\{ threadId: \\$id \\}\\)".into()),
            Matcher::PartialJson(json!({ "variables": { "id": "PRRT_1" } })),
        ]))
        .with_body(r#"{"data":{"resolveReviewThread":{"thread":{"isResolved":true}}}}"#)
        .create();
    let c = client(&server);
    c.set_thread_resolved("t", "PRRT_1", true).unwrap();
    resolve.assert();
    let unresolve = server
        .mock("POST", "/graphql")
        .match_body(Matcher::Regex("unresolveReviewThread".into()))
        .with_body(r#"{"data":{"unresolveReviewThread":{"thread":{"isResolved":false}}}}"#)
        .create();
    c.set_thread_resolved("t", "PRRT_1", false).unwrap();
    unresolve.assert();
}
