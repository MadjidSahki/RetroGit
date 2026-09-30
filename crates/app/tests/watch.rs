#![allow(clippy::unwrap_used)]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use retrogit::watch::Watcher;

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
    let count = Arc::new(AtomicUsize::new(0));
    let c = count.clone();
    let _w = Watcher::start(d.path(), move || {
        c.fetch_add(1, Ordering::SeqCst);
    })
    .unwrap();
    std::thread::sleep(Duration::from_millis(300)); // let FSEvents settle

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
