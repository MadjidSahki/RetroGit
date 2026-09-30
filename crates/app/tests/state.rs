#![allow(clippy::unwrap_used)]

use std::path::PathBuf;

use gitcore::{CloneProgress, Head, RepoSummary};
use github::{RepoInfo, User};
use retrogit::config::Config;
use retrogit::protocol::{AppError, Event, Op, Severity};
use retrogit::state::{AppState, Auth, CloneDialog, SignInDialog, filter_repos};

fn user() -> User {
    User {
        login: "ada".into(),
        name: None,
    }
}

fn summary(path: &str) -> RepoSummary {
    RepoSummary {
        name: "demo".into(),
        path: PathBuf::from(path),
        head: Head::Branch("main".into()),
        origin_url: None,
        last_commit: None,
    }
}

fn repo(full: &str) -> RepoInfo {
    let (owner, name) = full.split_once('/').unwrap();
    RepoInfo {
        full_name: full.into(),
        name: name.into(),
        owner: owner.into(),
        private: false,
        clone_url: format!("https://github.com/{full}.git"),
        updated_at: "2026-09-30T10:00:00Z".into(),
    }
}

fn err() -> AppError {
    AppError::new(Severity::Error, "boom")
}

#[test]
fn starts_checking_and_flags_missing_recents() {
    let mut c = Config::default();
    c.add_recent(
        "gone",
        std::path::Path::new("/definitely/not/here/retrogit"),
    );
    let s = AppState::new(c);
    assert_eq!(s.auth, Auth::Checking);
    assert!(
        s.missing
            .contains(&PathBuf::from("/definitely/not/here/retrogit"))
    );
}

#[test]
fn signed_out_opens_sign_in_and_clears_repos() {
    let mut s = AppState::new(Config::default());
    s.repos = vec![repo("a/b")];
    s.apply(Event::SignedOut);
    assert_eq!(s.auth, Auth::SignedOut);
    assert!(s.repos.is_empty());
    assert_eq!(s.sign_in, Some(SignInDialog::default()));
}

#[test]
fn device_code_then_signed_in_closes_dialog() {
    let mut s = AppState::new(Config::default());
    s.sign_in = Some(SignInDialog::default());
    s.apply(Event::DeviceCode {
        user_code: "ABCD-1234".into(),
        verification_uri: "https://github.com/login/device".into(),
    });
    assert!(matches!(s.auth, Auth::Waiting { ref user_code, .. } if user_code == "ABCD-1234"));
    s.apply(Event::SignedIn(user()));
    assert_eq!(s.user().unwrap().login, "ada");
    assert_eq!(s.sign_in, None);
}

#[test]
fn auth_error_while_waiting_resets_to_signed_out_and_queues_message() {
    let mut s = AppState::new(Config::default());
    s.sign_in = Some(SignInDialog {
        tab: 1,
        pat: "x".into(),
        pat_submitted: true,
    });
    s.auth = Auth::Waiting {
        user_code: "A".into(),
        verification_uri: "u".into(),
    };
    s.apply(Event::Error {
        during: Op::Auth,
        error: err(),
    });
    assert_eq!(s.auth, Auth::SignedOut);
    assert!(!s.sign_in.as_ref().unwrap().pat_submitted);
    assert_eq!(s.messages.len(), 1);
}

#[test]
fn clone_progress_done_and_recent_list() {
    let mut s = AppState::new(Config::default());
    s.clone = Some(CloneDialog::default());
    let p = CloneProgress {
        received_objects: 1,
        total_objects: 2,
        ..Default::default()
    };
    s.apply(Event::CloneProgress(p));
    assert_eq!(s.clone.as_ref().unwrap().progress, Some(p));
    s.apply(Event::CloneDone(summary("/tmp/demo")));
    assert!(s.clone.is_none());
    assert_eq!(s.config.recent[0].path, PathBuf::from("/tmp/demo"));
    assert!(s.config_dirty);
    assert_eq!(s.current.as_ref().unwrap().name, "demo");
}

