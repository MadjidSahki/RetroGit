#![allow(clippy::unwrap_used)]
//! Explore: files of a version, blame and history of a file (needs `git`).
mod common;

use std::path::Path;
use std::sync::atomic::AtomicBool;

use common::remote::{configure, git};
use gitcore::{EntryKind, FileContent, Repo};

fn commit_as(dir: &Path, who: &str, file: &str, text: &str, msg: &str) -> String {
    if let Some(parent) = Path::new(file).parent() {
        std::fs::create_dir_all(dir.join(parent)).unwrap();
    }
    std::fs::write(dir.join(file), text).unwrap();
    git(dir, &["add", "-A"]);
    let author = format!("{who} <{}@example.com>", who.to_lowercase());
    git(dir, &["commit", "-q", "--author", &author, "-m", msg]);
    git(dir, &["rev-parse", "HEAD"]).trim().to_string()
}

fn repo() -> Option<(tempfile::TempDir, Repo)> {
    if !gitcore::git_available() {
        return None;
    }
    let d = tempfile::tempdir().unwrap();
    git(d.path(), &["-c", "init.defaultBranch=main", "init", "-q"]);
    configure(d.path());
    let r = Repo::open(d.path()).unwrap();
    Some((d, r))
}

static NO: AtomicBool = AtomicBool::new(false);

#[test]
fn the_tree_and_files_of_any_version() {
    let Some((d, r)) = repo() else { return };
    assert!(r.tree("HEAD").unwrap().is_empty(), "no commit yet");
    let first = commit_as(d.path(), "Ada", "src/main.rs", "fn main() {}\n", "one");
    commit_as(d.path(), "Ada", "README.md", "# hi\r\nthere\r\n", "two");
    std::fs::write(d.path().join("bin.dat"), [0u8, 1, 2, 0, 255]).unwrap();
    git(d.path(), &["add", "-A"]);
    git(d.path(), &["commit", "-q", "-m", "binary"]);
    let tree = r.tree("HEAD").unwrap();
    let paths: Vec<(&str, EntryKind)> = tree.iter().map(|e| (e.path.as_str(), e.kind)).collect();
    assert_eq!(
        paths,
        [
            ("README.md", EntryKind::File),
            ("bin.dat", EntryKind::File),
            ("src", EntryKind::Dir),
            ("src/main.rs", EntryKind::File),
        ]
    );
    assert_eq!(
        r.tree(&first).unwrap().len(),
        2,
        "src and src/main.rs at the first commit"
    );
    assert_eq!(
        r.file_at("HEAD", "README.md").unwrap(),
        FileContent::Text("# hi\r\nthere\r\n".into())
    );
    assert_eq!(r.file_at("HEAD", "bin.dat").unwrap(), FileContent::Binary);
    assert!(r.file_at(&first, "README.md").is_err(), "not there yet");
    assert_eq!(r.resolve("HEAD~2").as_deref(), Some(first.as_str()));
    assert_eq!(r.resolve("no-such-branch"), None);
}

#[test]
fn big_files_are_not_loaded() {
    let Some((d, r)) = repo() else { return };
    let big = "x".repeat(gitcore::MAX_FILE_BYTES as usize + 1);
    commit_as(d.path(), "Ada", "big.txt", &big, "big");
    assert_eq!(
        r.file_at("HEAD", "big.txt").unwrap(),
        FileContent::TooLarge(gitcore::MAX_FILE_BYTES + 1)
    );
}

#[test]
fn blame_groups_lines_by_commit_and_goes_back_to_the_parent() {
    let Some((d, r)) = repo() else { return };
    let c1 = commit_as(d.path(), "Ada", "a.txt", "one\ntwo\nthree\n", "first");
    let c2 = commit_as(
        d.path(),
        "Bob",
        "a.txt",
        "one\nTWO\nthree\nfour\n",
        "second",
    );
    let blocks = r.blame("HEAD", "a.txt", &NO).unwrap();
    let got: Vec<(&str, &str, usize, usize)> = blocks
        .iter()
        .map(|b| (b.commit.as_str(), b.author.as_str(), b.start, b.count))
        .collect();
    assert_eq!(
        got,
        [
            (c1.as_str(), "Ada", 1, 1),
            (c2.as_str(), "Bob", 2, 1),
            (c1.as_str(), "Ada", 3, 1),
            (c2.as_str(), "Bob", 4, 1),
        ]
    );
    assert_eq!(blocks[1].summary, "second");
    assert_eq!(blocks[1].lines, ["TWO"]);
    // Blame the parent of the commit of line 2: line 2 is Ada's "two".
    let parent = r
        .blame(&format!("{c2}^"), &blocks[1].orig_path, &NO)
        .unwrap();
    assert_eq!(parent.len(), 1);
    assert_eq!(parent[0].commit, c1);
    assert_eq!(parent[0].count, 3);
}

