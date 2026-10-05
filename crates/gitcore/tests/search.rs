#![allow(clippy::unwrap_used)]
//! Searching file contents and commits (needs `git`).
mod common;

use std::path::Path;
use std::sync::atomic::AtomicBool;

use common::remote::{configure, git};
use gitcore::{LogSearch, RefKind, Repo};

static NO: AtomicBool = AtomicBool::new(false);

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

#[test]
fn grep_finds_text_in_a_version_with_case_and_paths() {
    let Some((d, r)) = repo() else { return };
    let old = commit_as(
        d.path(),
        "Ada",
        "src/a.rs",
        "let a = 1;\nlet Needle = 2;\n",
        "one",
    );
    commit_as(d.path(), "Ada", "notes/b c.txt", "needle here\r\n", "two");
    std::fs::write(d.path().join("bin.dat"), b"needle\0\x01").unwrap();
    git(d.path(), &["add", "-A"]);
    git(d.path(), &["commit", "-q", "-m", "bin"]);
    let all = r.grep("HEAD", "needle", false, "", 100, &NO).unwrap();
    let got: Vec<(&str, usize, &str)> = all
        .matches
        .iter()
        .map(|m| (m.path.as_str(), m.line, m.text.as_str()))
        .collect();
    assert_eq!(
        got,
        [
            ("notes/b c.txt", 1, "needle here"),
            ("src/a.rs", 2, "let Needle = 2;")
        ],
        "binary files skipped, no \\r"
    );
    assert!(!all.truncated);
    let cased = r.grep("HEAD", "needle", true, "", 100, &NO).unwrap();
    assert_eq!(cased.matches.len(), 1);
    let only_rs = r.grep("HEAD", "needle", false, "*.rs", 100, &NO).unwrap();
    assert_eq!(only_rs.matches.len(), 1);
    let at_old = r.grep(&old, "needle", false, "", 100, &NO).unwrap();
    assert_eq!(at_old.matches.len(), 1, "the first version has no notes/");
    let none = r.grep("HEAD", "absent text", false, "", 100, &NO).unwrap();
    assert!(none.matches.is_empty());
    let text = "x\n".repeat(30);
    commit_as(d.path(), "Ada", "many.txt", &text, "many");
    let capped = r.grep("HEAD", "x", true, "many.txt", 10, &NO).unwrap();
    assert_eq!(capped.matches.len(), 10);
    assert!(capped.truncated);
    assert!(
        r.grep("HEAD", "x", true, ":(bad", 10, &NO).is_err(),
        "invalid pathspec"
    );
}

#[test]
fn commits_are_found_by_message_author_hash_or_changed_text() {
    let Some((d, r)) = repo() else { return };
    let c1 = commit_as(d.path(), "Ada", "a.txt", "alpha\n", "Add the login page");
    git(d.path(), &["switch", "-q", "-c", "side"]);
    let c2 = commit_as(d.path(), "Bob", "b.txt", "secret token\n", "Side work");
    git(d.path(), &["switch", "-q", "main"]);
    commit_as(d.path(), "Ada", "a.txt", "beta\n", "Change alpha");
    let ids = |kind, q: &str| -> Vec<String> {
        r.search_log(kind, q, 100, &NO)
            .unwrap()
            .0
            .into_iter()
            .map(|e| e.id)
            .collect()
    };
    assert_eq!(
        ids(LogSearch::Message, "LOGIN"),
        [c1.as_str()],
        "case-insensitive"
    );
    assert_eq!(
        ids(LogSearch::Message, "("),
        Vec::<String>::new(),
        "literal text"
    );
    assert_eq!(ids(LogSearch::Author, "bob"), [c2.as_str()], "all branches");
    assert_eq!(ids(LogSearch::Hash, &c2[..7]), [c2.as_str()]);
    assert_eq!(ids(LogSearch::Hash, "zzzz"), Vec::<String>::new());
    assert_eq!(ids(LogSearch::ChangedText, "secret token"), [c2.as_str()]);
    assert_eq!(
        ids(LogSearch::ChangedText, "alpha").len(),
        2,
        "added then removed"
    );
    let (one, truncated) = r.search_log(LogSearch::Author, "ada", 1, &NO).unwrap();
    assert_eq!(one.len(), 1);
    assert!(truncated);
    assert!(
        one[0].refs.iter().any(|l| l.name == "main"),
        "labels like History"
    );
}