#[test]
fn clone_cancel_or_error_keeps_dialog_but_stops_progress() {
    let mut s = AppState::new(Config::default());
    s.clone = Some(CloneDialog {
        progress: Some(CloneProgress::default()),
        ..Default::default()
    });
    s.apply(Event::CloneCancelled);
    assert_eq!(s.clone.as_ref().unwrap().progress, None);
    s.clone.as_mut().unwrap().progress = Some(CloneProgress::default());
    s.apply(Event::Error {
        during: Op::Clone,
        error: err(),
    });
    assert_eq!(s.clone.as_ref().unwrap().progress, None);
}

#[test]
fn open_error_on_vanished_recent_marks_it_missing() {
    let mut c = Config::default();
    let gone = PathBuf::from("/definitely/not/here/retrogit2");
    c.add_recent("gone", &gone);
    let mut s = AppState::new(c);
    s.missing.clear();
    s.apply(Event::Error {
        during: Op::Open(gone.clone()),
        error: err(),
    });
    assert!(s.missing.contains(&gone));
}

#[test]
fn reopening_clears_missing_and_remove_recent_clears_current() {
    let mut s = AppState::new(Config::default());
    s.missing.insert(PathBuf::from("/tmp/demo"));
    s.apply(Event::RepoOpened(summary("/tmp/demo")));
    assert!(s.missing.is_empty());
    s.remove_recent(std::path::Path::new("/tmp/demo"));
    assert!(s.config.recent.is_empty());
    assert!(s.current.is_none());
}

#[test]
fn filter_is_case_insensitive_on_owner_and_name() {
    let repos = vec![
        repo("ExampleOrg/acme-cor-lab"),
        repo("ada/retrogit"),
        repo("ada/Notes"),
    ];
    assert_eq!(filter_repos(&repos, ""), vec![0, 1, 2]);
    assert_eq!(filter_repos(&repos, "  ADA/ "), vec![1, 2]);
    assert_eq!(filter_repos(&repos, "exampleorg"), vec![0]);
    assert_eq!(filter_repos(&repos, "zzz"), Vec::<usize>::new());
}

#[test]
fn offline_does_not_open_sign_in() {
    let mut s = AppState::new(Config::default());
    s.apply(Event::Error {
        during: Op::Auth,
        error: err(),
    });
    s.apply(Event::Offline);
    assert_eq!(s.auth, Auth::Offline);
    assert_eq!(s.sign_in, None);
    assert_eq!(s.messages.len(), 1);
}

#[test]
fn cancelled_clone_shows_an_info_message() {
    let mut s = AppState::new(Config::default());
    s.clone = Some(CloneDialog {
        progress: Some(CloneProgress::default()),
        ..Default::default()
    });
    s.apply(Event::CloneCancelled);
    let m = s.messages.front().unwrap();
    assert_eq!(m.severity, Severity::Info);
    assert_eq!(m.message, retrogit::strings::INFO_CLONE_CANCELLED);
}

mod changes {
    use super::*;
    use gitcore::{Change, CommitInfo, CommitOutcome, FileDiff, FileStatus, Side};

    fn file(path: &str, staged: Option<Change>, unstaged: Option<Change>) -> FileStatus {
        FileStatus {
            path: path.into(),
            staged,
            unstaged,
        }
    }

    fn diff(path: &str, side: Side) -> FileDiff {
        FileDiff {
            path: path.into(),
            side,
            binary: false,
            hunks: vec![],
        }
    }

    fn outcome(used_cli: bool) -> CommitOutcome {
        CommitOutcome {
            commit: CommitInfo {
                short_id: "a1b2c3d".into(),
                summary: "s".into(),
                author: "Ada".into(),
                time: 0,
            },
            used_cli,
        }
    }

    #[test]
    fn new_diff_clears_the_line_selection_and_ignores_stale_diffs() {
        let mut s = AppState::new(Config::default());
        s.changes.shown = Some(("a.txt".into(), Side::Unstaged));
        s.changes.selected_lines.insert((0, 1));
        s.apply(Event::DiffLoaded(diff("b.txt", Side::Unstaged)));
        assert!(
            s.changes.diff.is_none(),
            "diff of another file must be ignored"
        );
        assert_eq!(s.changes.selected_lines.len(), 1);
        s.apply(Event::DiffLoaded(diff("a.txt", Side::Unstaged)));
        assert!(s.changes.diff.is_some());
        assert!(s.changes.selected_lines.is_empty());
    }

