#![allow(clippy::unwrap_used)]
mod common;

use std::path::Path;

use gitcore::{GitError, LineKind, Refusal, Repo, Selection, Side, WholeAction, WholeKind};

/// `f.txt` committed with `old`, working tree holds `new`.
fn setup(old: &[u8], new: &[u8]) -> (tempfile::TempDir, Repo) {
    let d = tempfile::tempdir().unwrap();
    let repo = common::make_repo(d.path(), 1);
    std::fs::write(d.path().join("f.txt"), old).unwrap();
    let mut idx = repo.index().unwrap();
    idx.add_path(Path::new("f.txt")).unwrap();
    idx.write().unwrap();
    let tree = repo.find_tree(idx.write_tree().unwrap()).unwrap();
    let sig = git2::Signature::now("Ada", "ada@example.com").unwrap();
    let parent = repo.head().unwrap().peel_to_commit().unwrap();
    repo.commit(Some("HEAD"), &sig, &sig, "base", &tree, &[&parent])
        .unwrap();
    std::fs::write(d.path().join("f.txt"), new).unwrap();
    let r = Repo::open(d.path()).unwrap();
    (d, r)
}

fn read(d: &Path, name: &str) -> Vec<u8> {
    std::fs::read(d.join(name)).unwrap()
}

/// Select the changed lines whose text (without line ending) is in `texts`.
fn lines(r: &Repo, texts: &[&str]) -> Selection {
    let diff = r.diff_file("f.txt", Side::Unstaged).unwrap();
    let mut out = Vec::new();
    for (h, hunk) in diff.hunks.iter().enumerate() {
        for (i, l) in hunk.lines.iter().enumerate() {
            if l.kind != LineKind::Context && texts.contains(&l.text.trim_end_matches(['\n', '\r']))
            {
                out.push((h, i));
            }
        }
    }
    Selection::Lines(out)
}

#[test]
fn discard_one_added_line_keeps_the_others() {
    let (d, r) = setup(b"a\nb\n", b"a\nX\nb\nY\n");
    r.discard("f.txt", &lines(&r, &["X"]), None).unwrap();
    assert_eq!(read(d.path(), "f.txt"), b"a\nb\nY\n");
}

#[test]
fn discard_a_removed_line_puts_it_back() {
    let (d, r) = setup(b"a\nb\nc\n", b"a\nc\n");
    r.discard("f.txt", &lines(&r, &["b"]), None).unwrap();
    assert_eq!(read(d.path(), "f.txt"), b"a\nb\nc\n");
}

#[test]
fn discard_a_hunk_leaves_other_hunks() {
    let old: String = (1..=30).map(|i| format!("l{i}\n")).collect();
    let new = old.replace("l2\n", "l2 x\n").replace("l28\n", "l28 y\n");
    let (d, r) = setup(old.as_bytes(), new.as_bytes());
    r.discard("f.txt", &Selection::Hunks(vec![0]), None)
        .unwrap();
    assert_eq!(
        String::from_utf8(read(d.path(), "f.txt")).unwrap(),
        old.replace("l28\n", "l28 y\n")
    );
}

#[test]
fn discard_restores_the_staged_version_not_head() {
    let (d, r) = setup(b"a\n", b"a\nstaged\n");
    r.stage("f.txt", &Selection::All, None).unwrap();
    std::fs::write(d.path().join("f.txt"), b"a\nstaged\nunstaged\n").unwrap();
    r.discard_files(&["f.txt"]).unwrap();
    assert_eq!(read(d.path(), "f.txt"), b"a\nstaged\n");
    let status = r.status().unwrap();
    assert!(status[0].staged.is_some() && status[0].unstaged.is_none());
}

#[test]
fn discard_whole_file_restores_a_deleted_file() {
    let (d, r) = setup(b"a\n", b"a\n");
    std::fs::remove_file(d.path().join("f.txt")).unwrap();
    r.discard_files(&["f.txt"]).unwrap();
    assert_eq!(read(d.path(), "f.txt"), b"a\n");
}

#[test]
fn discarding_an_untracked_file_moves_it_to_the_trash() {
    let (d, r) = setup(b"a\n", b"a\n");
    let name = format!("retrogit-test-{}.txt", std::process::id());
    std::fs::write(d.path().join(&name), b"scratch\n").unwrap();
    r.discard_files(&[name.as_str()]).unwrap();
    assert!(!d.path().join(&name).exists());
}

#[test]
fn discard_keeps_a_missing_final_newline_and_crlf() {
    let (d, r) = setup(b"a\nb", b"a\nX\nb");
    r.discard("f.txt", &lines(&r, &["X"]), None).unwrap();
    assert_eq!(read(d.path(), "f.txt"), b"a\nb");

    let (d, r) = setup(b"a\r\nb\r\n", b"a\r\nb\r\nc\r\n");
    r.discard("f.txt", &lines(&r, &["c"]), None).unwrap();
    assert_eq!(read(d.path(), "f.txt"), b"a\r\nb\r\n");
}

