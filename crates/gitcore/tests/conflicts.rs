#![allow(clippy::unwrap_used)]
//! Conflicted files after real merges and rebases (needs `git`).
mod common;

use std::path::Path;

use common::remote::{configure, git};
use gitcore::{
    Choice, ConflictKind, GitError, Operation, Pick, Refusal, Repo, TodoAction, TodoItem,
    apply_choice, conflict_count,
};

/// A repository with `main` and `feature` both changing `file` (or as `setup` says), then
/// `git merge feature` (or `rebase`) leaving conflicts.
fn conflicted(
    dir: &Path,
    base: Option<&[u8]>,
    ours: Option<&[u8]>,
    theirs: Option<&[u8]>,
    rebase: bool,
) -> Repo {
    git(dir, &["-c", "init.defaultBranch=main", "init", "-q"]);
    configure(dir);
    std::fs::write(dir.join("other.txt"), "x\n").unwrap();
    if let Some(b) = base {
        std::fs::write(dir.join("file"), b).unwrap();
    }
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", "base"]);
    git(dir, &["switch", "-q", "-c", "feature"]);
    match theirs {
        Some(t) => std::fs::write(dir.join("file"), t).unwrap(),
        None => std::fs::remove_file(dir.join("file")).unwrap(),
    }
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", "theirs"]);
    git(dir, &["switch", "-q", "main"]);
    match ours {
        Some(o) => std::fs::write(dir.join("file"), o).unwrap(),
        None => std::fs::remove_file(dir.join("file")).unwrap(),
    }
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", "ours"]);
    let args: &[&str] = if rebase {
        &["switch", "-q", "feature"]
    } else {
        &["status"]
    };
    git(dir, args);
    let out = std::process::Command::new("git")
        .current_dir(dir)
        .args(if rebase {
            vec!["rebase", "main"]
        } else {
            vec!["merge", "feature"]
        })
        .output()
        .unwrap();
    assert!(!out.status.success(), "expected a conflict");
    Repo::open(dir).unwrap()
}

fn tmp() -> Option<tempfile::TempDir> {
    gitcore::git_available().then(|| tempfile::tempdir().unwrap())
}

#[test]
fn a_merge_conflict_gives_both_versions_and_the_marked_file() {
    let Some(d) = tmp() else { return };
    let r = conflicted(
        d.path(),
        Some(b"a\nb\nc\n"),
        Some(b"a\nB-ours\nc\n"),
        Some(b"a\nB-theirs\nc\n"),
        false,
    );
    let c = r.conflict("file").unwrap();
    assert_eq!(c.kind, ConflictKind::Content);
    assert_eq!(c.mine.as_deref(), Some("a\nB-ours\nc\n"));
    assert_eq!(c.theirs.as_deref(), Some("a\nB-theirs\nc\n"));
    assert_eq!(c.operation, Some(Operation::Merge));
    let working = c.working.unwrap();
    assert_eq!(conflict_count(&working), 1, "{working}");
    assert_eq!(
        r.conflict("other.txt").err(),
        Some(GitError::Refused(Refusal::NotInConflict(
            "other.txt".into()
        ))),
        "not in conflict"
    );
    assert_eq!(
        r.continue_operation().err(),
        Some(GitError::Refused(Refusal::FinishMergeByCommit))
    );
    let list = [TodoItem {
        action: TodoAction::Pick,
        id: "x".into(),
        summary: "x".into(),
        message: "x".into(),
    }];
    assert_eq!(
        r.interactive_rebase("HEAD", &list).err(),
        Some(GitError::Refused(Refusal::OperationInProgress))
    );
}

#[test]
fn resolving_with_content_or_a_side_ends_the_conflict() {
    let Some(d) = tmp() else { return };
    let r = conflicted(
        d.path(),
        Some(b"a\n"),
        Some(b"ours\n"),
        Some(b"theirs\n"),
        false,
    );
    r.resolve_with_content("file", "merged\n").unwrap();
    assert!(r.conflict("file").is_err());
    assert_eq!(
        std::fs::read_to_string(d.path().join("file")).unwrap(),
        "merged\n"
    );
    let staged = git(d.path(), &["diff", "--cached", "--name-only"]);
    assert!(staged.contains("file"));

    let Some(d) = tmp() else { return };
    let r = conflicted(
        d.path(),
        Some(b"a\n"),
        Some(b"ours\n"),
        Some(b"theirs\n"),
        false,
    );
    r.resolve_with("file", Pick::Theirs).unwrap();
    assert!(r.conflict("file").is_err());
    assert_eq!(
        std::fs::read_to_string(d.path().join("file")).unwrap(),
        "theirs\n"
    );
}