    #[test]
    fn an_identical_diff_from_a_background_refresh_keeps_the_selection() {
        let mut s = AppState::new(Config::default());
        s.changes.shown = Some(("a.txt".into(), Side::Unstaged));
        s.apply(Event::DiffLoaded(diff("a.txt", Side::Unstaged)));
        s.changes.selected_lines.insert((0, 1));
        s.changes.show_large = true;
        s.apply(Event::DiffLoaded(diff("a.txt", Side::Unstaged)));
        assert_eq!(s.changes.selected_lines.len(), 1);
        assert!(s.changes.show_large);
        let mut changed = diff("a.txt", Side::Unstaged);
        changed.binary = true;
        s.apply(Event::DiffLoaded(changed));
        assert!(
            s.changes.selected_lines.is_empty(),
            "a different diff invalidates the selection"
        );
    }

    #[test]
    fn status_without_the_shown_file_closes_its_diff() {
        let mut s = AppState::new(Config::default());
        s.changes.shown = Some(("a.txt".into(), Side::Unstaged));
        s.changes.diff = Some(diff("a.txt", Side::Unstaged));
        s.apply(Event::StatusLoaded(vec![file(
            "a.txt",
            Some(Change::Modified),
            None,
        )]));
        assert_eq!(s.changes.shown, None);
        assert_eq!(s.changes.diff, None);
    }

    #[test]
    fn commit_button_rules() {
        let mut s = AppState::new(Config::default());
        s.changes.summary = "Fix".into();
        assert!(!s.changes.can_commit(), "nothing staged");
        s.apply(Event::StatusLoaded(vec![file(
            "a.txt",
            Some(Change::Modified),
            None,
        )]));
        assert!(s.changes.can_commit());
        s.changes.summary = "  ".into();
        assert!(!s.changes.can_commit(), "empty summary");
        s.apply(Event::StatusLoaded(vec![]));
        s.changes.summary = "Reword".into();
        s.changes.amend = true;
        assert!(s.changes.can_commit(), "amend needs no staged change");
        s.changes.committing = true;
        assert!(!s.changes.can_commit());
    }

    #[test]
    fn commit_message_joins_summary_and_description() {
        let mut c = retrogit::state::ChangesView {
            summary: " Fix bug ".into(),
            ..Default::default()
        };
        assert_eq!(c.commit_message(), "Fix bug");
        c.description = "\nWhy it broke\n".into();
        assert_eq!(c.commit_message(), "Fix bug\n\nWhy it broke");
    }

    #[test]
    fn committed_resets_the_form_and_warns_once_without_git() {
        let mut s = AppState::new(Config::default());
        s.changes.summary = "x".into();
        s.changes.description = "y".into();
        s.changes.amend = true;
        s.changes.committing = true;
        s.apply(Event::Committed(outcome(false)));
        let c = &s.changes;
        assert!(c.summary.is_empty() && c.description.is_empty() && !c.amend && !c.committing);
        assert_eq!(c.last_commit_note.as_deref(), Some("Committed a1b2c3d"));
        assert_eq!(s.messages.len(), 1);
        s.apply(Event::Committed(outcome(false)));
        assert_eq!(s.messages.len(), 1, "warning only once");
    }

    #[test]
    fn commit_error_keeps_the_message() {
        let mut s = AppState::new(Config::default());
        s.changes.summary = "keep me".into();
        s.changes.committing = true;
        s.apply(Event::Error {
            during: Op::Commit,
            error: err(),
        });
        assert!(!s.changes.committing);
        assert_eq!(s.changes.summary, "keep me");
    }

    #[test]
    fn amend_info_prefills_only_empty_fields() {
        let mut s = AppState::new(Config::default());
        s.changes.amend = true;
        s.apply(Event::AmendInfo {
            message: Some("Title\n\nBody text".into()),
            pushed: true,
        });
        assert_eq!(
            (s.changes.summary.as_str(), s.changes.description.as_str()),
            ("Title", "Body text")
        );
        assert!(s.changes.head_pushed);
        s.changes.summary = "Mine".into();
        s.apply(Event::AmendInfo {
            message: Some("Other".into()),
            pushed: false,
        });
        assert_eq!(s.changes.summary, "Mine");
    }

