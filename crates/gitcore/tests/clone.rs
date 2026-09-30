#![allow(clippy::unwrap_used)]
mod common;

use std::sync::atomic::{AtomicBool, Ordering};

use gitcore::{CloneProgress, CloneRequest, GitError, Head, clone};

fn request(src: &std::path::Path, dest: std::path::PathBuf) -> CloneRequest {
    CloneRequest {
        url: common::file_url(src),
        dest,
        credentials: None,
    }
}

#[test]
fn clone_succeeds_and_reports_progress() {
    let src = tempfile::tempdir().unwrap();
    common::make_repo(src.path(), 3);
    let out = tempfile::tempdir().unwrap();
    let dest = out.path().join("demo");
    let mut last = CloneProgress::default();
    let mut calls = 0;
    let repo = clone(
        &request(src.path(), dest.clone()),
        |p| {
            calls += 1;
            last = p;
        },
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(calls > 0);
    assert!(last.total_objects > 0);
    assert_eq!(last.received_objects, last.total_objects);
    assert!((last.fraction() - 1.0).abs() < f32::EPSILON);
    assert_eq!(repo.summary().unwrap().head, Head::Branch("main".into()));
    assert!(dest.join("file2.txt").exists());
}

#[test]
fn clone_into_existing_empty_dir_is_allowed() {
    let src = tempfile::tempdir().unwrap();
    common::make_repo(src.path(), 1);
    let dest = tempfile::tempdir().unwrap();
    clone(
        &request(src.path(), dest.path().to_path_buf()),
        |_| {},
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(dest.path().join(".git").exists());
}

#[test]
fn clone_refuses_non_empty_destination_and_leaves_it_untouched() {
    let src = tempfile::tempdir().unwrap();
    common::make_repo(src.path(), 1);
    let dest = tempfile::tempdir().unwrap();
    std::fs::write(dest.path().join("keep.txt"), "mine").unwrap();
    let err = clone(
        &request(src.path(), dest.path().to_path_buf()),
        |_| {},
        &AtomicBool::new(false),
    )
    .err()
    .unwrap();
    assert_eq!(
        err,
        GitError::DestinationNotEmpty(dest.path().to_path_buf())
    );
    assert_eq!(
        std::fs::read_to_string(dest.path().join("keep.txt")).unwrap(),
        "mine"
    );
}

#[test]
fn cancelled_clone_removes_the_directory_it_created() {
    let src = tempfile::tempdir().unwrap();
    common::make_repo(src.path(), 3);
    let out = tempfile::tempdir().unwrap();
    let dest = out.path().join("demo");
    let cancel = AtomicBool::new(false);
    let err = clone(
        &request(src.path(), dest.clone()),
        |_| cancel.store(true, Ordering::Relaxed),
        &cancel,
    )
    .err()
    .unwrap();
    assert_eq!(err, GitError::Cancelled);
    assert!(!dest.exists());
}

#[test]
fn cancelled_clone_into_existing_empty_dir_keeps_the_dir_but_empties_it() {
    let src = tempfile::tempdir().unwrap();
    common::make_repo(src.path(), 3);
    let dest = tempfile::tempdir().unwrap();
    let cancel = AtomicBool::new(false);
    let err = clone(
        &request(src.path(), dest.path().to_path_buf()),
        |_| cancel.store(true, Ordering::Relaxed),
        &cancel,
    )
    .err()
    .unwrap();
    assert_eq!(err, GitError::Cancelled);
    assert!(dest.path().exists());
    assert_eq!(std::fs::read_dir(dest.path()).unwrap().count(), 0);
}

#[test]
fn clone_of_missing_source_fails_and_cleans_up() {
    let out = tempfile::tempdir().unwrap();
    let dest = out.path().join("demo");
    let missing = out.path().join("does-not-exist");
    let err = clone(
        &request(&missing, dest.clone()),
        |_| {},
        &AtomicBool::new(false),
    )
    .err()
    .unwrap();
    assert!(
        matches!(err, GitError::Other(_) | GitError::Network(_)),
        "{err:?}"
    );
    assert!(!dest.exists());
}

#[test]
fn credentials_debug_never_shows_password() {
    let c = gitcore::Credentials {
        username: "x-access-token".into(),
        password: "gho_secret".into(),
    };
    assert!(!format!("{c:?}").contains("gho_secret"));
}

#[test]
fn fraction_handles_zero_totals() {
    assert_eq!(CloneProgress::default().fraction(), 0.0);
}

#[test]
fn rejected_credentials_fail_fast_with_auth_error_and_clean_up() {
    // An HTTPS remote that always answers 401, like GitHub for a token not authorized for SSO.
    let mut server = mockito::Server::new();
    let m = server
        .mock("GET", mockito::Matcher::Any)
        .with_status(401)
        .with_header("WWW-Authenticate", "Basic realm=\"GitHub\"")
        .expect_at_most(4)
        .create();
    let out = tempfile::tempdir().unwrap();
    let dest = out.path().join("demo");
    let req = CloneRequest {
        url: format!("{}/org/demo.git", server.url()),
        dest: dest.clone(),
        credentials: Some(gitcore::Credentials {
            username: "x-access-token".into(),
            password: "gho_x".into(),
        }),
    };
    let err = clone(&req, |_| {}, &AtomicBool::new(false)).err().unwrap();
    assert!(matches!(err, GitError::Auth(_)), "{err:?}");
    assert!(!dest.exists());
    m.assert();
}

#[test]
fn checkout_progress_is_reported() {
    let src = tempfile::tempdir().unwrap();
    common::make_repo(src.path(), 5);
    let out = tempfile::tempdir().unwrap();
    let mut last = CloneProgress::default();
    clone(
        &request(src.path(), out.path().join("demo")),
        |p| last = p,
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(last.checkout_total >= 5, "{last:?}");
    assert_eq!(last.checkout_done, last.checkout_total);
}

#[test]
fn cancel_during_checkout_reports_cancelled_and_cleans_up() {
    let src = tempfile::tempdir().unwrap();
    common::make_repo(src.path(), 30);
    let out = tempfile::tempdir().unwrap();
    let dest = out.path().join("demo");
    let cancel = AtomicBool::new(false);
    let mut total = 0;
    let err = clone(
        &request(src.path(), dest.clone()),
        |p| {
            if p.checkout_total > 0 {
                total = p.checkout_total;
                cancel.store(true, Ordering::Relaxed);
            }
        },
        &cancel,
    )
    .err()
    .unwrap();
    assert_eq!(err, GitError::Cancelled);
    assert!(!dest.exists());
    // The cancel happened during checkout (libgit2 cannot abort a checkout mid-way, so the
    // clone is rolled back right after it).
    assert!(total >= 30);
}

#[test]
fn network_timeouts_can_be_configured() {
    gitcore::configure_network_timeouts().unwrap();
}
