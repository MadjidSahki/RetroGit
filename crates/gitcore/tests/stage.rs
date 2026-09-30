#![allow(clippy::unwrap_used)]
mod common;

use std::path::Path;
use std::process::Command;

use gitcore::{Direction, GitError, LineKind, Repo, Selection, Side, apply_selection};

/// Repo whose index (and HEAD) holds `old` in `f.txt` and whose working tree holds `new`.
fn setup(old: &str, new: &str) -> (tempfile::TempDir, Repo) {
    let d = tempfile::tempdir().unwrap();
    let repo = common::make_repo(d.path(), 1);
    std::fs::write(d.path().join("f.txt"), old).unwrap();
    commit_all(&repo, "base");
    std::fs::write(d.path().join("f.txt"), new).unwrap();
    let r = Repo::open(d.path()).unwrap();
    (d, r)
}

fn commit_all(repo: &git2::Repository, msg: &str) {
    let mut idx = repo.index().unwrap();
    idx.add_all(["*"], git2::IndexAddOption::DEFAULT, None)
        .unwrap();
    idx.write().unwrap();
    let tree = repo.find_tree(idx.write_tree().unwrap()).unwrap();
    let sig = git2::Signature::now("Ada", "ada@example.com").unwrap();
    let parent = repo.head().unwrap().peel_to_commit().unwrap();
    repo.commit(Some("HEAD"), &sig, &sig, msg, &tree, &[&parent])
        .unwrap();
}

fn index_content(dir: &Path, path: &str) -> Option<String> {
    let repo = git2::Repository::open(dir).unwrap();
    let idx = repo.index().unwrap();
    let e = idx.get_path(Path::new(path), 0)?;
    Some(String::from_utf8(repo.find_blob(e.id).unwrap().content().to_vec()).unwrap())
}

/// Positions of the added/removed lines, in diff order.
fn changes(r: &Repo, side: Side) -> Vec<(usize, usize, LineKind, String)> {
    let d = r.diff_file("f.txt", side).unwrap();
    let mut out = Vec::new();
    for (h, hunk) in d.hunks.iter().enumerate() {
        for (i, l) in hunk.lines.iter().enumerate() {
            if l.kind != LineKind::Context {
                out.push((h, i, l.kind, l.text.clone()));
            }
        }
    }
    out
}

fn pick(r: &Repo, side: Side, texts: &[&str]) -> Selection {
    Selection::Lines(
        changes(r, side)
            .into_iter()
            .filter(|(_, _, kind, t)| {
                let sign = if *kind == LineKind::Added { "+" } else { "-" };
                texts.contains(&format!("{sign}{}", t.trim_end_matches('\n')).as_str())
            })
            .map(|(h, i, _, _)| (h, i))
            .collect(),
    )
}

#[test]
fn stage_a_single_added_line() {
    let (d, r) = setup("a\nb\nc\n", "a\nX\nb\nY\nc\n");
    r.stage("f.txt", &pick(&r, Side::Unstaged, &["+Y"]), None)
        .unwrap();
    assert_eq!(index_content(d.path(), "f.txt").unwrap(), "a\nb\nY\nc\n");
}

#[test]
fn stage_a_single_removed_line_keeps_the_other_removal() {
    let (d, r) = setup("a\nb\nc\nd\n", "a\nd\n");
    r.stage("f.txt", &pick(&r, Side::Unstaged, &["-c"]), None)
        .unwrap();
    assert_eq!(index_content(d.path(), "f.txt").unwrap(), "a\nb\nd\n");
}

#[test]
fn stage_half_of_a_modification() {
    // A modified line is a "-" plus a "+": staging only the "+" duplicates, only "-" deletes.
    let (d, r) = setup("a\nold\nz\n", "a\nnew\nz\n");
    r.stage("f.txt", &pick(&r, Side::Unstaged, &["+new"]), None)
        .unwrap();
    assert_eq!(
        index_content(d.path(), "f.txt").unwrap(),
        "a\nold\nnew\nz\n"
    );
}

