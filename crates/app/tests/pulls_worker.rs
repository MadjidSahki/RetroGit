#![allow(clippy::unwrap_used)]
//! Pull request commands of the worker, against a mock GitHub.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use github::{
    Client, MemoryAccounts, Merge, MergeMethod, NewPull, PrFilter, Review, ReviewEvent,
    TokenProvider,
};
use mockito::Matcher;
use retrogit::protocol::{Command, Event, Op, Severity};
use retrogit::worker::{WorkerDeps, WorkerHandle, spawn};
use serde_json::json;

fn slug() -> (String, String) {
    ("o".into(), "r".into())
}

fn start(server: &mockito::Server, tokens: TokenProvider) -> WorkerHandle {
    let deps = WorkerDeps {
        client: Client::with_bases(&server.url(), &server.url()),
        store: Arc::new(MemoryAccounts::default()),
        client_id: String::new(),
        commit_backend: gitcore::CommitBackend::Git2,
        tokens,
        known_accounts: Vec::new(),
        repo_accounts: Default::default(),
    };
    spawn(deps, || {})
}

fn until(w: &WorkerHandle, done: impl Fn(&Event) -> bool) -> Vec<Event> {
    let deadline = Instant::now() + Duration::from_secs(15);
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

/// Worker signed in with the PAT `ghp_pat` (login "ada", member of organization "o", so
/// it is the account of every `o/*` repository).
fn signed_in(server: &mut mockito::Server, tokens: TokenProvider) -> WorkerHandle {
    server
        .mock("GET", "/user")
        .with_body(r#"{"login":"ada","name":null}"#)
        .create();
    server
        .mock("GET", "/user/orgs")
        .match_query(Matcher::Any)
        .with_body(r#"[{"login":"o"}]"#)
        .create();
    let w = start(server, tokens);
    w.send(Command::SavePat("ghp_pat".into()));
    until(&w, |e| matches!(e, Event::SignedIn(_)));
    w
}

fn detail_body(number: u64, head_sha: &str) -> String {
    json!({ "data": {
        "viewer": { "login": "ada" },
        "repository": {
            "mergeCommitAllowed": true, "squashMergeAllowed": true, "rebaseMergeAllowed": false,
            "viewerPermission": "WRITE",
            "labels": { "nodes": [] },
            "pullRequest": {
                "number": number, "title": "Fix", "url": "u", "isDraft": false, "state": "OPEN",
                "updatedAt": "2026-10-01T09:00:00Z", "body": "", "author": { "login": "bob" },
                "headRefName": "feat/x", "baseRefName": "main", "headRefOid": head_sha,
                "isCrossRepository": false,
                "headRepository": { "name": "r", "owner": { "login": "o" } },
                "reviewDecision": null, "mergeable": "MERGEABLE", "mergeStateStatus": "CLEAN",
                "viewerDidAuthor": false,
                "labels": { "nodes": [] }, "commits": { "totalCount": 0, "nodes": [] },
                "lastCommit": { "nodes": [] }, "comments": { "nodes": [] },
                "reviews": { "nodes": [] }, "reviewThreads": { "nodes": [] }
            }
        }
    } })
    .to_string()
}

fn mock_detail(server: &mut mockito::Server, number: u64) -> (mockito::Mock, mockito::Mock) {
    let detail = server
        .mock("POST", "/graphql")
        .match_body(Matcher::PartialJson(
            json!({ "variables": { "number": number } }),
        ))
        .with_body(detail_body(number, "abc"))
        .expect_at_least(1)
        .create();
    let files = server
        .mock("GET", format!("/repos/o/r/pulls/{number}/files").as_str())
        .match_query(Matcher::Any)
        .with_body(r#"[{"filename":"a.rs","status":"modified","additions":1,"deletions":0,"patch":"@@ -1 +1,2 @@\n a\n+b"}]"#)
        .expect_at_least(1)
        .create();
    (detail, files)
}

#[test]
fn list_and_detail_are_loaded_for_the_repository() {
    let mut server = mockito::Server::new();
    let w = signed_in(&mut server, TokenProvider::without_gh());
    server
        .mock("POST", "/graphql")
        .match_body(Matcher::PartialJson(json!({
            "variables": { "q": "repo:o/r is:pr is:open sort:updated-desc" }
        })))
        .with_body(r#"{"data":{"search":{"issueCount":41,"nodes":[{"number":7,"title":"Fix"}]}}}"#)
        .create();
    w.send(Command::LoadPulls {
        slug: slug(),
        filter: PrFilter::Open,
    });
    let evs = until(&w, |e| matches!(e, Event::PullsLoaded { .. }));
    assert!(matches!(
        evs.last(),
        Some(Event::PullsLoaded { slug: s, filter: PrFilter::Open, list, total: 41 }) if *s == slug() && list[0].number == 7
    ));
    let (_d, _f) = mock_detail(&mut server, 7);
    w.send(Command::LoadPull {
        slug: slug(),
        number: 7,
    });
    let evs = until(&w, |e| matches!(e, Event::PullFilesLoaded { .. }));
    assert!(
        evs.iter()
            .any(|e| matches!(e, Event::PullLoaded { detail, .. } if detail.head_sha == "abc"))
    );
    assert!(
        matches!(evs.last(), Some(Event::PullFilesLoaded { number: 7, files, .. }) if files[0].path == "a.rs")
    );
}

#[test]
fn an_organization_restricting_retrogit_is_read_with_the_gh_token() {
    let mut server = mockito::Server::new();
    let tokens = TokenProvider::new(Arc::new(|login: &str| {
        assert_eq!(
            login, "ada",
            "gh is asked for the account of the repository"
        );
        Some("gho_cli".to_string())
    }));
    let w = signed_in(&mut server, tokens);
    let refused = server
        .mock("POST", "/graphql")
        .match_header("authorization", "Bearer ghp_pat")
        .with_body(r#"{"errors":[{"type":"FORBIDDEN","message":"the `o` organization has enabled OAuth App access restrictions"}]}"#)
        .expect(1)
        .create();
    let allowed = server
        .mock("POST", "/graphql")
        .match_header("authorization", "Bearer gho_cli")
        .with_body(r#"{"data":{"search":{"nodes":[]}}}"#)
        .expect(2)
        .create();
    for _ in 0..2 {
        w.send(Command::LoadPulls {
            slug: slug(),
            filter: PrFilter::Open,
        });
        until(&w, |e| matches!(e, Event::PullsLoaded { .. }));
    }
    refused.assert();
    allowed.assert();
}

#[test]
fn a_repository_looking_missing_is_read_with_the_gh_token() {
    // What github.com really answers for an organization restricting the OAuth App.
    let mut server = mockito::Server::new();
    let tokens = TokenProvider::new(Arc::new(|_: &str| Some("gho_cli".to_string())));
    let w = signed_in(&mut server, tokens);
    server
        .mock("POST", "/graphql")
        .match_header("authorization", "Bearer ghp_pat")
        .with_body(r#"{"data":{"repository":null,"search":{"nodes":[]}},"errors":[{"type":"NOT_FOUND","path":["repository"],"message":"Could not resolve to a Repository with the name 'o/r'."}]}"#)
        .create();
    server
        .mock("POST", "/graphql")
        .match_header("authorization", "Bearer gho_cli")
        .with_body(r#"{"data":{"repository":{"id":"R_1"},"search":{"nodes":[{"number":3,"title":"Fix"}]}}}"#)
        .create();
    w.send(Command::LoadPulls {
        slug: slug(),
        filter: PrFilter::Open,
    });
    let evs = until(&w, |e| {
        matches!(e, Event::PullsLoaded { .. } | Event::Error { .. })
    });
    assert!(
        matches!(evs.last(), Some(Event::PullsLoaded { list, .. }) if list[0].number == 3),
        "{:?}",
        evs.last()
    );
}

#[test]
fn only_a_missing_repository_suggests_the_restriction() {
    let mut server = mockito::Server::new();
    let w = signed_in(&mut server, TokenProvider::without_gh());
    server
        .mock("POST", "/graphql")
        .match_body(Matcher::PartialJson(json!({ "variables": { "q": "repo:o/r is:pr is:open sort:updated-desc" } })))
        .with_body(r#"{"data":{"repository":null},"errors":[{"type":"NOT_FOUND","message":"Could not resolve to a Repository with the name 'o/r'."}]}"#)
        .create();
    server
        .mock("POST", "/graphql")
        .match_body(Matcher::PartialJson(json!({ "variables": { "number": 99 } })))
        .with_body(r#"{"data":{"repository":{"pullRequest":null}},"errors":[{"type":"NOT_FOUND","message":"Could not resolve to a PullRequest with the number of 99."}]}"#)
        .create();
    w.send(Command::LoadPulls {
        slug: slug(),
        filter: PrFilter::Open,
    });
    let evs = until(&w, |e| matches!(e, Event::Error { .. }));
    let Some(Event::Error { error, .. }) = evs.last() else {
        unreachable!()
    };
    assert_eq!(error.message, retrogit::strings::ERR_PULLS_NOT_FOUND);
    assert!(error.link.is_some());
    w.send(Command::LoadPull {
        slug: slug(),
        number: 99,
    });
    let evs = until(&w, |e| matches!(e, Event::Error { .. }));
    let Some(Event::Error { during, error }) = evs.last() else {
        unreachable!()
    };
    assert_eq!(*during, Op::PullDetail(99), "the detail pane says why");
    assert_eq!(
        error.message,
        "Could not resolve to a PullRequest with the number of 99."
    );
    assert!(error.link.is_none(), "not an access problem");
}

#[test]
fn without_gh_the_restriction_is_explained_with_a_link() {
    let mut server = mockito::Server::new();
    let w = signed_in(&mut server, TokenProvider::without_gh());
    server
        .mock("POST", "/graphql")
        .with_body(r#"{"errors":[{"type":"FORBIDDEN","message":"the `o` organization has enabled OAuth App access restrictions"}]}"#)
        .create();
    w.send(Command::LoadPulls {
        slug: slug(),
        filter: PrFilter::Open,
    });
    let evs = until(&w, |e| matches!(e, Event::Error { .. }));
    let Some(Event::Error { during, error }) = evs.last() else {
        unreachable!()
    };
    assert_eq!(*during, Op::Pulls);
    assert!(
        error.message.contains("organization o"),
        "{}",
        error.message
    );
    assert!(error.message.contains("gh auth login"));
    assert!(error.link.is_some());
}

#[test]
fn a_gh_too_old_to_choose_the_account_is_asked_to_be_updated() {
    use retrogit::strings as s;
    for body in [
        r#"{"errors":[{"type":"FORBIDDEN","message":"the `o` organization has enabled OAuth App access restrictions"}]}"#,
        r#"{"data":{"repository":null},"errors":[{"type":"NOT_FOUND","path":["repository"],"message":"Could not resolve to a Repository with the name 'o/r'."}]}"#,
    ] {
        let mut server = mockito::Server::new();
        let tokens = TokenProvider::from_gh(Arc::new(|_: &str| github::GhToken::TooOld));
        let w = signed_in(&mut server, tokens);
        server.mock("POST", "/graphql").with_body(body).create();
        w.send(Command::LoadPulls {
            slug: slug(),
            filter: PrFilter::Open,
        });
        let evs = until(&w, |e| matches!(e, Event::Error { .. }));
        let Some(Event::Error { error, .. }) = evs.last() else {
            unreachable!()
        };
        assert!(error.message.contains(s::GH_TOO_OLD), "{}", error.message);
        assert!(!error.message.contains("Install"), "{}", error.message);
        assert!(error.link.is_some());
    }
}

#[test]
fn a_review_is_sent_then_the_pull_request_reloaded() {
    let mut server = mockito::Server::new();
    let w = signed_in(&mut server, TokenProvider::without_gh());
    let review = server
        .mock("POST", "/repos/o/r/pulls/7/reviews")
        .match_body(Matcher::PartialJson(
            json!({ "commit_id": "abc", "event": "APPROVE" }),
        ))
        .with_body("{}")
        .create();
    let (_d, _f) = mock_detail(&mut server, 7);
    w.send(Command::SubmitReview {
        slug: slug(),
        number: 7,
        review: Review {
            commit_id: "abc".into(),
            event: ReviewEvent::Approve,
            body: String::new(),
            comments: vec![],
        },
    });
    let evs = until(&w, |e| matches!(e, Event::PullFilesLoaded { .. }));
    review.assert();
    assert!(
        evs.iter()
            .any(|e| matches!(e, Event::PullActionDone { number: 7, .. }))
    );
    assert!(evs.iter().any(|e| matches!(e, Event::PullLoaded { .. })));
}

#[test]
fn labels_are_changed_from_what_the_window_showed() {
    let mut server = mockito::Server::new();
    let w = signed_in(&mut server, TokenProvider::without_gh());
    let add = server
        .mock("POST", "/repos/o/r/issues/7/labels")
        .match_body(Matcher::Json(json!({ "labels": ["c"] })))
        .with_body("[]")
        .create();
    let remove = server
        .mock("DELETE", "/repos/o/r/issues/7/labels/a")
        .with_body("[]")
        .create();
    let put = server.mock("PUT", Matcher::Any).expect(0).create();
    let (_d, _f) = mock_detail(&mut server, 7);
    w.send(Command::SetLabels {
        slug: slug(),
        number: 7,
        old: vec!["a".into(), "b".into()],
        labels: vec!["b".into(), "c".into()],
    });
    let evs = until(&w, |e| matches!(e, Event::PullFilesLoaded { .. }));
    add.assert();
    remove.assert();
    put.assert();
    assert!(
        evs.iter()
            .any(|e| matches!(e, Event::PullActionDone { number: 7, .. }))
    );
}

#[test]
fn a_single_line_comment_is_sent_then_the_pull_request_reloaded() {
    let mut server = mockito::Server::new();
    let w = signed_in(&mut server, TokenProvider::without_gh());
    let posted = server
        .mock("POST", "/repos/o/r/pulls/7/comments")
        .match_body(Matcher::PartialJson(
            json!({ "commit_id": "abc", "line": 2, "side": "RIGHT" }),
        ))
        .with_status(201)
        .with_body("{}")
        .create();
    let (_d, _f) = mock_detail(&mut server, 7);
    w.send(Command::AddLineComment {
        slug: slug(),
        number: 7,
        commit_id: "abc".into(),
        comment: github::LineComment {
            path: "a.rs".into(),
            line: 2,
            side: github::DiffSide::Right,
            start: None,
            body: "Typo".into(),
        },
    });
    let evs = until(&w, |e| matches!(e, Event::PullFilesLoaded { .. }));
    posted.assert();
    assert!(
        evs.iter()
            .any(|e| matches!(e, Event::PullActionDone { number: 7, .. }))
    );
}

#[test]
fn a_thread_is_resolved_then_the_pull_request_reloaded() {
    let mut server = mockito::Server::new();
    let w = signed_in(&mut server, TokenProvider::without_gh());
    let resolve = server
        .mock("POST", "/graphql")
        .match_body(Matcher::Regex("\\bresolveReviewThread\\(".into()))
        .with_body(r#"{"data":{"resolveReviewThread":{"thread":{"isResolved":true}}}}"#)
        .create();
    let (_d, _f) = mock_detail(&mut server, 7);
    w.send(Command::ResolveThread {
        slug: slug(),
        number: 7,
        thread_id: "PRRT_1".into(),
        resolve: true,
    });
    let evs = until(&w, |e| matches!(e, Event::PullFilesLoaded { .. }));
    resolve.assert();
    assert!(
        evs.iter()
            .any(|e| matches!(e, Event::PullActionDone { number: 7, .. }))
    );
}

#[test]
fn a_merge_on_a_moved_head_is_refused_and_reloaded() {
    let mut server = mockito::Server::new();
    let w = signed_in(&mut server, TokenProvider::without_gh());
    server
        .mock("PUT", "/repos/o/r/pulls/7/merge")
        .with_status(409)
        .with_body(r#"{"message":"Head branch was modified. Review and try the merge again."}"#)
        .create();
    let (_d, _f) = mock_detail(&mut server, 7);
    w.send(Command::MergePull {
        slug: slug(),
        number: 7,
        merge: Merge {
            method: MergeMethod::Squash,
            title: "Fix (#7)".into(),
            message: String::new(),
            sha: "old".into(),
        },
        delete_branch: None,
    });
    let evs = until(&w, |e| matches!(e, Event::PullFilesLoaded { .. }));
    assert!(evs.iter().any(|e| matches!(
        e,
        Event::Error { during: Op::PullAction, error } if error.severity == Severity::Info && error.message.contains("changed")
    )));
    assert!(
        !evs.iter()
            .any(|e| matches!(e, Event::PullActionDone { .. }))
    );
}

#[test]
fn a_merged_branch_that_cannot_be_deleted_only_warns() {
    let mut server = mockito::Server::new();
    let w = signed_in(&mut server, TokenProvider::without_gh());
    server
        .mock("PUT", "/repos/o/r/pulls/7/merge")
        .with_body(r#"{"merged":true}"#)
        .create();
    server
        .mock("DELETE", "/repos/o/r/git/refs/heads/feat/x")
        .with_status(422)
        .with_body(r#"{"message":"Reference does not exist"}"#)
        .create();
    let (_d, _f) = mock_detail(&mut server, 7);
    w.send(Command::MergePull {
        slug: slug(),
        number: 7,
        merge: Merge {
            method: MergeMethod::Merge,
            title: "t".into(),
            message: "m".into(),
            sha: "abc".into(),
        },
        delete_branch: Some("feat/x".into()),
    });
    let evs = until(&w, |e| matches!(e, Event::PullFilesLoaded { .. }));
    assert!(evs.iter().any(|e| matches!(
        e,
        Event::Error { error, .. } if error.severity == Severity::Warning && error.detail.as_deref() == Some("Reference does not exist")
    )));
    assert!(
        evs.iter()
            .any(|e| matches!(e, Event::PullActionDone { number: 7, .. }))
    );
}

#[test]
fn creating_an_existing_pull_request_opens_it() {
    let mut server = mockito::Server::new();
    let w = signed_in(&mut server, TokenProvider::without_gh());
    server
        .mock("POST", "/repos/o/r/pulls")
        .with_status(422)
        .with_body(r#"{"message":"Validation Failed","errors":[{"message":"A pull request already exists for o:feat/x."}]}"#)
        .create();
    server
        .mock("GET", "/repos/o/r/pulls")
        .match_query(Matcher::UrlEncoded("head".into(), "o:feat/x".into()))
        .with_body(r#"[{"number":4}]"#)
        .create();
    let (_d, _f) = mock_detail(&mut server, 4);
    w.send(Command::CreatePull {
        slug: slug(),
        pull: NewPull {
            title: "Fix".into(),
            body: String::new(),
            head: "feat/x".into(),
            base: "main".into(),
            draft: false,
            labels: vec![],
        },
        publish: false,
    });
    let evs = until(&w, |e| matches!(e, Event::PullCreated { .. }));
    assert!(matches!(
        evs.last(),
        Some(Event::PullCreated { number: 4, .. })
    ));
    assert!(
        evs.iter()
            .any(|e| matches!(e, Event::Error { error, .. } if error.severity == Severity::Info))
    );
}

#[test]
fn a_created_pull_request_opens_even_when_its_labels_fail() {
    let mut server = mockito::Server::new();
    let w = signed_in(&mut server, TokenProvider::without_gh());
    server
        .mock("POST", "/repos/o/r/pulls")
        .with_status(201)
        .with_body(r#"{"number":12}"#)
        .create();
    let labels = server
        .mock("POST", "/repos/o/r/issues/12/labels")
        .match_body(Matcher::Json(json!({ "labels": ["bug"] })))
        .with_status(422)
        .with_body(r#"{"message":"Label does not exist"}"#)
        .create();
    let (_d, _f) = mock_detail(&mut server, 12);
    w.send(Command::CreatePull {
        slug: slug(),
        pull: NewPull {
            title: "Fix".into(),
            body: String::new(),
            head: "feat/x".into(),
            base: "main".into(),
            draft: false,
            labels: vec!["bug".into()],
        },
        publish: false,
    });
    let evs = until(&w, |e| matches!(e, Event::PullFilesLoaded { .. }));
    labels.assert();
    let created = evs
        .iter()
        .position(|e| matches!(e, Event::PullCreated { number: 12, .. }))
        .unwrap();
    let warned = evs
        .iter()
        .position(|e| {
            matches!(e, Event::Error { error, .. }
                if error.severity == Severity::Warning
                    && error.message == retrogit::strings::PULL_LABELS_FAILED
                    && error.detail.as_deref().is_some_and(|d| d.contains("Label does not exist")))
        })
        .unwrap();
    assert!(created < warned, "{evs:?}");
}

#[test]
fn a_revoked_token_signs_out() {
    let mut server = mockito::Server::new();
    let w = signed_in(&mut server, TokenProvider::without_gh());
    server.mock("POST", "/graphql").with_status(401).create();
    w.send(Command::LoadPulls {
        slug: slug(),
        filter: PrFilter::Mine,
    });
    until(&w, |e| matches!(e, Event::SignedOut));
}

mod checkout {
    use super::*;
    use std::process::Command as Cmd;

    fn git(dir: &Path, args: &[&str]) -> String {
        let out = Cmd::new("git")
            .current_dir(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    fn configure(dir: &Path) {
        for (k, v) in [
            ("user.name", "Ada"),
            ("user.email", "ada@example.com"),
            ("commit.gpgsign", "false"),
            ("core.hooksPath", ".git/hooks"),
            ("core.autocrlf", "false"),
        ] {
            git(dir, &["config", k, v]);
        }
    }

    /// origin.git with `main`, a `feat/x` branch and `refs/pull/1/head` (a fork's PR);
    /// `work` is a clone of it.
    fn env() -> Option<(tempfile::TempDir, std::path::PathBuf)> {
        if !gitcore::git_available() {
            return None;
        }
        let tmp = tempfile::tempdir().unwrap();
        let root = retrogit::watch::canonical(tmp.path());
        git(
            &root,
            &[
                "-c",
                "init.defaultBranch=main",
                "init",
                "-q",
                "--bare",
                "origin.git",
            ],
        );
        git(
            &root,
            &[
                "-c",
                "core.autocrlf=false",
                "clone",
                "-q",
                "origin.git",
                "seed",
            ],
        );
        let seed = root.join("seed");
        configure(&seed);
        git(&seed, &["switch", "-q", "-c", "main"]);
        std::fs::write(seed.join("a.txt"), "a\n").unwrap();
        git(&seed, &["add", "a.txt"]);
        git(&seed, &["commit", "-q", "-m", "a"]);
        git(&seed, &["push", "-q", "origin", "main"]);
        git(&seed, &["switch", "-q", "-c", "feat/x"]);
        std::fs::write(seed.join("x.txt"), "x\n").unwrap();
        git(&seed, &["add", "x.txt"]);
        git(&seed, &["commit", "-q", "-m", "x"]);
        git(&seed, &["push", "-q", "origin", "feat/x"]);
        git(&seed, &["switch", "-q", "main"]);
        std::fs::write(seed.join("fork.txt"), "fork\n").unwrap();
        git(&seed, &["add", "fork.txt"]);
        git(&seed, &["commit", "-q", "-m", "fork"]);
        git(&seed, &["push", "-q", "origin", "HEAD:refs/pull/1/head"]);
        git(
            &root,
            &[
                "-c",
                "core.autocrlf=false",
                "clone",
                "-q",
                "origin.git",
                "work",
            ],
        );
        let work = root.join("work");
        configure(&work);
        Some((tmp, work))
    }

    fn head_branch(evs: &[Event]) -> Option<String> {
        evs.iter().rev().find_map(|e| match e {
            Event::BranchesLoaded(b) => b.iter().find(|b| b.is_head).map(|b| b.name.clone()),
            _ => None,
        })
    }

    #[test]
    fn fork_pull_requests_are_checked_out_as_pr_n_and_others_as_their_branch() {
        let Some((_tmp, work)) = env() else { return };
        let server = mockito::Server::new();
        let w = start(&server, TokenProvider::without_gh());
        w.send(Command::OpenRepo(work.clone()));
        until(&w, |e| matches!(e, Event::SyncFinished { .. }));
        w.send(Command::CheckoutPull {
            number: 1,
            head: None,
        });
        let evs = until(&w, |e| matches!(e, Event::PullActionDone { .. }));
        assert_eq!(head_branch(&evs).as_deref(), Some("pr/1"));
        assert!(work.join("fork.txt").exists());
        w.send(Command::CheckoutPull {
            number: 2,
            head: Some("feat/x".into()),
        });
        let evs = until(&w, |e| matches!(e, Event::PullActionDone { .. }));
        assert_eq!(head_branch(&evs).as_deref(), Some("feat/x"));
        assert!(
            matches!(evs.last(), Some(Event::PullActionDone { note, .. }) if note.ends_with("feat/x"))
        );
    }

    #[test]
    fn an_existing_local_branch_is_brought_up_to_date() {
        let Some((_tmp, work)) = env() else { return };
        let server = mockito::Server::new();
        let w = start(&server, TokenProvider::without_gh());
        w.send(Command::OpenRepo(work.clone()));
        until(&w, |e| matches!(e, Event::SyncFinished { .. }));
        w.send(Command::CheckoutPull {
            number: 2,
            head: Some("feat/x".into()),
        });
        until(&w, |e| matches!(e, Event::PullActionDone { .. }));
        // The author pushes again; the reviewer is back on main, then checks out again.
        let seed = work.parent().unwrap().join("seed");
        git(&seed, &["switch", "-q", "feat/x"]);
        std::fs::write(seed.join("y.txt"), "y\n").unwrap();
        git(&seed, &["add", "y.txt"]);
        git(&seed, &["commit", "-q", "-m", "y"]);
        git(&seed, &["push", "-q", "origin", "feat/x"]);
        git(&work, &["switch", "-q", "main"]);
        w.send(Command::CheckoutPull {
            number: 2,
            head: Some("feat/x".into()),
        });
        until(&w, |e| matches!(e, Event::PullActionDone { .. }));
        assert!(
            work.join("y.txt").exists(),
            "fast-forwarded to the pull request"
        );
    }

    #[test]
    fn a_pull_request_branch_blocked_by_local_changes_is_not_a_failed_fetch() {
        use retrogit::protocol::SyncOp;
        use retrogit::strings as s;
        let Some((_tmp, work)) = env() else { return };
        let seed = work.parent().unwrap().join("seed");
        let server = mockito::Server::new();
        let w = start(&server, TokenProvider::without_gh());
        w.send(Command::OpenRepo(work.clone()));
        until(&w, |e| matches!(e, Event::SyncFinished { .. }));
        w.send(Command::CheckoutPull {
            number: 1,
            head: None,
        });
        until(&w, |e| matches!(e, Event::PullActionDone { .. }));
        // A local file the pull request's next commit also brings.
        std::fs::write(work.join("new.txt"), "local\n").unwrap();
        std::fs::write(seed.join("new.txt"), "fork\n").unwrap();
        git(&seed, &["add", "new.txt"]);
        git(&seed, &["commit", "-q", "-m", "new"]);
        git(&seed, &["push", "-q", "origin", "HEAD:refs/pull/1/head"]);
        w.send(Command::CheckoutPull {
            number: 1,
            head: None,
        });
        let evs = until(&w, |e| matches!(e, Event::Error { .. }));
        assert!(
            !evs.iter().any(|e| matches!(
                e,
                Event::SyncFinished {
                    op: SyncOp::Fetch,
                    ok: false
                }
            )),
            "{evs:?}"
        );
        assert!(
            evs.iter().any(|e| matches!(
                e,
                Event::SyncFinished {
                    op: SyncOp::Fetch,
                    ok: true
                }
            )),
            "{evs:?}"
        );
        assert!(
            matches!(
                evs.last(),
                Some(Event::Error { during: Op::PullAction, error })
                    if error.message == s::ERR_WOULD_OVERWRITE
            ),
            "{evs:?}"
        );
    }

    fn fetch_shown(evs: &[Event]) -> bool {
        use retrogit::protocol::SyncOp;
        evs.iter().any(|e| {
            matches!(
                e,
                Event::SyncStarted {
                    op: SyncOp::Fetch,
                    background: false
                }
            )
        }) && evs.iter().any(|e| {
            matches!(
                e,
                Event::SyncFinished {
                    op: SyncOp::Fetch,
                    ok: true
                }
            )
        })
    }

    fn behind_warning(evs: &[Event]) -> Option<String> {
        evs.iter().find_map(|e| match e {
            Event::Error {
                during: Op::PullAction,
                error,
            } if error.severity == Severity::Warning => Some(error.message.clone()),
            _ => None,
        })
    }

    #[test]
    fn checkout_fetches_with_progress_like_fetch() {
        let Some((_tmp, work)) = env() else { return };
        let server = mockito::Server::new();
        let w = start(&server, TokenProvider::without_gh());
        w.send(Command::OpenRepo(work.clone()));
        until(&w, |e| matches!(e, Event::SyncFinished { .. }));
        for (number, head) in [(1, None), (2, Some("feat/x".to_string()))] {
            w.send(Command::CheckoutPull { number, head });
            let evs = until(&w, |e| matches!(e, Event::PullActionDone { .. }));
            assert!(fetch_shown(&evs), "{evs:?}");
            assert_eq!(behind_warning(&evs), None, "nothing local");
        }
    }

    #[test]
    fn a_pull_request_branch_with_local_commits_says_it_was_not_moved() {
        let Some((_tmp, work)) = env() else { return };
        let seed = work.parent().unwrap().join("seed");
        let server = mockito::Server::new();
        let w = start(&server, TokenProvider::without_gh());
        w.send(Command::OpenRepo(work.clone()));
        until(&w, |e| matches!(e, Event::SyncFinished { .. }));
        for (number, head, branch) in [(1, None, "pr/1"), (2, Some("feat/x"), "feat/x")] {
            let checkout = || Command::CheckoutPull {
                number,
                head: head.map(str::to_string),
            };
            w.send(checkout());
            until(&w, |e| matches!(e, Event::PullActionDone { .. }));
            // A local commit, then the author pushes again.
            std::fs::write(work.join(format!("mine-{number}.txt")), "mine\n").unwrap();
            git(&work, &["add", "-A"]);
            git(&work, &["commit", "-q", "-m", "mine"]);
            let remote = match head {
                Some(b) => b.to_string(),
                None => "refs/pull/1/head".to_string(),
            };
            git(
                &seed,
                &[
                    "fetch",
                    "-q",
                    "origin",
                    &format!("{remote}:theirs-{number}"),
                ],
            );
            git(&seed, &["switch", "-q", &format!("theirs-{number}")]);
            std::fs::write(seed.join(format!("theirs-{number}.txt")), "t\n").unwrap();
            git(&seed, &["add", "-A"]);
            git(&seed, &["commit", "-q", "-m", "theirs"]);
            git(&seed, &["push", "-q", "origin", &format!("HEAD:{remote}")]);
            git(&work, &["switch", "-q", "main"]);
            w.send(checkout());
            let evs = until(&w, |e| matches!(e, Event::PullActionDone { .. }));
            assert_eq!(head_branch(&evs).as_deref(), Some(branch));
            assert_eq!(
                behind_warning(&evs),
                Some(retrogit::strings::pr_branch_behind(branch)),
                "{evs:?}"
            );
            assert!(work.join(format!("mine-{number}.txt")).exists(), "kept");
            assert!(!work.join(format!("theirs-{number}.txt")).exists());
        }
    }

    #[test]
    fn local_changes_in_the_way_ask_to_stash() {
        let Some((_tmp, work)) = env() else { return };
        let server = mockito::Server::new();
        let w = start(&server, TokenProvider::without_gh());
        w.send(Command::OpenRepo(work.clone()));
        until(&w, |e| matches!(e, Event::SyncFinished { .. }));
        std::fs::write(work.join("fork.txt"), "mine\n").unwrap();
        w.send(Command::CheckoutPull {
            number: 1,
            head: None,
        });
        let evs = until(&w, |e| matches!(e, Event::WouldOverwrite { .. }));
        assert!(
            matches!(evs.last(), Some(Event::WouldOverwrite { branch, .. }) if branch == "pr/1")
        );
        // The failed switch resyncs (still on main)...
        let evs = until(&w, |e| matches!(e, Event::BranchesLoaded(_)));
        assert_eq!(head_branch(&evs).as_deref(), Some("main"));
        // ...then "Stash and switch" (the existing dialog) finishes the checkout.
        w.send(Command::SwitchBranch {
            name: "pr/1".into(),
            stash: true,
        });
        let evs = until(&w, |e| matches!(e, Event::BranchesLoaded(_)));
        assert_eq!(head_branch(&evs).as_deref(), Some("pr/1"));
        assert!(
            !evs.iter()
                .any(|e| matches!(e, Event::PullActionDone { .. }))
        );
    }
}

mod more {
    use super::*;

    #[test]
    fn a_refresh_reloads_the_detail_but_not_the_files() {
        let mut server = mockito::Server::new();
        let w = signed_in(&mut server, TokenProvider::without_gh());
        let detail = server
            .mock("POST", "/graphql")
            .match_body(Matcher::PartialJson(
                json!({ "variables": { "number": 7 } }),
            ))
            .with_body(detail_body(7, "abc"))
            .create();
        let files = server
            .mock("GET", "/repos/o/r/pulls/7/files")
            .match_query(Matcher::Any)
            .with_body("[]")
            .expect(0)
            .create();
        w.send(Command::RefreshPull {
            slug: slug(),
            number: 7,
        });
        until(&w, |e| matches!(e, Event::PullLoaded { .. }));
        w.send(Command::ValidateToken);
        until(&w, |e| matches!(e, Event::SignedIn(_) | Event::SignedOut));
        detail.assert();
        files.assert();
    }

    #[test]
    fn title_reviewers_assignees_and_draft_are_changed_then_reloaded() {
        let mut server = mockito::Server::new();
        let w = signed_in(&mut server, TokenProvider::without_gh());
        let patch = server
            .mock("PATCH", "/repos/o/r/pulls/7")
            .with_body("{}")
            .create();
        let reviewers = server
            .mock("POST", "/repos/o/r/pulls/7/requested_reviewers")
            .match_body(Matcher::Json(json!({ "reviewers": ["carol"] })))
            .with_status(201)
            .with_body("{}")
            .create();
        let ready = server
            .mock("POST", "/graphql")
            .match_body(Matcher::Regex("markPullRequestReadyForReview".into()))
            .with_body(
                r#"{"data":{"markPullRequestReadyForReview":{"pullRequest":{"isDraft":false}}}}"#,
            )
            .create();
        let (_d, _f) = mock_detail(&mut server, 7);
        w.send(Command::UpdatePull {
            slug: slug(),
            number: 7,
            title: "New".into(),
            body: "Text".into(),
        });
        until(&w, |e| matches!(e, Event::PullFilesLoaded { .. }));
        w.send(Command::SetPeople {
            slug: slug(),
            number: 7,
            kind: retrogit::state::PeopleKind::Reviewers,
            add: vec!["carol".into()],
            remove: vec![],
        });
        until(&w, |e| matches!(e, Event::PullFilesLoaded { .. }));
        w.send(Command::SetDraft {
            slug: slug(),
            number: 7,
            pull_id: "PR_7".into(),
            draft: false,
        });
        let evs = until(&w, |e| matches!(e, Event::PullFilesLoaded { .. }));
        assert!(evs.iter().any(|e| matches!(e, Event::PullActionDone { note, .. } if note == retrogit::strings::NOTE_READY)));
        patch.assert();
        reviewers.assert();
        ready.assert();
    }

    #[test]
    fn a_half_done_people_change_says_which_half() {
        let mut server = mockito::Server::new();
        let w = signed_in(&mut server, TokenProvider::without_gh());
        let add = server
            .mock("POST", "/repos/o/r/issues/7/assignees")
            .match_body(Matcher::Json(json!({ "assignees": ["carol"] })))
            .with_status(201)
            .with_body("{}")
            .create();
        let remove = server
            .mock("DELETE", "/repos/o/r/issues/7/assignees")
            .with_status(422)
            .with_body(r#"{"message":"Validation Failed"}"#)
            .create();
        let (_d, _f) = mock_detail(&mut server, 7);
        w.send(Command::SetPeople {
            slug: slug(),
            number: 7,
            kind: retrogit::state::PeopleKind::Assignees,
            add: vec!["carol".into()],
            remove: vec!["bob".into(), "dan".into()],
        });
        let evs = until(&w, |e| matches!(e, Event::PullFilesLoaded { .. }));
        add.assert();
        remove.assert();
        let message = evs.iter().find_map(|e| match e {
            Event::Error {
                during: Op::PullAction,
                error,
            } => Some(error.message.clone()),
            _ => None,
        });
        assert_eq!(
            message.as_deref(),
            Some("Assignees added, but @bob, @dan could not be removed.")
        );
        assert_eq!(
            message.unwrap(),
            retrogit::strings::people_partly(
                retrogit::strings::PEOPLE_ASSIGNEES,
                &["bob".to_string(), "dan".to_string()]
            )
        );
        assert!(
            evs.iter().any(|e| matches!(e, Event::PullLoaded { .. })),
            "reloaded"
        );
        assert!(
            !evs.iter()
                .any(|e| matches!(e, Event::PullActionDone { .. }))
        );
    }

    #[test]
    fn people_who_can_be_asked_are_loaded() {
        let mut server = mockito::Server::new();
        let w = signed_in(&mut server, TokenProvider::without_gh());
        server
            .mock("POST", "/graphql")
            .match_body(Matcher::Regex("assignableUsers".into()))
            .with_body(
                r#"{"data":{"repository":{"assignableUsers":{"nodes":[{"login":"carol"}]}}}}"#,
            )
            .create();
        w.send(Command::LoadAssignable {
            slug: slug(),
            query: String::new(),
        });
        let evs = until(&w, |e| matches!(e, Event::AssignableLoaded { .. }));
        assert!(
            matches!(evs.last(), Some(Event::AssignableLoaded { users, .. }) if users == &["carol".to_string()])
        );
    }
}

mod suggestions {
    use super::*;
    use std::process::Command as Cmd;

    fn git(dir: &Path, args: &[&str]) -> String {
        let out = Cmd::new("git")
            .current_dir(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    #[test]
    fn a_suggestion_is_applied_only_on_the_pull_request_branch_at_its_head() {
        if !gitcore::git_available() {
            return;
        }
        let d = tempfile::tempdir().unwrap();
        let dir = retrogit::watch::canonical(d.path());
        git(&dir, &["-c", "init.defaultBranch=main", "init", "-q"]);
        for (k, v) in [
            ("user.name", "Ada"),
            ("user.email", "ada@example.com"),
            ("commit.gpgsign", "false"),
            ("core.hooksPath", ".git/hooks"),
            ("core.autocrlf", "false"),
        ] {
            git(&dir, &["config", k, v]);
        }
        std::fs::write(dir.join("a.rs"), "a\nb\n").unwrap();
        git(&dir, &["add", "-A"]);
        git(&dir, &["commit", "-q", "-m", "base"]);
        let head = git(&dir, &["rev-parse", "HEAD"]).trim().to_string();
        let server = mockito::Server::new();
        let w = start(&server, TokenProvider::without_gh());
        w.send(Command::OpenRepo(dir.clone()));
        until(&w, |e| matches!(e, Event::StatusLoaded(_)));
        let apply = |branch: &str, sha: &str| Command::ApplySuggestion {
            number: 7,
            head_branch: branch.into(),
            head_sha: sha.into(),
            path: "a.rs".into(),
            start: 2,
            end: 2,
            expected: vec!["b".into()],
            replacement: "B\n".into(),
            author: "bob".into(),
            author_id: None,
        };
        // Not the pull request's branch: refused.
        w.send(apply("feat/x", &head));
        let evs = until(&w, |e| matches!(e, Event::Error { .. }));
        assert!(
            matches!(evs.last(), Some(Event::Error { error, .. }) if error.message == retrogit::strings::WHY_CHECKOUT_FIRST)
        );
        // Right branch, but the pull request moved on GitHub: pull first.
        w.send(apply("main", "0000000000000000000000000000000000000000"));
        let evs = until(&w, |e| matches!(e, Event::Error { .. }));
        assert!(
            matches!(evs.last(), Some(Event::Error { error, .. }) if error.message == retrogit::strings::WHY_PULL_FIRST)
        );
        // Right branch at the right commit.
        w.send(apply("main", &head));
        let evs = until(&w, |e| matches!(e, Event::PullActionDone { .. }));
        assert!(
            matches!(evs.last(), Some(Event::PullActionDone { note, .. }) if note == retrogit::strings::NOTE_SUGGESTION_APPLIED)
        );
        assert_eq!(std::fs::read_to_string(dir.join("a.rs")).unwrap(), "a\nB\n");
        assert_eq!(
            git(&dir, &["log", "-1", "--format=%s"]).trim(),
            "Apply suggestion from @bob"
        );
        // A second suggestion of the same review (HEAD is now past the PR head).
        w.send(Command::ApplySuggestion {
            number: 7,
            head_branch: "main".into(),
            head_sha: head.clone(),
            path: "a.rs".into(),
            start: 1,
            end: 1,
            expected: vec!["a".into()],
            replacement: "A\n".into(),
            author: "carol".into(),
            author_id: None,
        });
        let evs = until(&w, |e| {
            matches!(e, Event::PullActionDone { .. } | Event::Error { .. })
        });
        assert!(
            matches!(evs.last(), Some(Event::PullActionDone { .. })),
            "{evs:?}"
        );
        assert_eq!(std::fs::read_to_string(dir.join("a.rs")).unwrap(), "A\nB\n");
    }

    /// A repository of `ada` (the signed-in account) with `a.rs` = "a\nb\nc\n", open in
    /// the worker. Origin is set after opening: no fetch reaches github.com.
    fn ada_repo(
        server: &mut mockito::Server,
    ) -> (tempfile::TempDir, std::path::PathBuf, WorkerHandle) {
        let d = tempfile::tempdir().unwrap();
        let dir = retrogit::watch::canonical(d.path());
        git(&dir, &["-c", "init.defaultBranch=main", "init", "-q"]);
        for (k, v) in [
            ("user.name", "Ada"),
            ("user.email", "ada@example.com"),
            ("commit.gpgsign", "false"),
            ("core.hooksPath", ".git/hooks"),
            ("core.autocrlf", "false"),
        ] {
            git(&dir, &["config", k, v]);
        }
        std::fs::write(dir.join("a.rs"), "a\nb\nc\n").unwrap();
        git(&dir, &["add", "-A"]);
        git(&dir, &["commit", "-q", "-m", "base"]);
        let w = signed_in(server, TokenProvider::without_gh());
        w.send(Command::OpenRepo(dir.clone()));
        until(&w, |e| matches!(e, Event::StatusLoaded(_)));
        git(
            &dir,
            &["remote", "add", "origin", "https://github.com/ada/r.git"],
        );
        (d, dir, w)
    }

    fn suggest(
        dir: &Path,
        line: u32,
        from: &str,
        to: &str,
        author: &str,
        id: Option<u64>,
    ) -> Command {
        Command::ApplySuggestion {
            number: 7,
            head_branch: "main".into(),
            head_sha: git(dir, &["rev-parse", "HEAD"]).trim().to_string(),
            path: "a.rs".into(),
            start: line,
            end: line,
            expected: vec![from.into()],
            replacement: format!("{to}\n"),
            author: author.into(),
            author_id: id,
        }
    }

    fn co_authors(dir: &Path) -> String {
        git(
            dir,
            &["log", "-1", "--format=%(trailers:key=Co-authored-by)"],
        )
        .trim()
        .to_string()
    }

    #[test]
    fn suggestion_commits_credit_their_author_but_not_yourself_or_a_bot() {
        if !gitcore::git_available() {
            return;
        }
        let mut server = mockito::Server::new();
        let (_d, dir, w) = ada_repo(&mut server);
        w.send(suggest(&dir, 1, "a", "A", "bob", Some(41)));
        let evs = until(&w, |e| {
            matches!(e, Event::PullActionDone { .. } | Event::Error { .. })
        });
        assert!(
            matches!(evs.last(), Some(Event::PullActionDone { .. })),
            "{evs:?}"
        );
        assert_eq!(
            git(&dir, &["log", "-1", "--format=%B"]).trim(),
            "Apply suggestion from @bob\n\nCo-authored-by: bob <41+bob@users.noreply.github.com>"
        );
        // The repository's own account, then a bot (no user id): no trailer.
        for (line, from, author, id) in [(2, "b", "Ada", Some(42)), (3, "c", "copilot", None)] {
            w.send(suggest(&dir, line, from, "X", author, id));
            let evs = until(&w, |e| {
                matches!(e, Event::PullActionDone { .. } | Event::Error { .. })
            });
            assert!(
                matches!(evs.last(), Some(Event::PullActionDone { .. })),
                "{evs:?}"
            );
            assert_eq!(
                git(&dir, &["log", "-1", "--format=%s"]).trim(),
                format!("Apply suggestion from @{author}")
            );
            assert_eq!(co_authors(&dir), "", "{author}");
        }
    }

    #[test]
    fn a_suggestion_already_applied_says_so() {
        if !gitcore::git_available() {
            return;
        }
        let mut server = mockito::Server::new();
        let (_d, dir, w) = ada_repo(&mut server);
        let before = git(&dir, &["rev-parse", "HEAD"]);
        w.send(suggest(&dir, 2, "b", "b", "bob", Some(41)));
        let evs = until(&w, |e| {
            matches!(e, Event::PullActionDone { .. } | Event::Error { .. })
        });
        assert!(
            matches!(evs.last(), Some(Event::Error { error, .. })
                if error.severity == Severity::Info
                    && error.message == retrogit::strings::SUGGESTION_ALREADY_APPLIED),
            "{evs:?}"
        );
        assert_eq!(git(&dir, &["rev-parse", "HEAD"]), before);
    }
}
