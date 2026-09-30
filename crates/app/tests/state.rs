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
