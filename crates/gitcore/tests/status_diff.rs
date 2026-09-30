#![allow(clippy::unwrap_used)]
mod common;

use gitcore::{Change, FileStatus, LineKind, Repo, Side};

fn st(path: &str, staged: Option<Change>, unstaged: Option<Change>) -> FileStatus {
    FileStatus {
        path: path.into(),
        staged,
        unstaged,
    }
}

#[test]
fn status_reports_every_kind_of_change_sorted() {
    let d = tempfile::tempdir().unwrap();
    let repo = common::make_repo(d.path(), 3); // file0.txt, file1.txt, file2.txt
    std::fs::write(d.path().join(".gitignore"), "*.log\n").unwrap();
    std::fs::write(d.path().join("debug.log"), "ignored").unwrap();
    std::fs::write(d.path().join("file0.txt"), "changed\n").unwrap(); // unstaged modify
    std::fs::remove_file(d.path().join("file1.txt")).unwrap(); // unstaged delete
    std::fs::create_dir(d.path().join("dir")).unwrap();
    std::fs::write(d.path().join("dir/new.txt"), "x\n").unwrap(); // untracked in new dir
    std::fs::write(d.path().join("added.txt"), "a\n").unwrap();
    let mut idx = repo.index().unwrap();
    idx.add_path(std::path::Path::new("added.txt")).unwrap(); // staged add
    idx.write().unwrap();
    std::fs::write(d.path().join("added.txt"), "a\nb\n").unwrap(); // + unstaged modify

    let s = Repo::open(d.path()).unwrap().status().unwrap();
    assert_eq!(
        s,
        vec![
            st(".gitignore", None, Some(Change::Untracked)),
            st("added.txt", Some(Change::Added), Some(Change::Modified)),
            st("dir/new.txt", None, Some(Change::Untracked)),
            st("file0.txt", None, Some(Change::Modified)),
            st("file1.txt", None, Some(Change::Deleted)),
        ]
    );
}

#[test]
fn staged_rename_is_detected() {
    let d = tempfile::tempdir().unwrap();
    let repo = common::make_repo(d.path(), 1);
    std::fs::rename(d.path().join("file0.txt"), d.path().join("moved.txt")).unwrap();
    let mut idx = repo.index().unwrap();
    idx.remove_path(std::path::Path::new("file0.txt")).unwrap();
    idx.add_path(std::path::Path::new("moved.txt")).unwrap();
    idx.write().unwrap();
    let s = Repo::open(d.path()).unwrap().status().unwrap();
    assert_eq!(
        s,
        vec![st(
            "moved.txt",
            Some(Change::Renamed {
                from: "file0.txt".into()
            }),
            None
        )]
    );
}

#[test]
fn clean_repo_has_empty_status() {
    let d = tempfile::tempdir().unwrap();
    common::make_repo(d.path(), 2);
    assert!(Repo::open(d.path()).unwrap().status().unwrap().is_empty());
}

#[test]
fn unstaged_and_staged_diffs() {
    let d = tempfile::tempdir().unwrap();
    let repo = common::make_repo(d.path(), 1);
    let original = std::fs::read_to_string(d.path().join("file0.txt")).unwrap();
    std::fs::write(d.path().join("file0.txt"), format!("top\n{original}")).unwrap();
    let r = Repo::open(d.path()).unwrap();
    let unstaged = r.diff_file("file0.txt", Side::Unstaged).unwrap();
    assert!(!unstaged.binary);
    assert_eq!(unstaged.side, Side::Unstaged);
    let first_change = unstaged.hunks[0]
        .lines
        .iter()
        .find(|l| l.kind != LineKind::Context)
        .unwrap();
    assert_eq!(
        (
            first_change.kind,
            first_change.text.as_str(),
            first_change.new_no
        ),
        (LineKind::Added, "top\n", Some(1))
    );
    assert!(
        r.diff_file("file0.txt", Side::Staged)
            .unwrap()
            .hunks
            .is_empty()
    );

    let mut idx = repo.index().unwrap();
    idx.add_path(std::path::Path::new("file0.txt")).unwrap();
    idx.write().unwrap();
    assert!(
        r.diff_file("file0.txt", Side::Unstaged)
            .unwrap()
            .hunks
            .is_empty()
    );
    assert_eq!(
        r.diff_file("file0.txt", Side::Staged).unwrap().hunks.len(),
        1
    );
}

#[test]
fn untracked_file_diffs_as_all_added() {
    let d = tempfile::tempdir().unwrap();
    common::make_repo(d.path(), 1);
    std::fs::write(d.path().join("n.txt"), "1\n2").unwrap();
    let diff = Repo::open(d.path())
        .unwrap()
        .diff_file("n.txt", Side::Unstaged)
        .unwrap();
    let lines = &diff.hunks[0].lines;
    assert_eq!(lines.len(), 2);
    assert!(lines.iter().all(|l| l.kind == LineKind::Added));
    assert!(lines[1].no_newline_at_eof && !lines[0].no_newline_at_eof);
}

#[test]
fn staged_diff_works_without_any_commit() {
    let d = tempfile::tempdir().unwrap();
    let repo = common::make_repo(d.path(), 0);
    std::fs::write(d.path().join("a.txt"), "a\n").unwrap();
    let mut idx = repo.index().unwrap();
    idx.add_path(std::path::Path::new("a.txt")).unwrap();
    idx.write().unwrap();
    let diff = Repo::open(d.path())
        .unwrap()
        .diff_file("a.txt", Side::Staged)
        .unwrap();
    assert_eq!(diff.hunks[0].lines[0].kind, LineKind::Added);
}

#[test]
fn binary_file_is_flagged() {
    let d = tempfile::tempdir().unwrap();
    common::make_repo(d.path(), 1);
    std::fs::write(d.path().join("img.bin"), [0u8, 159, 146, 150, 0, 1]).unwrap();
    let diff = Repo::open(d.path())
        .unwrap()
        .diff_file("img.bin", Side::Unstaged)
        .unwrap();
    assert!(diff.binary);
    assert!(diff.hunks.is_empty());
}
