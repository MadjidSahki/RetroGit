#![allow(clippy::unwrap_used)]
//! Applying a pull request suggestion as a local commit (needs `git`).
mod common;

use std::path::Path;

use common::remote::{configure, git};
use gitcore::{GitError, Refusal, Repo};

fn repo(dir: &Path, content: &[u8]) -> Repo {
    git(dir, &["-c", "init.defaultBranch=main", "init", "-q"]);
    configure(dir);
    std::fs::write(dir.join("a.rs"), content).unwrap();
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", "base"]);
    Repo::open(dir).unwrap()
}

fn lines(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

#[test]
fn a_suggestion_replaces_its_lines_in_one_commit() {
    if !gitcore::git_available() {
        return;
    }
    let d = tempfile::tempdir().unwrap();
    let r = repo(d.path(), b"a\nb\nc\nd\n");
    r.apply_suggestion(
        "a.rs",
        2,
        3,
        &lines(&["b", "c"]),
        "B1\nB2\nB3\n",
        "Apply suggestion from @bob",
    )
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(d.path().join("a.rs")).unwrap(),
        "a\nB1\nB2\nB3\nd\n"
    );
    let log = git(d.path(), &["log", "-1", "--format=%s"]);
    assert_eq!(log.trim(), "Apply suggestion from @bob");
    assert!(
        git(d.path(), &["status", "--porcelain"]).is_empty(),
        "committed"
    );
}

#[test]
fn deleting_lines_and_keeping_crlf_and_the_missing_final_newline() {
    if !gitcore::git_available() {
        return;
    }
    let d = tempfile::tempdir().unwrap();
    let r = repo(d.path(), b"a\r\nb\r\nc");
    r.apply_suggestion("a.rs", 3, 3, &lines(&["c"]), "C\n", "s")
        .unwrap();
    assert_eq!(
        std::fs::read(d.path().join("a.rs")).unwrap(),
        b"a\r\nb\r\nC"
    );
    r.apply_suggestion("a.rs", 2, 2, &lines(&["b"]), "", "s")
        .unwrap();
    assert_eq!(std::fs::read(d.path().join("a.rs")).unwrap(), b"a\r\nC");
}

#[test]
fn changed_lines_or_local_changes_are_refused() {
    if !gitcore::git_available() {
        return;
    }
    let d = tempfile::tempdir().unwrap();
    let r = repo(d.path(), b"a\nb\n");
    assert_eq!(
        r.apply_suggestion("a.rs", 2, 2, &lines(&["not b"]), "x\n", "s"),
        Err(GitError::SuggestionOutdated)
    );
    assert_eq!(
        r.apply_suggestion("a.rs", 5, 6, &lines(&["", ""]), "x\n", "s"),
        Err(GitError::SuggestionOutdated),
        "past the end"
    );
    std::fs::write(d.path().join("a.rs"), "a\nb\nmine\n").unwrap();
    assert!(matches!(
        r.apply_suggestion("a.rs", 2, 2, &lines(&["b"]), "x\n", "s"),
        Err(GitError::Refused(Refusal::LocalChangesFirst))
    ));
    assert_eq!(
        std::fs::read_to_string(d.path().join("a.rs")).unwrap(),
        "a\nb\nmine\n"
    );
}

#[cfg(unix)]
#[test]
fn a_refused_commit_puts_the_file_back() {
    use std::os::unix::fs::PermissionsExt;
    if !gitcore::git_available() {
        return;
    }
    let d = tempfile::tempdir().unwrap();
    let r = repo(d.path(), b"a\nb\n");
    let hook = d.path().join(".git/hooks/pre-commit");
    std::fs::write(&hook, "#!/bin/sh\necho 'lint failed' >&2\nexit 1\n").unwrap();
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    let e = r
        .apply_suggestion("a.rs", 2, 2, &lines(&["b"]), "B\n", "s")
        .unwrap_err();
    assert!(matches!(e, GitError::CommitRejected { .. }), "{e:?}");
    assert_eq!(
        std::fs::read_to_string(d.path().join("a.rs")).unwrap(),
        "a\nb\n"
    );
    assert!(
        git(d.path(), &["status", "--porcelain"]).is_empty(),
        "nothing left staged"
    );
}

#[test]
fn each_line_keeps_its_own_ending() {
    let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    assert_eq!(
        gitcore::replace_lines("a\nb\r\nc\n", 3, 3, &s(&["c"]), "C\n").as_deref(),
        Some("a\nb\r\nC\n"),
        "only the replaced line changes"
    );
    assert_eq!(
        gitcore::replace_lines("a\r\nb\nc\n", 1, 1, &s(&["a"]), "A1\nA2\n").as_deref(),
        Some("A1\r\nA2\r\nb\nc\n"),
        "new lines take the ending of the line they replace"
    );
}

#[test]
fn a_suggestion_already_applied_says_so_and_commits_nothing() {
    if !gitcore::git_available() {
        return;
    }
    let d = tempfile::tempdir().unwrap();
    let r = repo(d.path(), b"a\nb\n");
    let before = git(d.path(), &["rev-parse", "HEAD"]);
    assert_eq!(
        r.apply_suggestion("a.rs", 2, 2, &lines(&["b"]), "b\n", "s"),
        Err(GitError::SuggestionApplied)
    );
    assert_eq!(
        std::fs::read_to_string(d.path().join("a.rs")).unwrap(),
        "a\nb\n"
    );
    assert_eq!(git(d.path(), &["rev-parse", "HEAD"]), before, "no commit");
    assert!(git(d.path(), &["status", "--porcelain"]).is_empty());
}

#[test]
fn applying_a_suggestion_twice_says_it_is_already_applied() {
    if !gitcore::git_available() {
        return;
    }
    let d = tempfile::tempdir().unwrap();
    let r = repo(d.path(), b"a\nb\nc\n");
    let expected = lines(&["b"]);
    r.apply_suggestion("a.rs", 2, 2, &expected, "B1\nB2\n", "s")
        .unwrap();
    let before = git(d.path(), &["rev-parse", "HEAD"]);
    // The lines now read as the suggestion (even when it changed their count).
    assert_eq!(
        r.apply_suggestion("a.rs", 2, 2, &expected, "B1\nB2\n", "s"),
        Err(GitError::SuggestionApplied)
    );
    assert_eq!(git(d.path(), &["rev-parse", "HEAD"]), before, "no commit");
    // Lines changed by someone else: still outdated.
    std::fs::write(d.path().join("a.rs"), "a\nX\nc\n").unwrap();
    git(d.path(), &["commit", "-qam", "other"]);
    assert_eq!(
        r.apply_suggestion("a.rs", 2, 2, &expected, "B1\nB2\n", "s"),
        Err(GitError::SuggestionOutdated)
    );
    // A suggestion deleting the lines never reads as applied by itself.
    assert_eq!(
        r.apply_suggestion("a.rs", 2, 2, &expected, "", "s"),
        Err(GitError::SuggestionOutdated)
    );
}
