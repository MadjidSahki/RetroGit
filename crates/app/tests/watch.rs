#![allow(clippy::unwrap_used)]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use std::path::Path;
use std::sync::Mutex;

use retrogit::watch::{Refresh, Watcher, is_relevant, touches_refs};

fn wait_for(count: &AtomicUsize, at_least: usize, timeout: Duration) -> bool {
    let end = Instant::now() + timeout;
    while Instant::now() < end {
        if count.load(Ordering::SeqCst) >= at_least {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

#[test]
fn a_burst_of_writes_is_reported_once_and_git_internals_are_ignored() {
    let d = tempfile::tempdir().unwrap();
    git2::Repository::init(d.path()).unwrap();
    // FSEvents can deliver the `git init` writes (.git/HEAD...) late: let them pass first.
    std::thread::sleep(Duration::from_millis(1000));
    let count = Arc::new(AtomicUsize::new(0));
    let c = count.clone();
    let _w = Watcher::start(d.path(), move |_| {
        c.fetch_add(1, Ordering::SeqCst);
    })
    .unwrap();
    std::thread::sleep(Duration::from_millis(700)); // let the watcher settle
    count.store(0, Ordering::SeqCst);

    std::fs::create_dir_all(d.path().join(".git/objects/aa")).unwrap();
    std::fs::write(d.path().join(".git/objects/aa/bb"), "x").unwrap();
    assert!(
        !wait_for(&count, 1, Duration::from_millis(900)),
        "git internals triggered a refresh"
    );

    for i in 0..5 {
        std::fs::write(d.path().join(format!("f{i}.txt")), "x").unwrap();
    }
    assert!(wait_for(&count, 1, Duration::from_secs(2)));
    std::thread::sleep(Duration::from_millis(600));
    assert_eq!(count.load(Ordering::SeqCst), 1);
}

#[test]
fn refs_merge_and_rebase_state_matter_inside_dot_git_but_logs_and_locks_do_not() {
    let root = Path::new("/r");
    for p in [
        "/r/.git/index",
        "/r/.git/HEAD",
        "/r/.git/packed-refs",
        "/r/.git/refs/heads/main",
        "/r/.git/refs/remotes/origin/main",
        "/r/.git/refs/tags/v1",
        "/r/.git/MERGE_HEAD",
        "/r/.git/CHERRY_PICK_HEAD",
        "/r/.git/REVERT_HEAD",
        "/r/.git/rebase-merge/done",
        "/r/.git/rebase-apply/next",
        "/r/src/main.rs",
    ] {
        assert!(is_relevant(root, Path::new(p)), "{p} should be relevant");
    }
    for p in [
        "/r/.git/logs/HEAD",
        "/r/.git/logs/refs/heads/main",
        "/r/.git/FETCH_HEAD",
        "/r/.git/ORIG_HEAD",
        "/r/.git/objects/ab/cdef",
        "/r/.git/index.lock",
        "/r/.git/HEAD.lock",
        "/r/.git/packed-refs.lock",
        "/r/.git/refs/heads/main.lock",
        "/r/.git/config",
        "/r/.git/refs",
        "/r/.git/rebase-merge",
        "/elsewhere/.git/refs/heads/main",
    ] {
        assert!(!is_relevant(root, Path::new(p)), "{p} should be ignored");
    }
}

#[test]
fn head_packed_refs_and_refs_are_ref_changes_the_rest_is_work() {
    let root = Path::new("/r");
    for p in [
        "/r/.git/HEAD",
        "/r/.git/packed-refs",
        "/r/.git/refs/heads/main",
        "/r/.git/refs/remotes/origin/main",
    ] {
        assert!(touches_refs(root, Path::new(p)), "{p} touches refs");
    }
    for p in [
        "/r/.git/index",
        "/r/.git/MERGE_HEAD",
        "/r/.git/rebase-merge/done",
        "/r/refs/heads/main",
        "/r/HEAD",
        "/r/src/main.rs",
        "/elsewhere/.git/HEAD",
    ] {
        assert!(!touches_refs(root, Path::new(p)), "{p} is work");
    }
}

#[test]
fn ref_lock_files_are_not_ref_changes() {
    let root = Path::new("/r");
    for p in [
        "/r/.git/refs/heads/main.lock",
        "/r/.git/HEAD.lock",
        "/r/.git/packed-refs.lock",
    ] {
        assert!(!touches_refs(root, Path::new(p)), "{p} is a lock");
    }
}

#[test]
fn a_ref_written_outside_the_app_asks_for_a_refs_refresh() {
    let d = tempfile::tempdir().unwrap();
    git2::Repository::init(d.path()).unwrap();
    std::thread::sleep(Duration::from_millis(1000));
    let seen = Arc::new(Mutex::new(Vec::<Refresh>::new()));
    let s = seen.clone();
    let count = Arc::new(AtomicUsize::new(0));
    let c = count.clone();
    let _w = Watcher::start(d.path(), move |kind| {
        s.lock().unwrap().push(kind);
        c.fetch_add(1, Ordering::SeqCst);
    })
    .unwrap();
    std::thread::sleep(Duration::from_millis(700));
    count.store(0, Ordering::SeqCst);
    seen.lock().unwrap().clear();

    std::fs::write(d.path().join("work.txt"), "x").unwrap();
    assert!(wait_for(&count, 1, Duration::from_secs(2)));
    std::thread::sleep(Duration::from_millis(600));
    assert_eq!(*seen.lock().unwrap(), vec![Refresh::Status]);
    count.store(0, Ordering::SeqCst);
    seen.lock().unwrap().clear();

    std::fs::create_dir_all(d.path().join(".git/refs/heads")).unwrap();
    std::fs::write(d.path().join(".git/refs/heads/x"), "0".repeat(40)).unwrap();
    std::fs::write(d.path().join("other.txt"), "x").unwrap();
    assert!(wait_for(&count, 1, Duration::from_secs(2)));
    std::thread::sleep(Duration::from_millis(600));
    assert_eq!(*seen.lock().unwrap(), vec![Refresh::Refs]);
}