    #[test]
    fn opening_another_repo_resets_the_changes_view() {
        let mut s = AppState::new(Config::default());
        s.apply(Event::RepoOpened(summary("/tmp/a")));
        s.changes.summary = "draft".into();
        s.apply(Event::RepoOpened(summary("/tmp/a")));
        assert_eq!(s.changes.summary, "draft", "same repo keeps the draft");
        s.apply(Event::RepoOpened(summary("/tmp/b")));
        assert!(s.changes.summary.is_empty());
    }
}

mod discard {
    use super::*;
    use retrogit::protocol::Command;

    #[test]
    fn discard_is_sent_only_after_confirmation() {
        let mut s = AppState::new(Config::default());
        let cmd = Command::DiscardFiles(vec!["a.txt".into()]);
        s.changes
            .request_discard(cmd.clone(), "Discard a.txt?".into());
        assert_eq!(
            s.changes
                .pending_discard
                .as_ref()
                .map(|p| p.question.as_str()),
            Some("Discard a.txt?")
        );
        s.changes.cancel_discard();
        assert!(s.changes.pending_discard.is_none());
        s.changes
            .request_discard(cmd.clone(), "Discard a.txt?".into());
        assert_eq!(s.changes.confirm_discard(), Some(cmd));
        assert_eq!(
            s.changes.confirm_discard(),
            None,
            "a confirmation is used once"
        );
    }
}

mod recents {
    use super::*;

    #[test]
    fn recents_are_listed_alphabetically_and_opening_does_not_reorder() {
        let mut s = AppState::new(Config::default());
        for p in ["/w/zeta", "/w/Alpha", "/w/beta"] {
            s.apply(Event::RepoOpened(RepoSummary {
                name: p.rsplit('/').next().unwrap().into(),
                ..summary(p)
            }));
        }
        let names = |s: &AppState| {
            s.recents_sorted()
                .into_iter()
                .map(|r| r.name)
                .collect::<Vec<_>>()
        };
        assert_eq!(names(&s), ["Alpha", "beta", "zeta"]);
        s.apply(Event::RepoOpened(RepoSummary {
            name: "zeta".into(),
            ..summary("/w/zeta")
        }));
        assert_eq!(names(&s), ["Alpha", "beta", "zeta"]);
        assert_eq!(
            s.selected_recent(),
            Some(2),
            "the open repo is the highlighted row"
        );
    }

    #[test]
    fn same_name_is_ordered_by_path_and_no_repo_means_no_highlight() {
        let mut s = AppState::new(Config::default());
        s.config.add_recent("app", std::path::Path::new("/b/app"));
        s.config.add_recent("app", std::path::Path::new("/a/app"));
        let paths: Vec<_> = s.recents_sorted().into_iter().map(|r| r.path).collect();
        assert_eq!(paths, [PathBuf::from("/a/app"), PathBuf::from("/b/app")]);
        assert_eq!(s.selected_recent(), None);
    }
}

mod sync {
    use super::*;
    use gitcore::{Branch, LogEntry, PullOutcome};
    use retrogit::protocol::SyncOp;
    use retrogit::state::{LOG_PAGE, PendingDialog, Tab, branch_name_error};

    fn entry(id: &str, parents: &[&str]) -> LogEntry {
        LogEntry {
            id: id.into(),
            short_id: id.into(),
            parents: parents.iter().map(|p| p.to_string()).collect(),
            author: "Ada".into(),
            email: String::new(),
            time: 0,
            summary: id.into(),
            refs: vec![],
        }
    }

