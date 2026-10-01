#![allow(clippy::unwrap_used)]
mod common;

use gitcore::{GitError, Head, Repo};

#[test]
fn open_rejects_plain_folder() {
    let d = tempfile::tempdir().unwrap();
    let err = Repo::open(d.path()).err().unwrap();
    assert_eq!(err, GitError::NotARepository(d.path().to_path_buf()));
}

#[test]
fn open_does_not_search_parent_directories() {
    let d = tempfile::tempdir().unwrap();
    common::make_repo(d.path(), 1);
    let sub = d.path().join("sub");
    std::fs::create_dir(&sub).unwrap();
    assert!(matches!(Repo::open(&sub), Err(GitError::NotARepository(_))));
}

#[test]
fn summary_of_repo_with_commits() {
    let d = tempfile::tempdir().unwrap();
    let r = common::make_repo(d.path(), 2);
    r.remote("origin", "https://github.com/ada/demo.git")
        .unwrap();
    let s = Repo::open(d.path()).unwrap().summary().unwrap();
    assert_eq!(s.head, Head::Branch("main".into()));
    assert_eq!(
        s.origin_url.as_deref(),
        Some("https://github.com/ada/demo.git")
    );
    let c = s.last_commit.unwrap();
    assert_eq!(c.summary, "commit 1");
    assert_eq!(c.author, "Ada");
    assert_eq!(c.short_id.len(), 7);
    assert_eq!(c.time, 1_700_000_000);
    assert_eq!(s.name, d.path().file_name().unwrap().to_string_lossy());
}

#[test]
fn summary_of_empty_repo_is_unborn() {
    let d = tempfile::tempdir().unwrap();
    common::make_repo(d.path(), 0);
    let s = Repo::open(d.path()).unwrap().summary().unwrap();
    assert_eq!(s.head, Head::Unborn("main".into()));
    assert_eq!(s.last_commit, None);
    assert_eq!(s.origin_url, None);
}

#[test]
fn summary_of_detached_head() {
    let d = tempfile::tempdir().unwrap();
    let r = common::make_repo(d.path(), 2);
    let id = r.head().unwrap().peel_to_commit().unwrap().id();
    r.set_head_detached(id).unwrap();
    let s = Repo::open(d.path()).unwrap().summary().unwrap();
    assert_eq!(s.head, Head::Detached(id.to_string()[..7].to_string()));
}

#[test]
fn discover_finds_the_root_from_a_subfolder() {
    let d = tempfile::tempdir().unwrap();
    common::make_repo(d.path(), 1);
    let sub = d.path().join("a/b");
    std::fs::create_dir_all(&sub).unwrap();
    let root = Repo::discover(&sub).unwrap();
    assert_eq!(
        root.canonicalize().unwrap(),
        d.path().canonicalize().unwrap()
    );
    let outside = tempfile::tempdir().unwrap();
    assert!(matches!(
        Repo::discover(outside.path()),
        Err(GitError::NotARepository(_))
    ));
}
