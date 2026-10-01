#![allow(clippy::unwrap_used)]
//! Pull request commands of the worker, against a mock GitHub.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use github::{
    Client, MemoryStore, Merge, MergeMethod, NewPull, PrFilter, Review, ReviewEvent, TokenProvider,
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
        store: Arc::new(MemoryStore::default()),
        client_id: String::new(),
        commit_backend: gitcore::CommitBackend::Git2,
        tokens,
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

/// Worker signed in with the PAT `ghp_pat` (login "ada").
fn signed_in(server: &mut mockito::Server, tokens: TokenProvider) -> WorkerHandle {
    server
        .mock("GET", "/user")
        .with_body(r#"{"login":"ada","name":null}"#)
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
        .with_body(r#"{"data":{"search":{"nodes":[{"number":7,"title":"Fix"}]}}}"#)
        .create();
    w.send(Command::LoadPulls {
        slug: slug(),
        filter: PrFilter::Open,
    });
    let evs = until(&w, |e| matches!(e, Event::PullsLoaded { .. }));
    assert!(matches!(
        evs.last(),
        Some(Event::PullsLoaded { slug: s, filter: PrFilter::Open, list }) if *s == slug() && list[0].number == 7
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
    let tokens = TokenProvider::new(Arc::new(|login: Option<&str>| {
        assert_eq!(login, Some("ada"), "gh is asked for the signed-in account");
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