    #[test]
    fn history_pages_append_and_stale_pages_are_ignored() {
        let mut s = AppState::new(Config::default());
        let page1: Vec<_> = (0..LOG_PAGE)
            .map(|i| entry(&format!("c{i}"), &[&format!("c{}", i + 1)]))
            .collect();
        s.apply(Event::LogLoaded {
            skip: 0,
            entries: page1,
        });
        assert_eq!(s.history.entries.len(), LOG_PAGE);
        assert_eq!(s.history.graph.len(), LOG_PAGE);
        assert!(!s.history.end_reached && s.wants_more_history());
        s.apply(Event::LogLoaded {
            skip: 7,
            entries: vec![entry("x", &[])],
        });
        assert_eq!(
            s.history.entries.len(),
            LOG_PAGE,
            "a page for another offset is ignored"
        );
        s.apply(Event::LogLoaded {
            skip: LOG_PAGE,
            entries: vec![entry(&format!("c{LOG_PAGE}"), &[])],
        });
        assert_eq!(s.history.entries.len(), LOG_PAGE + 1);
        assert!(s.history.end_reached && !s.wants_more_history());
        s.apply(Event::LogLoaded {
            skip: 0,
            entries: vec![entry("new", &[])],
        });
        assert_eq!(s.history.entries.len(), 1, "a reload starts over");
    }

    #[test]
    fn commit_detail_is_kept_only_for_the_selected_commit() {
        let mut s = AppState::new(Config::default());
        s.select_commit("aaa");
        let detail = |id: &str| gitcore::CommitDetail {
            id: id.into(),
            short_id: id.into(),
            parents: vec![],
            author: String::new(),
            email: String::new(),
            time: 0,
            committer: String::new(),
            message: String::new(),
            files: vec![],
        };
        s.apply(Event::CommitLoaded(detail("bbb")));
        assert!(s.history.detail.is_none());
        s.apply(Event::CommitLoaded(detail("aaa")));
        assert!(s.history.detail.is_some());
        s.apply(Event::SignatureLoaded {
            id: "aaa".into(),
            status: gitcore::SignatureStatus::Unsigned,
        });
        assert_eq!(
            s.history.signature,
            Some(gitcore::SignatureStatus::Unsigned)
        );
        s.select_commit("ccc");
        assert!(s.history.detail.is_none() && s.history.signature.is_none());
    }

    #[test]
    fn sync_lifecycle_and_dialogs() {
        let mut s = AppState::new(Config::default());
        s.apply(Event::SyncStarted {
            op: SyncOp::Push,
            background: false,
        });
        assert_eq!(s.sync.running, Some(SyncOp::Push));
        s.apply(Event::SyncProgress(gitcore::NetProgress {
            phase: "Writing objects".into(),
            percent: Some(40),
        }));
        assert!(s.sync.progress.is_some());
        s.apply(Event::SyncFinished {
            op: SyncOp::Push,
            ok: false,
        });
        s.apply(Event::PushRejected { can_force: false });
        assert_eq!(s.sync.running, None);
        assert_eq!(
            s.dialog,
            Some(PendingDialog::PushRejected { can_force: false })
        );
        s.apply(Event::PushRejected { can_force: true });
        assert_eq!(
            s.dialog,
            Some(PendingDialog::PushRejected { can_force: true })
        );
        s.apply(Event::SyncFinished {
            op: SyncOp::Push,
            ok: true,
        });
        assert_eq!(s.sync.note.as_deref(), Some("Pushed"));

        s.apply(Event::Diverged {
            ahead: 2,
            behind: 3,
        });
        assert_eq!(
            s.dialog,
            Some(PendingDialog::Diverged {
                ahead: 2,
                behind: 3
            })
        );
        s.apply(Event::WouldOverwrite {
            branch: "b".into(),
            files: vec!["f".into()],
        });
        assert!(matches!(
            s.dialog,
            Some(PendingDialog::WouldOverwrite { .. })
        ));
        s.apply(Event::NotMerged("old".into()));
        assert_eq!(
            s.dialog,
            Some(PendingDialog::DeleteNotMerged { name: "old".into() })
        );
    }

    #[test]
    fn conflicts_after_pull_switch_to_changes_with_a_message() {
        let mut s = AppState::new(Config::default());
        s.tab = Tab::History;
        s.apply(Event::Pulled(PullOutcome::Conflicts));
        assert_eq!(s.tab, Tab::Changes);
        assert_eq!(s.messages.len(), 1);
        s.apply(Event::Pulled(PullOutcome::UpToDate));
        assert_eq!(s.sync.note.as_deref(), Some("Already up to date"));
    }