#[test]
fn deleted_on_one_side() {
    let Some(d) = tmp() else { return };
    let r = conflicted(d.path(), Some(b"a\n"), Some(b"changed\n"), None, false);
    let c = r.conflict("file").unwrap();
    assert_eq!(c.kind, ConflictKind::DeletedByThem);
    r.resolve_delete("file").unwrap();
    assert!(r.conflict("file").is_err());
    assert!(!d.path().join("file").exists());

    let Some(d) = tmp() else { return };
    let r = conflicted(d.path(), Some(b"a\n"), None, Some(b"changed\n"), false);
    let c = r.conflict("file").unwrap();
    assert_eq!(c.kind, ConflictKind::DeletedByUs);
    assert_eq!(c.mine, None);
    r.resolve_with("file", Pick::Theirs).unwrap();
    assert!(r.conflict("file").is_err());
}

#[test]
fn added_on_both_sides_and_binary_files() {
    let Some(d) = tmp() else { return };
    let r = conflicted(d.path(), None, Some(b"mine\n"), Some(b"theirs\n"), false);
    assert_eq!(r.conflict("file").unwrap().kind, ConflictKind::AddedByBoth);

    let Some(d) = tmp() else { return };
    let r = conflicted(
        d.path(),
        Some(b"\x00a"),
        Some(b"\x00ours"),
        Some(b"\x00theirs"),
        false,
    );
    let c = r.conflict("file").unwrap();
    assert_eq!(c.kind, ConflictKind::Binary);
    r.resolve_with("file", Pick::Ours).unwrap();
    assert!(r.conflict("file").is_err());
}

#[test]
fn a_rebase_conflict_says_so() {
    let Some(d) = tmp() else { return };
    let r = conflicted(
        d.path(),
        Some(b"a\n"),
        Some(b"upstream\n"),
        Some(b"my commit\n"),
        true,
    );
    let c = r.conflict("file").unwrap();
    assert_eq!(c.operation, Some(Operation::Rebase));
    // During a rebase, "ours" is the upstream and "theirs" the commit being replayed.
    assert_eq!(c.mine.as_deref(), Some("upstream\n"));
    assert_eq!(c.theirs.as_deref(), Some("my commit\n"));
}

#[test]
fn paths_are_literal_not_patterns() {
    let Some(d) = tmp() else { return };
    let dir = d.path();
    git(dir, &["-c", "init.defaultBranch=main", "init", "-q"]);
    configure(dir);
    let write = |f: &str, t: &str| std::fs::write(dir.join(f), t).unwrap();
    write("a[1].txt", "base\n");
    write("a1.txt", "other\n");
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", "base"]);
    git(dir, &["switch", "-q", "-c", "feature"]);
    write("a[1].txt", "theirs\n");
    git(dir, &["commit", "-qam", "theirs"]);
    git(dir, &["switch", "-q", "main"]);
    write("a[1].txt", "ours\n");
    git(dir, &["commit", "-qam", "ours"]);
    let _ = std::process::Command::new("git")
        .current_dir(dir)
        .args(["merge", "feature"])
        .output();
    // A local change to a file the pattern `a[1].txt` would match.
    write("a1.txt", "my unstaged work\n");
    let r = Repo::open(dir).unwrap();
    r.resolve_with("a[1].txt", Pick::Theirs).unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.join("a1.txt")).unwrap(),
        "my unstaged work\n"
    );
    let staged = git(dir, &["diff", "--cached", "--name-only"]);
    assert!(!staged.contains("a1.txt"), "{staged}");
}

#[test]
fn an_unreadable_working_file_is_an_error_not_a_binary_file() {
    let Some(d) = tmp() else { return };
    let r = conflicted(
        d.path(),
        Some(b"a\nb\nc\n"),
        Some(b"a\nB-ours\nc\n"),
        Some(b"a\nB-theirs\nc\n"),
        false,
    );
    // A directory where the file should be: reading it fails on every platform.
    std::fs::remove_file(d.path().join("file")).unwrap();
    std::fs::create_dir(d.path().join("file")).unwrap();
    let err = r.conflict("file").unwrap_err();
    assert!(err.to_string().contains("file"), "{err}");
}

#[test]
fn both_sides_of_a_mixed_line_ending_file_keep_their_own_line_endings() {
    // CRLF elsewhere in the file, mine with lone '\n', theirs with CRLF.
    let text = "a\r\n<<<<<<< HEAD\nm1\nm2\n=======\r\nt1\r\n>>>>>>> x\r\nz\n";
    assert_eq!(
        apply_choice(text, 0, Choice::Both),
        "a\r\nm1\nm2\nt1\r\nz\n"
    );
}