#[test]
fn stage_one_hunk_of_two_matches_git_apply_cached() {
    let old: String = (1..=30).map(|i| format!("line{i}\n")).collect();
    let new = old
        .replace("line3\n", "line3 changed\n")
        .replace("line27\n", "line27 changed\n");
    for hunk in 0..2 {
        let (d, r) = setup(&old, &new);
        assert_eq!(r.diff_file("f.txt", Side::Unstaged).unwrap().hunks.len(), 2);
        r.stage("f.txt", &Selection::Hunks(vec![hunk]), None)
            .unwrap();
        let ours = index_content(d.path(), "f.txt").unwrap();
        let expected = git_apply_hunk(d.path(), hunk);
        if let Some(expected) = expected {
            assert_eq!(ours, expected, "hunk {hunk}");
        }
    }
}

/// What `git apply --cached` gives for hunk `n` of `git diff f.txt` (None if git is absent).
fn git_apply_hunk(dir: &Path, n: usize) -> Option<String> {
    let copy = tempfile::tempdir().unwrap();
    let status = Command::new("git")
        .arg("clone")
        .arg("-q")
        .arg(dir)
        .arg(copy.path())
        .status()
        .ok()?;
    assert!(status.success());
    std::fs::copy(dir.join("f.txt"), copy.path().join("f.txt")).unwrap();
    let out = Command::new("git")
        .current_dir(copy.path())
        .args(["diff", "-U3", "f.txt"])
        .output()
        .unwrap();
    let patch = String::from_utf8(out.stdout).unwrap();
    let (header, hunks) = split_patch(&patch);
    let mut one = header;
    one.push_str(&hunks[n]);
    let mut child = Command::new("git")
        .current_dir(copy.path())
        .args(["apply", "--cached", "-"])
        .stdin(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    use std::io::Write;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(one.as_bytes())
        .unwrap();
    assert!(child.wait().unwrap().success());
    Some(index_content(copy.path(), "f.txt").unwrap())
}

fn split_patch(patch: &str) -> (String, Vec<String>) {
    let mut header = String::new();
    let mut hunks: Vec<String> = Vec::new();
    for line in patch.split_inclusive('\n') {
        if line.starts_with("@@") {
            hunks.push(String::new());
        }
        match hunks.last_mut() {
            Some(h) => h.push_str(line),
            None => header.push_str(line),
        }
    }
    (header, hunks)
}

#[test]
fn non_contiguous_lines_across_hunks() {
    let old: String = (1..=30).map(|i| format!("l{i}\n")).collect();
    let new = old
        .replace("l2\n", "l2\nA\nB\n")
        .replace("l28\n", "l28\nC\n");
    let (d, r) = setup(&old, &new);
    r.stage("f.txt", &pick(&r, Side::Unstaged, &["+A", "+C"]), None)
        .unwrap();
    let expected = old.replace("l2\n", "l2\nA\n").replace("l28\n", "l28\nC\n");
    assert_eq!(index_content(d.path(), "f.txt").unwrap(), expected);
}

#[test]
fn missing_final_newline_is_preserved() {
    let (d, r) = setup("a\nb", "a\nb\nc");
    // Diff: "-b" (no NL), "+b\n", "+c" (no NL). Stage everything line by line.
    let all = Selection::Lines(
        changes(&r, Side::Unstaged)
            .into_iter()
            .map(|(h, i, _, _)| (h, i))
            .collect(),
    );
    r.stage("f.txt", &all, None).unwrap();
    assert_eq!(index_content(d.path(), "f.txt").unwrap(), "a\nb\nc");
}

#[test]
fn staging_an_added_line_after_a_line_without_newline_does_not_glue_them() {
    let (d, r) = setup("a\nb", "a\nb\nc\n");
    r.stage("f.txt", &pick(&r, Side::Unstaged, &["+c"]), None)
        .unwrap();
    assert_eq!(index_content(d.path(), "f.txt").unwrap(), "a\nb\nc\n");
}

#[test]
fn only_adding_the_final_newline_can_be_staged() {
    let (d, r) = setup("a\nb", "a\nb\n");
    let all = Selection::Lines(
        changes(&r, Side::Unstaged)
            .into_iter()
            .map(|(h, i, _, _)| (h, i))
            .collect(),
    );
    r.stage("f.txt", &all, None).unwrap();
    assert_eq!(index_content(d.path(), "f.txt").unwrap(), "a\nb\n");
}

#[test]
fn crlf_content_is_kept_byte_for_byte() {
    let (d, r) = setup("a\r\nb\r\n", "a\r\nX\r\nb\r\n");
    r.stage("f.txt", &pick(&r, Side::Unstaged, &["+X\r"]), None)
        .unwrap();
    assert_eq!(index_content(d.path(), "f.txt").unwrap(), "a\r\nX\r\nb\r\n");
}

#[test]
fn with_autocrlf_crlf_working_tree_only_shows_real_changes() {
    let (d, r) = setup("a\nb\n", "a\r\nX\r\nb\r\n");
    git2::Repository::open(d.path())
        .unwrap()
        .config()
        .unwrap()
        .set_str("core.autocrlf", "true")
        .unwrap();
    let r = Repo::open(d.path()).unwrap_or(r);
    let changes = changes(&r, Side::Unstaged);
    assert_eq!(changes.len(), 1, "{changes:?}");
    r.stage("f.txt", &pick(&r, Side::Unstaged, &["+X"]), None)
        .unwrap();
    assert_eq!(index_content(d.path(), "f.txt").unwrap(), "a\nX\nb\n");
}

#[test]
fn partially_staging_an_untracked_file_adds_only_the_chosen_lines() {
    let d = tempfile::tempdir().unwrap();
    common::make_repo(d.path(), 1);
    std::fs::write(d.path().join("new.txt"), "one\ntwo\nthree\n").unwrap();
    let r = Repo::open(d.path()).unwrap();
    let diff = r.diff_file("new.txt", Side::Unstaged).unwrap();
    assert_eq!(diff.hunks.len(), 1);
    r.stage(
        "new.txt",
        &Selection::Lines(vec![(0, 0), (0, 2)]),
        Some(&diff),
    )
    .unwrap();
    assert_eq!(index_content(d.path(), "new.txt").unwrap(), "one\nthree\n");
}

#[test]
fn stage_then_unstage_the_same_lines_restores_the_index() {
    let old: String = (1..=12).map(|i| format!("l{i}\n")).collect();
    let new = old
        .replace("l3\n", "l3 x\n")
        .replace("l9\n", "")
        .replace("l12\n", "l12\nend\n");
    let (d, r) = setup(&old, &new);
    let before = index_content(d.path(), "f.txt").unwrap();
    r.stage(
        "f.txt",
        &pick(&r, Side::Unstaged, &["+l3 x", "-l9", "+end"]),
        None,
    )
    .unwrap();
    assert_ne!(index_content(d.path(), "f.txt").unwrap(), before);
    r.unstage(
        "f.txt",
        &pick(&r, Side::Staged, &["+l3 x", "-l9", "+end"]),
        None,
    )
    .unwrap();
    assert_eq!(index_content(d.path(), "f.txt").unwrap(), before);
}

#[test]
fn unstage_one_line_keeps_the_rest_staged() {
    let (d, r) = setup("a\nb\n", "a\nX\nb\nY\n");
    r.stage("f.txt", &Selection::All, None).unwrap();
    r.unstage("f.txt", &pick(&r, Side::Staged, &["+X"]), None)
        .unwrap();
    assert_eq!(index_content(d.path(), "f.txt").unwrap(), "a\nb\nY\n");
}

#[test]
fn unstaging_all_lines_of_a_new_file_makes_it_untracked_again() {
    let d = tempfile::tempdir().unwrap();
    common::make_repo(d.path(), 1);
    std::fs::write(d.path().join("new.txt"), "one\n").unwrap();
    let r = Repo::open(d.path()).unwrap();
    r.stage("new.txt", &Selection::All, None).unwrap();
    r.unstage("new.txt", &Selection::Hunks(vec![0]), None)
        .unwrap();
    assert_eq!(index_content(d.path(), "new.txt"), None);
}

#[test]
fn whole_file_stage_and_unstage() {
    let (d, r) = setup("a\n", "b\n");
    r.stage("f.txt", &Selection::All, None).unwrap();
    assert_eq!(index_content(d.path(), "f.txt").unwrap(), "b\n");
    r.unstage("f.txt", &Selection::All, None).unwrap();
    assert_eq!(index_content(d.path(), "f.txt").unwrap(), "a\n");
}

#[test]
fn whole_file_stage_of_a_deletion_removes_it_from_the_index() {
    let (d, r) = setup("a\n", "a\n");
    std::fs::remove_file(d.path().join("f.txt")).unwrap();
    r.stage("f.txt", &Selection::All, None).unwrap();
    assert_eq!(index_content(d.path(), "f.txt"), None);
    r.unstage("f.txt", &Selection::All, None).unwrap();
    assert_eq!(index_content(d.path(), "f.txt").unwrap(), "a\n");
}

#[test]
fn partial_stage_refuses_deletions_and_binaries() {
    let (d, r) = setup("a\n", "a\n");
    std::fs::remove_file(d.path().join("f.txt")).unwrap();
    assert!(matches!(
        r.stage("f.txt", &Selection::Hunks(vec![0]), None),
        Err(GitError::Unsupported(_))
    ));
    std::fs::write(d.path().join("bin.dat"), [0u8, 1, 2, 0, 255]).unwrap();
    let diff = r.diff_file("bin.dat", Side::Unstaged).unwrap();
    assert!(diff.binary);
    assert!(matches!(
        r.stage("bin.dat", &Selection::Hunks(vec![0]), None),
        Err(GitError::Unsupported(_))
    ));
    r.stage("bin.dat", &Selection::All, None).unwrap();
}

#[test]
fn stale_selection_writes_nothing() {
    let (d, r) = setup("a\nb\n", "a\nX\nb\n");
    let shown = r.diff_file("f.txt", Side::Unstaged).unwrap();
    std::fs::write(d.path().join("f.txt"), "a\nX\nb\nmore\n").unwrap();
    let err = r
        .stage("f.txt", &Selection::Lines(vec![(0, 1)]), Some(&shown))
        .err()
        .unwrap();
    assert_eq!(err, GitError::StaleSelection);
    assert_eq!(index_content(d.path(), "f.txt").unwrap(), "a\nb\n");
}

#[test]
fn apply_selection_with_empty_selection_returns_base() {
    let (_d, r) = setup("a\nb\nc\n", "a\nc\nd\n");
    let diff = r.diff_file("f.txt", Side::Unstaged).unwrap();
    let out = apply_selection(
        b"a\nb\nc\n",
        &diff,
        &Selection::Lines(vec![]),
        Direction::Stage,
    )
    .unwrap();
    assert_eq!(out, b"a\nb\nc\n");
    let all = apply_selection(b"a\nb\nc\n", &diff, &Selection::All, Direction::Stage).unwrap();
    assert_eq!(all, b"a\nc\nd\n");
}

#[test]
fn partial_staging_keeps_non_utf8_bytes() {
    let d = tempfile::tempdir().unwrap();
    let repo = common::make_repo(d.path(), 1);
    std::fs::write(d.path().join("f.txt"), b"a\n").unwrap();
    commit_all(&repo, "base");
    std::fs::write(d.path().join("f.txt"), b"a\ncaf\xE9\n").unwrap(); // Latin-1 "café"
    let r = Repo::open(d.path()).unwrap();
    r.stage("f.txt", &Selection::Hunks(vec![0]), None).unwrap();
    let repo = git2::Repository::open(d.path()).unwrap(); // re-read the index from disk
    let idx = repo.index().unwrap();
    let e = idx.get_path(Path::new("f.txt"), 0).unwrap();
    assert_eq!(repo.find_blob(e.id).unwrap().content(), b"a\ncaf\xE9\n");
}