#[test]
fn a_file_history_follows_renames() {
    let Some((d, r)) = repo() else { return };
    commit_as(d.path(), "Ada", "old name.txt", "a\nb\nc\nd\n", "create");
    commit_as(d.path(), "Ada", "other.txt", "x\n", "unrelated");
    git(d.path(), &["mv", "old name.txt", "new.txt"]);
    git(d.path(), &["commit", "-q", "-m", "rename"]);
    commit_as(d.path(), "Bob", "new.txt", "a\nb\nc\nd\ne\n", "edit");
    let h = r.file_history("HEAD", "new.txt", 100, &NO).unwrap();
    let got: Vec<(&str, &str, Option<&str>)> = h
        .iter()
        .map(|c| {
            (
                c.entry.summary.as_str(),
                c.path.as_str(),
                c.renamed_from.as_deref(),
            )
        })
        .collect();
    assert_eq!(
        got,
        [
            ("edit", "new.txt", None),
            ("rename", "new.txt", Some("old name.txt")),
            ("create", "old name.txt", None),
        ]
    );
    assert_eq!(h[0].entry.author, "Bob");
    assert_eq!(
        r.file_history("HEAD", "new.txt", 1, &NO).unwrap().len(),
        1,
        "limit"
    );
}

#[test]
fn a_cancelled_blame_stops() {
    let Some((d, r)) = repo() else { return };
    commit_as(d.path(), "Ada", "a.txt", "one\n", "first");
    let cancelled = AtomicBool::new(true);
    assert!(matches!(
        r.blame("HEAD", "a.txt", &cancelled),
        Err(gitcore::GitError::Cancelled)
    ));
}

#[test]
fn porcelain_blame_is_parsed() {
    let text = "\
aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa 1 1 2
author Ada
author-mail <ada@example.com>
author-time 1700000000
author-tz +0000
committer Ada
committer-mail <ada@example.com>
committer-time 1700000000
committer-tz +0000
summary First commit
boundary
filename src/old.rs
\tline one
aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa 2 2
\tline two
bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb 5 3 1
author Bob
author-mail <bob@example.com>
author-time 1700000100
author-tz +0000
committer Bob
committer-mail <bob@example.com>
committer-time 1700000100
committer-tz +0000
summary Second
previous aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa src/old.rs
filename src/new.rs
\tline three
";
    let b = gitcore::parse_blame_porcelain(text);
    assert_eq!(b.len(), 2);
    assert_eq!((b[0].start, b[0].count, b[0].orig_start), (1, 2, 1));
    assert_eq!(b[0].author, "Ada");
    assert_eq!(b[0].time, 1_700_000_000);
    assert!(b[0].boundary);
    assert_eq!(b[0].orig_path, "src/old.rs");
    assert_eq!((b[1].start, b[1].count, b[1].orig_start), (3, 1, 5));
    assert_eq!(b[1].summary, "Second");
    assert_eq!(b[1].orig_path, "src/new.rs");
    assert!(!b[1].boundary);
    assert_eq!(b[0].lines, ["line one", "line two"]);
    assert_eq!(b[1].lines, ["line three"]);
}

#[test]
fn option_like_ref_names_are_never_passed_as_options() {
    let Some((d, r)) = repo() else { return };
    commit_as(d.path(), "Ada", "f.txt", "one\n", "first");
    git(
        d.path(),
        &["update-ref", "refs/tags/--output=pwned.txt", "HEAD"],
    );
    let rev = "--output=pwned.txt";
    assert_eq!(r.file_history(rev, "f.txt", 10, &NO).unwrap().len(), 1);
    assert_eq!(r.blame(rev, "f.txt", &NO).unwrap().len(), 1);
    assert!(
        !d.path().join("pwned.txt").exists(),
        "git wrote where the tag said"
    );
}

#[test]
fn blame_the_parent_follows_a_rename_in_the_same_commit() {
    let Some((d, r)) = repo() else { return };
    let c1 = commit_as(d.path(), "Ada", "café.txt", "one\n", "create");
    git(d.path(), &["mv", "café.txt", "renamé.txt"]);
    std::fs::write(d.path().join("renamé.txt"), "one\ntwo\n").unwrap();
    git(d.path(), &["add", "-A"]);
    git(d.path(), &["commit", "-q", "-m", "rename and add"]);
    let blocks = r.blame("HEAD", "renamé.txt", &NO).unwrap();
    let added = blocks.iter().find(|b| b.lines == ["two"]).unwrap();
    let (prev, path) = added.previous.clone().unwrap();
    assert_eq!((prev.as_str(), path.as_str()), (c1.as_str(), "café.txt"));
    assert_eq!(r.blame(&prev, &path, &NO).unwrap().len(), 1);
    let created = r.blame(&c1, "café.txt", &NO).unwrap();
    assert_eq!(
        created[0].previous, None,
        "a line added by the first commit has no parent"
    );
}