    #[test]
    fn a_sync_error_stops_the_progress() {
        let mut s = AppState::new(Config::default());
        s.apply(Event::SyncStarted {
            op: SyncOp::Fetch,
            background: true,
        });
        s.apply(Event::Error {
            during: Op::Sync,
            error: err(),
        });
        assert_eq!(s.sync.running, None);
    }

    #[test]
    fn opening_another_repo_resets_history_branches_and_dialogs() {
        let mut s = AppState::new(Config::default());
        s.apply(Event::RepoOpened(summary("/tmp/a")));
        s.apply(Event::LogLoaded {
            skip: 0,
            entries: vec![entry("x", &[])],
        });
        s.apply(Event::BranchesLoaded(vec![Branch {
            name: "main".into(),
            remote: false,
            is_head: true,
            upstream: None,
            ahead: 0,
            behind: 0,
        }]));
        s.dialog = Some(PendingDialog::ConfirmForcePush);
        s.apply(Event::RepoOpened(summary("/tmp/b")));
        assert!(s.history.entries.is_empty() && s.branches.is_empty() && s.dialog.is_none());
    }

    #[test]
    fn branch_names_follow_git_rules() {
        let existing = vec![Branch {
            name: "main".into(),
            remote: false,
            is_head: true,
            upstream: None,
            ahead: 0,
            behind: 0,
        }];
        for bad in [
            "", " ", "a b", "-x", "x/", "x.lock", "a..b", "a~1", "a^", "a:b", "a?", "a*", "a[b",
            "a\\b", "@", "x@{y", ".hidden", "a/.b", "a//b", "end.",
        ] {
            assert!(
                branch_name_error(bad, &existing).is_some(),
                "{bad:?} should be refused"
            );
        }
        assert!(
            branch_name_error("main", &existing)
                .unwrap()
                .contains("already exists")
        );
        for good in ["feature/login", "fix-42", "v1.2", "user/ada/test"] {
            assert_eq!(branch_name_error(good, &existing), None, "{good:?}");
        }
    }
}

#[test]
fn access_denied_explains_org_oauth_restrictions() {
    let e = AppError::from_git(&gitcore::GitError::AccessDenied(
        "remote: Repository not found.".into(),
    ));
    assert_eq!(e.message, retrogit::strings::ERR_ACCESS_DENIED);
    assert_eq!(e.detail.as_deref(), Some("remote: Repository not found."));
}

mod highlight_cache {
    use super::*;
    use gitcore::{FileDiff, Side};

    fn diff(path: &str, binary: bool) -> FileDiff {
        FileDiff {
            path: path.into(),
            side: Side::Unstaged,
            binary,
            hunks: vec![],
        }
    }

    #[test]
    fn colors_are_dropped_when_the_diff_changes_and_kept_otherwise() {
        let mut s = AppState::new(Config::default());
        s.changes.shown = Some(("a.rs".into(), Side::Unstaged));
        s.apply(Event::DiffLoaded(diff("a.rs", false)));
        s.changes.diff_colors = Some(Some(vec![]));
        s.apply(Event::DiffLoaded(diff("a.rs", false)));
        assert!(
            s.changes.diff_colors.is_some(),
            "same diff: keep the colors"
        );
        s.apply(Event::DiffLoaded(diff("a.rs", true)));
        assert!(s.changes.diff_colors.is_none(), "new diff: recompute");
    }

    #[test]
    fn commit_file_colors_follow_the_commit_file_diff() {
        let mut s = AppState::new(Config::default());
        s.select_commit("c1");
        s.history.detail_file = Some("a.rs".into());
        s.history.detail_colors = Some(None);
        s.apply(Event::CommitFileDiffLoaded {
            id: "c1".into(),
            diff: diff("a.rs", false),
        });
        assert!(s.history.detail_colors.is_none());
        s.history.detail_colors = Some(None);
        s.select_commit("c2");
        assert!(s.history.detail_colors.is_none());
    }
}