#[test]
fn the_refs_to_explore() {
    let Some((d, r)) = repo() else { return };
    commit_as(d.path(), "Ada", "a.txt", "a\n", "one");
    git(d.path(), &["branch", "side"]);
    git(d.path(), &["tag", "v1"]);
    let refs: Vec<(String, RefKind)> = r
        .explore_refs()
        .into_iter()
        .map(|x| (x.name, x.kind))
        .collect();
    assert_eq!(
        refs,
        [
            ("HEAD".to_string(), RefKind::Head),
            ("main".to_string(), RefKind::LocalBranch),
            ("side".to_string(), RefKind::LocalBranch),
            ("v1".to_string(), RefKind::Tag),
        ]
    );
}

#[test]
fn tags_to_something_else_than_a_commit_are_not_explored() {
    let Some((d, r)) = repo() else { return };
    commit_as(d.path(), "Ada", "a.txt", "a\n", "one");
    git(
        d.path(),
        &["tag", "-a", "-m", "t", "treetag", "HEAD^{tree}"],
    );
    git(d.path(), &["tag", "-a", "-m", "v", "v1"]);
    git(d.path(), &["tag", "v2"]);
    let names: Vec<String> = r.explore_refs().into_iter().map(|x| x.name).collect();
    assert_eq!(names, ["HEAD", "main", "v1", "v2"]);
}

#[test]
fn grep_output_is_parsed() {
    let id = "a".repeat(40);
    let out = format!("{id}:src/x.rs\x002\x00  let x = 1;\r\n{id}:a:b.txt\x0010\x00t\n");
    let got = gitcore::parse_grep(&out, &id);
    assert_eq!(got[0].path, "src/x.rs");
    assert_eq!((got[0].line, got[0].text.as_str()), (2, "  let x = 1;"));
    assert_eq!(got[1].path, "a:b.txt");
    assert_eq!(got[1].line, 10);
}

#[test]
fn case_insensitive_searches_understand_accents() {
    let Some((d, r)) = repo() else { return };
    let c = commit_as(d.path(), "Ada", "a.txt", "le café\n", "Ajoute le café");
    assert_eq!(
        r.grep("HEAD", "CAFÉ", false, "", 10, &NO)
            .unwrap()
            .matches
            .len(),
        1
    );
    let (found, _) = r.search_log(LogSearch::Message, "CAFÉ", 10, &NO).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].id, c);
}

#[test]
fn searching_commits_of_an_empty_repository_finds_nothing() {
    let Some((_d, r)) = repo() else { return };
    for kind in [
        LogSearch::Message,
        LogSearch::Author,
        LogSearch::Hash,
        LogSearch::ChangedText,
    ] {
        assert_eq!(
            r.search_log(kind, "fix", 100, &NO).unwrap(),
            (vec![], false)
        );
    }
}

#[test]
fn a_capped_search_knows_it_was_capped_even_by_one_line() {
    let Some((d, r)) = repo() else { return };
    commit_as(d.path(), "Ada", "eleven.txt", &"x\n".repeat(11), "eleven");
    commit_as(d.path(), "Ada", "ten.txt", &"x\n".repeat(10), "ten");
    // The cap is read once git's output is fully read: repeat to catch a reader that is late.
    for i in 0..50 {
        let over = r.grep("HEAD", "x", true, "eleven.txt", 10, &NO).unwrap();
        assert_eq!(over.matches.len(), 10, "run {i}");
        assert!(over.truncated, "11 lines for a limit of 10 (run {i})");
        let exact = r.grep("HEAD", "x", true, "ten.txt", 10, &NO).unwrap();
        assert_eq!(exact.matches.len(), 10, "run {i}");
        assert!(
            !exact.truncated,
            "exactly the limit is not capped (run {i})"
        );
    }
}
