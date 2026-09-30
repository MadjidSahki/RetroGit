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