#[test]
fn discard_puts_back_crlf_lines_in_an_autocrlf_checkout() {
    // Index holds LF; the working tree is CRLF (core.autocrlf=true, like Windows).
    let (d, r) = setup(b"a\nb\nc\n", b"a\r\nc\r\n");
    git2::Repository::open(d.path())
        .unwrap()
        .config()
        .unwrap()
        .set_str("core.autocrlf", "true")
        .unwrap();
    let r2 = Repo::open(d.path()).unwrap();
    r2.discard("f.txt", &lines(&r2, &["b"]), None).unwrap();
    assert_eq!(read(d.path(), "f.txt"), b"a\r\nb\r\nc\r\n");
    drop(r);
}

#[test]
fn discard_keeps_the_executable_bit() {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let (d, r) = setup(b"#!/bin/sh\n", b"#!/bin/sh\necho x\n");
        let p = d.path().join("f.txt");
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        r.discard("f.txt", &Selection::Hunks(vec![0]), None)
            .unwrap();
        assert_eq!(
            std::fs::metadata(&p).unwrap().permissions().mode() & 0o777,
            0o755
        );
    }
}

#[test]
fn stale_or_unsupported_discards_write_nothing() {
    let (d, r) = setup(b"a\n", b"a\nX\n");
    let shown = r.diff_file("f.txt", Side::Unstaged).unwrap();
    std::fs::write(d.path().join("f.txt"), b"a\nX\nY\n").unwrap();
    assert_eq!(
        r.discard("f.txt", &Selection::Lines(vec![(0, 1)]), Some(&shown))
            .err(),
        Some(GitError::StaleSelection)
    );
    assert_eq!(read(d.path(), "f.txt"), b"a\nX\nY\n");

    std::fs::remove_file(d.path().join("f.txt")).unwrap();
    assert!(matches!(
        r.discard("f.txt", &Selection::Hunks(vec![0]), None),
        Err(GitError::Refused(Refusal::WholeFileOnly {
            kind: WholeKind::Deleted,
            action: WholeAction::Restore
        }))
    ));
    // A tracked binary file (an untracked one is refused as untracked first).
    let (_bin, r) = setup(&[0u8, 1, 0, 2], &[0u8, 3, 0, 2]);
    assert!(matches!(
        r.discard("f.txt", &Selection::Hunks(vec![0]), None),
        Err(GitError::Refused(Refusal::WholeFileOnly {
            kind: WholeKind::Binary,
            action: WholeAction::Discard
        }))
    ));
}

#[test]
fn partial_discard_of_an_untracked_file_is_refused_and_keeps_it() {
    let (d, r) = setup(b"a\n", b"a\n");
    std::fs::write(d.path().join("notes.txt"), b"one\ntwo\n").unwrap();
    let err = r
        .discard("notes.txt", &Selection::Hunks(vec![0]), None)
        .err();
    assert!(
        matches!(
            err,
            Some(GitError::Refused(Refusal::WholeFileOnly {
                kind: WholeKind::Untracked,
                action: WholeAction::Discard
            }))
        ),
        "{err:?}"
    );
    assert_eq!(read(d.path(), "notes.txt"), b"one\ntwo\n");
}

#[test]
fn libgit2_discard_treats_paths_literally() {
    let d = tempfile::tempdir().unwrap();
    let repo = common::make_repo(d.path(), 1);
    for name in ["[id].tsx", "i.tsx"] {
        std::fs::write(d.path().join(name), b"v1\n").unwrap();
    }
    let mut idx = repo.index().unwrap();
    idx.add_all(["*"], git2::IndexAddOption::DEFAULT, None)
        .unwrap();
    idx.write().unwrap();
    for name in ["[id].tsx", "i.tsx"] {
        std::fs::write(d.path().join(name), b"v2\n").unwrap();
    }
    let r = Repo::open(d.path()).unwrap();
    r.discard_files_git2(&["[id].tsx"]).unwrap();
    assert_eq!(read(d.path(), "[id].tsx"), b"v1\n");
    assert_eq!(
        read(d.path(), "i.tsx"),
        b"v2\n",
        "a glob must not match other files"
    );
}

#[cfg(unix)]
#[test]
fn partial_discard_of_a_symlink_is_refused() {
    let (d, r) = setup(b"target line\n", b"target line\n");
    let repo = git2::Repository::open(d.path()).unwrap();
    std::os::unix::fs::symlink("f.txt", d.path().join("link")).unwrap();
    let mut idx = repo.index().unwrap();
    idx.add_path(Path::new("link")).unwrap();
    idx.write().unwrap();
    std::fs::remove_file(d.path().join("link")).unwrap();
    std::os::unix::fs::symlink("elsewhere.txt", d.path().join("link")).unwrap();
    let err = r.discard("link", &Selection::Hunks(vec![0]), None).err();
    assert!(
        matches!(
            err,
            Some(GitError::Refused(Refusal::WholeFileOnly {
                kind: WholeKind::Symlink,
                action: WholeAction::Discard
            }))
        ),
        "{err:?}"
    );
    assert!(
        std::fs::symlink_metadata(d.path().join("link"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
}
