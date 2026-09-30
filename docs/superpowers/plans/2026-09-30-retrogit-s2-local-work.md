# RetroGit S2 — Local Work (status, diff, staging, commit) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** In an open repository, show status and diffs, stage/unstage whole files, hunks or single lines, commit through the installed `git` (hooks, signing) with a libgit2 fallback, amend, add to `.gitignore`, and refresh automatically when files change on disk.

**Architecture:** `gitcore` gains `status`, `diff`, `stage` (a pure `apply_selection` that computes the new index content, like `git add -p`), `commit` (CLI first) and `ignore`. `win95` gains `checkbox` and `text_area`. The app gets a `notify`-based watcher, new worker commands/events (still one worker thread, so index writes never race), a `ChangesView` in the pure reducer, and a "Changes" screen replacing the S1 repository summary.

**Tech Stack:** Existing S1 stack (Rust ≥ 1.95, egui/eframe 0.36, git2 0.21, …) plus `notify = "8"` (verified with 8.2.0). `git` CLI optional at runtime (verified with git 2.50).

**Spec:** `docs/superpowers/specs/2026-09-30-retrogit-s2-local-work-design.md`

> Every code block was compiled, formatted, linted (`clippy -D warnings`) and tested on macOS arm64 with Rust 1.98.1 before this plan was written (148 tests), then the plan itself was replayed task by task on a clean checkout of `main`. Copy code verbatim; if an `Expected:` differs, stop and investigate.

## Global Constraints

- Work on branch `feat/s2-local-work` created from `main` in `~/perso/retrogit`.
- Targets: `aarch64-apple-darwin` and `x86_64-pc-windows-msvc`.
- `win95`, `gitcore`, `github` must not depend on each other; no `git2` type in `gitcore`'s public API.
- All user-visible strings in `crates/app/src/strings.rs`, in English.
- One worker thread performs every Git operation (no concurrent index writes).
- Commits go through `git commit` when `git` is runnable (hooks, signing, global config); libgit2 fallback otherwise, with a one-time warning.
- Out of scope: discard, conflict resolution, stash, history/branches/sync (S3), PRs (S4), partial staging of binaries, deletions or renames.
- Diffs over 20 000 lines are only rendered after "Show anyway".
- New dependency: `notify = "8"` (workspace), used by `crates/app` only. Release binary must stay under 15 MB (13 MB when this plan was written).
- Tests must not depend on the developer's global Git config: test repos set `core.autocrlf=false`, and commit tests set `user.name`, `user.email`, `commit.gpgsign=false`, `core.hooksPath=.git/hooks` locally. Tests needing the `git` CLI return early when it is absent.
- Every task ends with `cargo fmt --all --check` and `cargo clippy --workspace --all-targets -- -D warnings` clean and `cargo test --workspace` green.
- Ruled deviations from the spec (decided while verifying the code):
  - `Repo::stage` / `Repo::unstage` take a third argument `shown: Option<&FileDiff>` (the diff the selection was made on) — that is how a stale selection is detected.
  - `Repo::commit` takes a `CommitBackend` (`PreferCli` in the app, `Git2` in worker tests for determinism).
  - The diff view uses egui's built-in monospace font: W95FA is proportional and misaligns columns (the spec allowed this fallback). `FontFamily::Monospace` no longer starts with W95FA.
  - Worker events add `Event::AmendInfo { message, pushed }` instead of separate `LastCommitMessage` / `HeadPushed` events.
  - On macOS the app asks the login shell for the user's PATH in a background thread and hands it to `gitcore::set_git_search_path`, so hooks that need Homebrew/nvm tools work when RetroGit is started from the Dock.

## Review Focus

1. **Hooks that need tools outside the GUI PATH** (husky/node, pre-commit/python from Homebrew or nvm) when RetroGit is launched from the Dock: the commit must behave as in a terminal. Pinned by `login_shell_path_includes_system_dirs` (Task 4) and wired in `main.rs` (Task 5); final check is manual (Task 7).
2. **A file edited on disk between showing its diff and clicking "Stage selected lines"**: nothing is written, the diff reloads and an info box explains it. Pinned by `stale_selection_writes_nothing` (Task 1) and `stale_selection_reports_and_resyncs` (Task 5).
3. **Windows/CRLF repositories with `core.autocrlf=true`**: only real changes appear, and staging lines writes LF content like Git would. Pinned by `with_autocrlf_crlf_working_tree_only_shows_real_changes` and `crlf_content_is_kept_byte_for_byte` (Task 1).
4. **Files without a final newline**: staging a line never glues two lines together, and the missing newline survives. Pinned by `missing_final_newline_is_preserved`, `staging_an_added_line_after_a_line_without_newline_does_not_glue_them`, `only_adding_the_final_newline_can_be_staged` (Task 1).
5. **Bursts of disk events** (build output, `npm install`) must not flood the worker: one refresh per 300 ms burst, at most one queued. Pinned by `a_burst_of_writes_is_reported_once_and_git_internals_are_ignored` (Task 4) and `refresh_requests_are_deduplicated_while_one_is_pending` (Task 5).

---


### Task 1: `gitcore` status, diff and partial staging

**Files:**
- Create: `crates/gitcore/src/status.rs`, `crates/gitcore/src/diff.rs`, `crates/gitcore/src/stage.rs`
- Modify: `crates/gitcore/src/error.rs`, `crates/gitcore/src/repo.rs`, `crates/gitcore/src/lib.rs`, `crates/gitcore/tests/common/mod.rs`
- Modify: `crates/app/src/strings.rs`, `crates/app/src/protocol.rs` (map the new `GitError` variants)
- Test: `crates/gitcore/tests/status_diff.rs`, `crates/gitcore/tests/stage.rs`

**Interfaces:**
- Consumes: S1 `Repo`, `GitError`, `GitError::from_git2`.
- Produces:
  - `GitError` new variants: `StaleSelection`, `Unsupported(String)`, `CommitRejected { output: String }`, `MissingIdentity`.
  - `Repo::status(&self) -> Result<Vec<FileStatus>, GitError>`; `FileStatus { path: String, staged: Option<Change>, unstaged: Option<Change> }`; `Change::{Added, Modified, Deleted, Renamed { from: String }, TypeChange, Untracked, Conflicted}`.
  - `Repo::diff_file(&self, path: &str, side: Side) -> Result<FileDiff, GitError>`; `Side::{Unstaged, Staged}` (`Copy + Eq + Hash`); `FileDiff { path, side, binary, hunks: Vec<Hunk> }` with `line_count()`; `Hunk { header, old_start, old_lines, new_start, new_lines: u32, lines: Vec<DiffLine> }`; `DiffLine { kind: LineKind, old_no, new_no: Option<u32>, text: String /* with its line ending */, no_newline_at_eof: bool }`; `LineKind::{Context, Added, Removed}`.
  - `Selection::{All, Hunks(Vec<usize>), Lines(Vec<(usize, usize)>)}`; `Direction::{Stage, Unstage}`; `apply_selection(base: &[u8], diff: &FileDiff, selection: &Selection, direction: Direction) -> Result<Vec<u8>, GitError>`; `selectable_lines(&FileDiff) -> BTreeSet<(usize, usize)>`.
  - `Repo::stage(&self, path: &str, selection: &Selection, shown: Option<&FileDiff>) -> Result<(), GitError>`; `Repo::unstage(...)` same signature.
  - crate-private: `Repo::git(&self) -> &git2::Repository`, `Repo::workdir(&self) -> Result<&Path, GitError>`.
  - `strings::*` gains every S2 string (used from Task 5 on); `AppError::from_git` maps the four new variants.

- [ ] **Step 1: Create the branch**

Run: `git checkout -b feat/s2-local-work`

Expected: `Switched to a new branch 'feat/s2-local-work'`.

- [ ] **Step 2: Isolate test repos from the global Git config**

Replace the whole file with the version below (it keeps everything from sub-project 1). The only change: `make_repo` sets `core.autocrlf=false` so a developer's `autocrlf=input` cannot change test results.

`crates/gitcore/tests/common/mod.rs`:

```rust
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;

/// Create a non-bare repo with `commits` commits on branch `main`.
pub fn make_repo(dir: &Path, commits: usize) -> git2::Repository {
    let mut opts = git2::RepositoryInitOptions::new();
    opts.initial_head("main");
    let repo = git2::Repository::init_opts(dir, &opts).unwrap();
    // Tests must not depend on the developer's ~/.gitconfig (e.g. core.autocrlf=input).
    repo.config()
        .unwrap()
        .set_str("core.autocrlf", "false")
        .unwrap();
    {
        let sig =
            git2::Signature::new("Ada", "ada@example.com", &git2::Time::new(1_700_000_000, 0))
                .unwrap();
        for i in 0..commits {
            let file = format!("file{i}.txt");
            std::fs::write(dir.join(&file), format!("content {i}\n").repeat(50)).unwrap();
            let mut idx = repo.index().unwrap();
            idx.add_path(Path::new(&file)).unwrap();
            idx.write().unwrap();
            let tree = repo.find_tree(idx.write_tree().unwrap()).unwrap();
            let parents: Vec<git2::Commit<'_>> = repo
                .head()
                .ok()
                .and_then(|h| h.peel_to_commit().ok())
                .into_iter()
                .collect();
            let parent_refs: Vec<&git2::Commit<'_>> = parents.iter().collect();
            repo.commit(
                Some("HEAD"),
                &sig,
                &sig,
                &format!("commit {i}"),
                &tree,
                &parent_refs,
            )
            .unwrap();
        }
    }
    repo
}

/// `file://` URL for a local path, valid on Unix and Windows. Forces the "smart" transport
/// so that transfer progress callbacks fire (a plain path uses a local copy instead).
pub fn file_url(p: &Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    if s.starts_with('/') {
        format!("file://{s}")
    } else {
        format!("file:///{s}")
    }
}
```

- [ ] **Step 3: Write the failing status/diff tests**

`crates/gitcore/tests/status_diff.rs`:

```rust
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
```

- [ ] **Step 4: Write the failing staging tests**

`stage_one_hunk_of_two_matches_git_apply_cached` compares our result with `git apply --cached` of the same hunk (skipped when `git` is absent).

`crates/gitcore/tests/stage.rs`:

```rust
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
```

- [ ] **Step 5: Declare the modules**

Replace the whole file with the version below (it keeps everything from sub-project 1).

`crates/gitcore/src/lib.rs`:

```rust
//! RetroGit's internal Git API. No `git2` type is ever exposed publicly.

mod clone;
mod diff;
mod error;
mod repo;
mod stage;
mod status;

pub use clone::{CloneProgress, CloneRequest, Credentials, clone};
pub use diff::{DiffLine, FileDiff, Hunk, LineKind, Side};
pub use error::GitError;
pub use repo::{CommitInfo, Head, Repo, RepoSummary};
pub use stage::{Direction, Selection, apply_selection, selectable_lines};
pub use status::{Change, FileStatus};

/// Bound libgit2's network waits (default: infinite) so a stalled clone eventually fails
/// and can be cleaned up. Call once at startup, before any network operation.
#[allow(unsafe_code)]
pub fn configure_network_timeouts() -> Result<(), GitError> {
    // SAFETY: git2 marks these unsafe only because they mutate libgit2 global state;
    // calling them before any other thread uses libgit2 is sound.
    unsafe {
        git2::opts::set_server_connect_timeout_in_milliseconds(15_000)
            .and_then(|()| git2::opts::set_server_timeout_in_milliseconds(60_000))
            .map_err(|e| GitError::Other(e.message().to_string()))
    }
}
```

- [ ] **Step 6: Run the tests to see them fail**

Run: `cargo test -p gitcore`

Expected: compile errors: `file not found for module diff`, `stage`, `status`.

- [ ] **Step 7: Add the new error variants**

Replace the whole file with the version below (it keeps everything from sub-project 1).

`crates/gitcore/src/error.rs`:

```rust
use std::path::PathBuf;

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum GitError {
    #[error("'{0}' is not a Git repository")]
    NotARepository(PathBuf),
    #[error("destination '{0}' already exists and is not empty")]
    DestinationNotEmpty(PathBuf),
    #[error("operation cancelled")]
    Cancelled,
    #[error("authentication failed: {0}")]
    Auth(String),
    #[error("network error: {0}")]
    Network(String),
    /// The file changed since its diff was displayed; nothing was written.
    #[error("the file changed since its diff was shown")]
    StaleSelection,
    #[error("{0}")]
    Unsupported(String),
    /// `git commit` exited with an error (hook, signing...). `output` is its stdout+stderr.
    #[error("commit rejected")]
    CommitRejected { output: String },
    #[error("user.name / user.email are not configured")]
    MissingIdentity,
    #[error("{0}")]
    Other(String),
}

impl GitError {
    /// Map a libgit2 error to our error type (cancellation is decided by the caller).
    pub(crate) fn from_git2(e: &git2::Error) -> Self {
        use git2::{ErrorClass as C, ErrorCode as K};
        let msg = e.message().to_string();
        match (e.code(), e.class()) {
            (K::Auth, _) | (_, C::Ssh) => GitError::Auth(msg),
            (_, C::Http | C::Net | C::Ssl | C::Os) => {
                if msg.contains("401") || msg.contains("403") || msg.contains("authentication") {
                    GitError::Auth(msg)
                } else {
                    GitError::Network(msg)
                }
            }
            _ => GitError::Other(msg),
        }
    }
}
```

- [ ] **Step 8: Give the new modules access to the libgit2 handle**

Replace the whole file with the version below (it keeps everything from sub-project 1). Only addition: the crate-private `git()` accessor.

`crates/gitcore/src/repo.rs`:

```rust
use std::path::{Path, PathBuf};

use crate::GitError;

/// An open local repository.
pub struct Repo {
    inner: git2::Repository,
    path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Head {
    /// HEAD points to a branch that has at least one commit.
    Branch(String),
    /// HEAD points to a branch with no commit yet (fresh `git init`).
    Unborn(String),
    /// HEAD is detached at this short commit id.
    Detached(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitInfo {
    pub short_id: String,
    pub summary: String,
    pub author: String,
    /// Seconds since the Unix epoch.
    pub time: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoSummary {
    /// Folder name of the working directory.
    pub name: String,
    pub path: PathBuf,
    pub head: Head,
    pub origin_url: Option<String>,
    pub last_commit: Option<CommitInfo>,
}

impl Repo {
    /// Open the repository whose working directory (or bare dir) is exactly `path`.
    pub fn open(path: &Path) -> Result<Repo, GitError> {
        let flags = git2::RepositoryOpenFlags::NO_SEARCH;
        match git2::Repository::open_ext(path, flags, std::iter::empty::<&std::ffi::OsStr>()) {
            Ok(inner) => Ok(Repo {
                inner,
                path: path.to_path_buf(),
            }),
            Err(e) if e.code() == git2::ErrorCode::NotFound => {
                Err(GitError::NotARepository(path.to_path_buf()))
            }
            Err(e) => Err(GitError::from_git2(&e)),
        }
    }

    pub(crate) fn from_git2(inner: git2::Repository, path: PathBuf) -> Repo {
        Repo { inner, path }
    }

    pub(crate) fn git(&self) -> &git2::Repository {
        &self.inner
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn summary(&self) -> Result<RepoSummary, GitError> {
        let name = self
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.path.display().to_string());
        let origin_url = self
            .inner
            .find_remote("origin")
            .ok()
            .and_then(|r| r.url().ok().map(str::to_owned));
        let (head, last_commit) = self.head_info()?;
        Ok(RepoSummary {
            name,
            path: self.path.clone(),
            head,
            origin_url,
            last_commit,
        })
    }

    fn head_info(&self) -> Result<(Head, Option<CommitInfo>), GitError> {
        let head_ref = match self.inner.head() {
            Ok(r) => r,
            Err(e) if e.code() == git2::ErrorCode::UnbornBranch => {
                let branch = self
                    .inner
                    .find_reference("HEAD")
                    .ok()
                    .and_then(|r| r.symbolic_target().ok().flatten().map(str::to_owned))
                    .map(|t| t.trim_start_matches("refs/heads/").to_string())
                    .unwrap_or_else(|| "HEAD".to_string());
                return Ok((Head::Unborn(branch), None));
            }
            Err(e) => return Err(GitError::from_git2(&e)),
        };
        let commit = head_ref
            .peel_to_commit()
            .map_err(|e| GitError::from_git2(&e))?;
        let info = CommitInfo {
            short_id: short_id(&commit),
            summary: commit.summary().ok().flatten().unwrap_or("").to_string(),
            author: commit.author().name().unwrap_or("").to_string(),
            time: commit.time().seconds(),
        };
        let detached = self
            .inner
            .head_detached()
            .map_err(|e| GitError::from_git2(&e))?;
        let head = if detached {
            Head::Detached(info.short_id.clone())
        } else {
            Head::Branch(head_ref.shorthand().unwrap_or("HEAD").to_string())
        };
        Ok((head, Some(info)))
    }
}

fn short_id(commit: &git2::Commit<'_>) -> String {
    let full = commit.id().to_string();
    full.chars().take(7).collect()
}
```

- [ ] **Step 9: Implement status**

For a staged rename, `entry.path()` is the *old* path; the new path comes from `head_to_index().new_file()`.

`crates/gitcore/src/status.rs`:

```rust
use crate::{GitError, Repo};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    Added,
    Modified,
    Deleted,
    Renamed { from: String },
    TypeChange,
    Untracked,
    Conflicted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileStatus {
    /// Path relative to the repository root, `/`-separated.
    pub path: String,
    /// Change between HEAD and the index.
    pub staged: Option<Change>,
    /// Change between the index and the working tree.
    pub unstaged: Option<Change>,
}

impl Repo {
    /// Working tree status, sorted by path. Ignored files are excluded.
    pub fn status(&self) -> Result<Vec<FileStatus>, GitError> {
        let mut opts = git2::StatusOptions::new();
        opts.include_untracked(true)
            .recurse_untracked_dirs(true)
            .include_ignored(false)
            .exclude_submodules(true)
            .renames_head_to_index(true);
        let statuses = self
            .git()
            .statuses(Some(&mut opts))
            .map_err(|e| GitError::from_git2(&e))?;
        let mut out: Vec<FileStatus> = statuses
            .iter()
            .filter_map(|entry| {
                let s = entry.status();
                // For a staged rename, `entry.path()` is the old path: use the new one.
                let renamed_to = entry
                    .head_to_index()
                    .filter(|_| s.is_index_renamed())
                    .and_then(|d| {
                        d.new_file()
                            .path()
                            .map(|p| p.to_string_lossy().replace('\\', "/"))
                    });
                let path = match renamed_to {
                    Some(p) => p,
                    None => entry.path().ok()?.to_string(),
                };
                if s.is_conflicted() {
                    return Some(FileStatus {
                        path,
                        staged: None,
                        unstaged: Some(Change::Conflicted),
                    });
                }
                let staged = if s.is_index_new() {
                    Some(Change::Added)
                } else if s.is_index_modified() {
                    Some(Change::Modified)
                } else if s.is_index_deleted() {
                    Some(Change::Deleted)
                } else if s.is_index_renamed() {
                    let from = entry
                        .head_to_index()
                        .and_then(|d| {
                            d.old_file()
                                .path()
                                .map(|p| p.to_string_lossy().replace('\\', "/"))
                        })
                        .unwrap_or_default();
                    Some(Change::Renamed { from })
                } else if s.is_index_typechange() {
                    Some(Change::TypeChange)
                } else {
                    None
                };
                let unstaged = if s.is_wt_new() {
                    Some(Change::Untracked)
                } else if s.is_wt_modified() {
                    Some(Change::Modified)
                } else if s.is_wt_deleted() {
                    Some(Change::Deleted)
                } else if s.is_wt_typechange() {
                    Some(Change::TypeChange)
                } else {
                    None
                };
                (staged.is_some() || unstaged.is_some()).then_some(FileStatus {
                    path,
                    staged,
                    unstaged,
                })
            })
            .collect();
        out.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(out)
    }
}
```

- [ ] **Step 10: Implement the diff model**

Each `DiffLine::text` keeps its own line ending, so a missing final newline is simply a line without `\n`; libgit2's `\ No newline` marker lines are skipped. libgit2 diffs the *filtered* working tree (autocrlf applied), so CRLF checkouts produce no fake changes.

`crates/gitcore/src/diff.rs`:

```rust
use std::path::Path;

use crate::{GitError, Repo};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Side {
    /// Index -> working tree.
    Unstaged,
    /// HEAD -> index.
    Staged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    Context,
    Added,
    Removed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: LineKind,
    pub old_no: Option<u32>,
    pub new_no: Option<u32>,
    /// Raw line content, including its line ending when it has one.
    pub text: String,
    /// The line is the last of its file and has no trailing newline.
    pub no_newline_at_eof: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
    pub header: String,
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
    pub lines: Vec<DiffLine>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDiff {
    pub path: String,
    pub side: Side,
    pub binary: bool,
    pub hunks: Vec<Hunk>,
}

impl FileDiff {
    pub fn line_count(&self) -> usize {
        self.hunks.iter().map(|h| h.lines.len()).sum()
    }
}

impl Repo {
    /// Unified diff (3 context lines) of one file. Untracked files diff against empty content.
    pub fn diff_file(&self, path: &str, side: Side) -> Result<FileDiff, GitError> {
        let repo = self.git();
        let mut opts = git2::DiffOptions::new();
        opts.pathspec(path)
            .disable_pathspec_match(true)
            .include_untracked(true)
            .recurse_untracked_dirs(true)
            .show_untracked_content(true)
            .context_lines(3);
        let map = |e: git2::Error| GitError::from_git2(&e);
        let diff = match side {
            Side::Unstaged => repo
                .diff_index_to_workdir(None, Some(&mut opts))
                .map_err(map)?,
            Side::Staged => {
                let head_tree = repo.head().ok().and_then(|h| h.peel_to_tree().ok());
                repo.diff_tree_to_index(head_tree.as_ref(), None, Some(&mut opts))
                    .map_err(map)?
            }
        };
        let mut out = FileDiff {
            path: path.to_string(),
            side,
            binary: false,
            hunks: Vec::new(),
        };
        for idx in 0..diff.deltas().len() {
            let Some(patch) = git2::Patch::from_diff(&diff, idx).map_err(map)? else {
                out.binary = true;
                continue;
            };
            if patch.delta().flags().is_binary() {
                out.binary = true;
                continue;
            }
            for h in 0..patch.num_hunks() {
                let (hunk, n) = patch.hunk(h).map_err(map)?;
                let mut lines = Vec::with_capacity(n);
                for l in 0..n {
                    let line = patch.line_in_hunk(h, l).map_err(map)?;
                    let kind = match line.origin() {
                        ' ' => LineKind::Context,
                        '+' => LineKind::Added,
                        '-' => LineKind::Removed,
                        // "\ No newline at end of file" markers: encoded in `text` already.
                        _ => continue,
                    };
                    let text = String::from_utf8_lossy(line.content()).into_owned();
                    lines.push(DiffLine {
                        kind,
                        old_no: line.old_lineno(),
                        new_no: line.new_lineno(),
                        no_newline_at_eof: !text.ends_with('\n'),
                        text,
                    });
                }
                out.hunks.push(Hunk {
                    header: String::from_utf8_lossy(hunk.header())
                        .trim_end()
                        .to_string(),
                    old_start: hunk.old_start(),
                    old_lines: hunk.old_lines(),
                    new_start: hunk.new_start(),
                    new_lines: hunk.new_lines(),
                    lines,
                });
            }
        }
        Ok(out)
    }

    pub(crate) fn workdir(&self) -> Result<&Path, GitError> {
        self.git()
            .workdir()
            .ok_or_else(|| GitError::Unsupported("bare repositories are not supported".into()))
    }
}
```

- [ ] **Step 11: Implement partial staging**

How `apply_selection` works: walk the base (index) lines; context lines are copied; a line present in the base (`-` when staging, `+` when unstaging) is dropped only if selected; a line absent from the base is inserted only if selected. `push_line` inserts a newline before appending after a line that had none, so lines are never glued. For a zero-line hunk range, `start` is the line *after which* the hunk applies. Any mismatch between the diff and the base is reported as `StaleSelection`.

`crates/gitcore/src/stage.rs`:

```rust
use std::collections::BTreeSet;
use std::path::Path;

use crate::diff::{FileDiff, LineKind, Side};
use crate::{GitError, Repo};

/// What to stage or unstage in one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selection {
    All,
    /// Indexes into `FileDiff::hunks`.
    Hunks(Vec<usize>),
    /// `(hunk index, line index within the hunk)`; only added/removed lines matter.
    Lines(Vec<(usize, usize)>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Stage,
    Unstage,
}

impl Selection {
    fn picks(&self, hunk: usize, line: usize) -> bool {
        match self {
            Selection::All => true,
            Selection::Hunks(h) => h.contains(&hunk),
            Selection::Lines(l) => l.contains(&(hunk, line)),
        }
    }
}

fn push_line(out: &mut Vec<u8>, line: &[u8]) {
    // Never glue two lines together when the previous one had no trailing newline.
    if !out.is_empty() && out.last() != Some(&b'\n') {
        out.push(b'\n');
    }
    out.extend_from_slice(line);
}

/// Compute the new index content of a file.
///
/// - `Stage`: `base` is the index content, `diff` the unstaged diff (index -> working tree).
/// - `Unstage`: `base` is the index content, `diff` the staged diff (HEAD -> index).
///
/// Unselected changes are left as they are in `base`; selected ones are applied (stage)
/// or reverted (unstage). Same result as `git add -p` / `git reset -p`.
pub fn apply_selection(
    base: &[u8],
    diff: &FileDiff,
    selection: &Selection,
    direction: Direction,
) -> Result<Vec<u8>, GitError> {
    let lines: Vec<&[u8]> = base.split_inclusive(|b| *b == b'\n').collect();
    let mut out = Vec::with_capacity(base.len());
    let mut cursor = 0usize; // number of base lines consumed
    // In `base`, the lines we walk are the diff's old side (stage) or new side (unstage).
    let (base_kind, other_kind) = match direction {
        Direction::Stage => (LineKind::Removed, LineKind::Added),
        Direction::Unstage => (LineKind::Added, LineKind::Removed),
    };
    let stale = || GitError::StaleSelection;
    for (h, hunk) in diff.hunks.iter().enumerate() {
        let (start, count) = match direction {
            Direction::Stage => (hunk.old_start as usize, hunk.old_lines),
            Direction::Unstage => (hunk.new_start as usize, hunk.new_lines),
        };
        // With a zero-line range, `start` is the line *after which* the hunk applies.
        let first = if count == 0 {
            start
        } else {
            start.saturating_sub(1)
        };
        if first < cursor || first > lines.len() {
            return Err(stale());
        }
        for l in &lines[cursor..first] {
            push_line(&mut out, l);
        }
        cursor = first;
        for (i, line) in hunk.lines.iter().enumerate() {
            if line.kind == LineKind::Context {
                let l = lines.get(cursor).ok_or_else(stale)?;
                push_line(&mut out, l);
                cursor += 1;
            } else if line.kind == base_kind {
                // Present in base: kept unless selected.
                let l = lines.get(cursor).ok_or_else(stale)?;
                if !selection.picks(h, i) {
                    push_line(&mut out, l);
                }
                cursor += 1;
            } else if line.kind == other_kind && selection.picks(h, i) {
                // Absent from base: inserted only if selected.
                push_line(&mut out, line.text.as_bytes());
            }
        }
    }
    for l in &lines[cursor.min(lines.len())..] {
        push_line(&mut out, l);
    }
    Ok(out)
}

impl Repo {
    /// Stage a selection of the unstaged changes of `path`.
    /// `shown` is the diff the user made the selection on (ignored for `Selection::All`).
    pub fn stage(
        &self,
        path: &str,
        selection: &Selection,
        shown: Option<&FileDiff>,
    ) -> Result<(), GitError> {
        self.change_index(path, selection, shown, Direction::Stage)
    }

    /// Unstage a selection of the staged changes of `path`.
    pub fn unstage(
        &self,
        path: &str,
        selection: &Selection,
        shown: Option<&FileDiff>,
    ) -> Result<(), GitError> {
        self.change_index(path, selection, shown, Direction::Unstage)
    }

    fn change_index(
        &self,
        path: &str,
        selection: &Selection,
        shown: Option<&FileDiff>,
        direction: Direction,
    ) -> Result<(), GitError> {
        let repo = self.git();
        let map = |e: git2::Error| GitError::from_git2(&e);
        let rel = Path::new(path);
        if *selection == Selection::All {
            return match direction {
                Direction::Stage => {
                    let mut index = repo.index().map_err(map)?;
                    if self.workdir()?.join(rel).symlink_metadata().is_ok() {
                        index.add_path(rel).map_err(map)?;
                    } else {
                        index.remove_path(rel).map_err(map)?;
                    }
                    index.write().map_err(map)
                }
                Direction::Unstage => {
                    let head = repo.head().ok().and_then(|h| h.peel_to_commit().ok());
                    let target = head.as_ref().map(|c| c.as_object());
                    repo.reset_default(target, [path]).map_err(map)
                }
            };
        }

        let side = match direction {
            Direction::Stage => Side::Unstaged,
            Direction::Unstage => Side::Staged,
        };
        let current = self.diff_file(path, side)?;
        if shown.is_some_and(|s| *s != current) {
            return Err(GitError::StaleSelection);
        }
        if current.binary {
            return Err(GitError::Unsupported(
                "binary files can only be staged as a whole".into(),
            ));
        }
        let mut index = repo.index().map_err(map)?;
        let existing = index.get_path(rel, 0);
        let is_deletion = match direction {
            Direction::Stage => self.workdir()?.join(rel).symlink_metadata().is_err(),
            Direction::Unstage => existing.is_none(),
        };
        if is_deletion {
            return Err(GitError::Unsupported(
                "deleted files can only be staged as a whole".into(),
            ));
        }
        let base = match &existing {
            Some(entry) => repo.find_blob(entry.id).map_err(map)?.content().to_vec(),
            None => Vec::new(),
        };
        let content = apply_selection(&base, &current, selection, direction)?;
        let entry = match existing {
            Some(e) => e,
            None => new_entry(path, self.file_mode(rel)),
        };
        index.add_frombuffer(&entry, &content).map_err(map)?;
        index.write().map_err(map)?;
        // Unstaging everything of a file that is new in the index leaves an empty entry:
        // drop it so the file goes back to "untracked".
        if direction == Direction::Unstage && content.is_empty() {
            let head_has = repo
                .head()
                .ok()
                .and_then(|h| h.peel_to_tree().ok())
                .is_some_and(|t| t.get_path(rel).is_ok());
            if !head_has {
                let mut index = repo.index().map_err(map)?;
                index.remove_path(rel).map_err(map)?;
                index.write().map_err(map)?;
            }
        }
        Ok(())
    }

    fn file_mode(&self, rel: &Path) -> u32 {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(dir) = self.workdir()
                && let Ok(meta) = std::fs::metadata(dir.join(rel))
                && meta.permissions().mode() & 0o111 != 0
            {
                return 0o100755;
            }
        }
        let _ = rel;
        0o100644
    }
}

fn new_entry(path: &str, mode: u32) -> git2::IndexEntry {
    git2::IndexEntry {
        ctime: git2::IndexTime::new(0, 0),
        mtime: git2::IndexTime::new(0, 0),
        dev: 0,
        ino: 0,
        mode,
        uid: 0,
        gid: 0,
        file_size: 0,
        id: git2::Oid::ZERO_SHA1,
        flags: 0,
        flags_extended: 0,
        path: path.as_bytes().to_vec(),
    }
}

/// Selected `(hunk, line)` pairs that are real changes (context lines are never selectable).
pub fn selectable_lines(diff: &FileDiff) -> BTreeSet<(usize, usize)> {
    diff.hunks
        .iter()
        .enumerate()
        .flat_map(|(h, hunk)| {
            hunk.lines
                .iter()
                .enumerate()
                .filter(|(_, l)| l.kind != LineKind::Context)
                .map(move |(i, _)| (h, i))
        })
        .collect()
}
```

- [ ] **Step 12: Add every S2 user-visible string**

Replace the whole file with the version below (it keeps everything from sub-project 1).

`crates/app/src/strings.rs`:

```rust
//! Every user-visible string, in one place.

pub const APP_NAME: &str = "RetroGit";

pub const MENU_FILE: &str = "File";
pub const MENU_REPOSITORY: &str = "Repository";
pub const MENU_VIEW: &str = "View";
pub const MENU_HELP: &str = "Help";
pub const SIGN_IN_MENU: &str = "Sign in...";
pub const SIGN_OUT: &str = "Sign out";
pub const CLONE_MENU: &str = "Clone...";
pub const OPEN_MENU: &str = "Open...";
pub const EXIT: &str = "Exit";
pub const REFRESH_REPO: &str = "Refresh";
pub const ABOUT_MENU: &str = "About RetroGit";

pub const CLONE: &str = "Clone";
pub const OPEN: &str = "Open";
pub const FETCH: &str = "Fetch";
pub const PULL: &str = "Pull";
pub const PUSH: &str = "Push";

pub const REPOSITORIES: &str = "Repositories";
pub const REMOVE_FROM_LIST: &str = "Remove from list";
pub const NO_REPO: &str = "No repository open. Use File > Clone... or File > Open...";
pub const FOLDER_MISSING: &str = "This folder no longer exists.";
pub const BRANCH: &str = "Branch:";
pub const DETACHED: &str = "(detached)";
pub const NO_COMMITS: &str = "(no commits yet)";
pub const REMOTE: &str = "Remote:";
pub const NO_REMOTE: &str = "(none)";
pub const LAST_COMMIT: &str = "Last commit:";
pub const READY: &str = "Ready";
pub const NOT_SIGNED_IN: &str = "Not signed in";
pub const CHECKING: &str = "Checking GitHub session...";
pub const OFFLINE: &str = "Offline - GitHub unreachable";

pub const SIGN_IN_TITLE: &str = "Sign in to GitHub";
pub const TAB_STANDARD: &str = "Standard";
pub const TAB_ADVANCED: &str = "Advanced";
pub const SIGN_IN_INTRO: &str =
    "RetroGit will show a code. Enter it on github.com to authorize this computer.";
pub const SIGN_IN_BUTTON: &str = "Sign in";
pub const DEVICE_GO_TO: &str = "Go to:";
pub const DEVICE_ENTER_CODE: &str = "and enter this code:";
pub const COPY_CODE: &str = "Copy code";
pub const OPEN_BROWSER: &str = "Open browser";
pub const WAITING_AUTH: &str = "Waiting for authorization...";
pub const PAT_LABEL: &str = "Personal access token:";
pub const PAT_HELP: &str = "Use a token with 'repo' and 'read:org' scopes, authorized for SSO.";
pub const OK: &str = "OK";
pub const CANCEL: &str = "Cancel";

pub const CLONE_TITLE: &str = "Clone a repository";
pub const FILTER: &str = "Filter:";
pub const REFRESH: &str = "Refresh";
pub const LOADING: &str = "Loading repositories...";
pub const COL_NAME: &str = "Name";
pub const COL_OWNER: &str = "Owner";
pub const COL_PRIVATE: &str = "Private";
pub const COL_UPDATED: &str = "Updated";
pub const YES: &str = "Yes";
pub const DEST_FOLDER: &str = "Destination folder:";
pub const BROWSE: &str = "Browse...";
pub const WILL_CLONE_INTO: &str = "Will be cloned into:";
pub const CLONING_TITLE: &str = "Cloning";
pub const RECEIVING: &str = "Receiving objects:";

pub const STAGED_CHANGES: &str = "Staged changes";
pub const CHANGES: &str = "Changes";
pub const STAGE_ALL: &str = "Stage all";
pub const UNSTAGE_ALL: &str = "Unstage all";
pub const STAGE: &str = "Stage";
pub const UNSTAGE: &str = "Unstage";
pub const IGNORE_FILE: &str = "Add to .gitignore";
pub const IGNORE_EXT: &str = "Add *.{ext} to .gitignore";
pub const STAGE_LINES: &str = "Stage selected lines";
pub const UNSTAGE_LINES: &str = "Unstage selected lines";
pub const STAGE_HUNK: &str = "Stage hunk";
pub const UNSTAGE_HUNK: &str = "Unstage hunk";
pub const STAGE_FILE: &str = "Stage file";
pub const UNSTAGE_FILE: &str = "Unstage file";
pub const BINARY_FILE: &str = "Binary file";
pub const DIFF_TOO_LARGE: &str = "Diff too large to display.";
pub const SHOW_ANYWAY: &str = "Show anyway";
pub const RESOLVE_CONFLICTS: &str = "This file has conflicts. Resolve conflicts first.";
pub const NO_DIFF: &str = "No changes to show.";
pub const SELECT_A_FILE: &str = "Select a file to see its changes.";
pub const WORKING_TREE_CLEAN: &str = "Nothing to commit, working tree clean.";
pub const SIDE_UNSTAGED: &str = "(unstaged)";
pub const SIDE_STAGED: &str = "(staged)";
pub const SUMMARY: &str = "Summary:";
pub const DESCRIPTION: &str = "Description:";
pub const AMEND: &str = "Amend last commit";
pub const AMEND_PUSHED_WARNING: &str =
    "This commit is already pushed: amending it will require a force push.";
pub const COMMIT: &str = "Commit";
pub const COMMITTING: &str = "Committing...";
pub const COMMITTED: &str = "Committed";
pub const COMMIT_MENU: &str = "Commit...";
pub const ABOUT_TITLE: &str = "About RetroGit";
pub const ABOUT_TAGLINE: &str = "A Git client with a Windows 95 look.";
pub const ABOUT_FONT: &str = "Font: W95FA by Alina Sava (SIL Open Font License 1.1)";

pub const ERR_TITLE: &str = "RetroGit";
pub const ERR_NO_NETWORK: &str = "Could not reach GitHub. Check your network connection.";
pub const ERR_UNAUTHORIZED: &str = "GitHub rejected your session. Please sign in again.";
pub const ERR_PAT_REJECTED: &str = "GitHub rejected this token.";
pub const ERR_SSO: &str = "Your organization requires SSO authorization for this token. Open the link below, authorize, then try again.";
pub const ERR_SSO_PARTIAL: &str = "Some organization repositories are hidden because RetroGit is not authorized for their SSO. Open the link below, grant access (or authorize your token for SSO), then click Refresh.";
pub const ERR_RATE_LIMIT: &str = "GitHub API rate limit reached. Try again in a few minutes.";
pub const ERR_DEVICE_EXPIRED: &str = "The sign-in code expired. Click Sign in to get a new one.";
pub const ERR_DEVICE_DENIED: &str = "Authorization was denied on github.com.";
pub const ERR_NO_CLIENT_ID: &str = "This build has no GitHub OAuth App client ID. Use the Advanced tab to paste a personal access token.";
pub const ERR_DEST_NOT_EMPTY: &str = "The destination folder already exists and is not empty.";
pub const ERR_NOT_A_REPO: &str = "This folder is not a Git repository.";
pub const ERR_GIT_AUTH: &str = "GitHub refused access to this repository. Check that your token is authorized for the organization (SSO).";
pub const ERR_KEYCHAIN: &str = "Could not access the system credential store.";
pub const ERR_INTERNAL: &str = "An internal error occurred. Details were written to the log file.";
pub const INFO_STALE_SELECTION: &str =
    "The file changed since its diff was shown. The diff was reloaded: please select again.";
pub const ERR_COMMIT_REJECTED: &str = "The commit was rejected (hook or signing). Git's output:";
pub const ERR_MISSING_IDENTITY: &str = "Git does not know who you are. Run:\n  git config --global user.name \"Your Name\"\n  git config --global user.email \"you@example.com\"";
pub const WARN_NO_GIT_CLI: &str = "git was not found: this commit was made without hooks and without signing. Install Git to enable them.";
pub const INFO_CLONE_CANCELLED: &str = "Clone cancelled.";
```

- [ ] **Step 13: Map the new errors for message boxes**

Replace the whole file with the version below (it keeps everything from sub-project 1). Only change: four new arms in `AppError::from_git`.

`crates/app/src/protocol.rs`:

```rust
//! Messages between the UI thread and the worker thread.

use std::path::PathBuf;

use gitcore::{CloneProgress, GitError, RepoSummary};
use github::{DeviceFlowFailure, GithubError, RepoInfo, TokenStoreError, User};

use crate::strings as s;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    ValidateToken,
    StartDeviceFlow,
    SavePat(String),
    SignOut,
    ListRepos,
    Clone { url: String, dest: PathBuf },
    OpenRepo(PathBuf),
}

/// Which operation an error belongs to, so the state can reset the right thing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Op {
    Auth,
    Repos,
    Clone,
    Open(PathBuf),
    Internal,
}

#[derive(Debug, Clone)]
pub enum Event {
    SignedIn(User),
    SignedOut,
    /// A token is stored but GitHub could not be reached; it is kept for later calls.
    Offline,
    DeviceCode {
        user_code: String,
        verification_uri: String,
    },
    DeviceFlowCancelled,
    ReposLoaded(Vec<RepoInfo>),
    CloneProgress(CloneProgress),
    CloneDone(RepoSummary),
    CloneCancelled,
    RepoOpened(RepoSummary),
    Error {
        during: Op,
        error: AppError,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
    Info,
}

/// What the message box shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppError {
    pub severity: Severity,
    pub message: String,
    /// Technical detail (already redacted), shown in small text.
    pub detail: Option<String>,
    /// Clickable link (e.g. SSO authorization page).
    pub link: Option<String>,
}

impl AppError {
    pub fn new(severity: Severity, message: &str) -> AppError {
        AppError {
            severity,
            message: message.to_string(),
            detail: None,
            link: None,
        }
    }

    fn with_detail(mut self, detail: impl ToString) -> AppError {
        self.detail = Some(detail.to_string());
        self
    }

    pub fn from_github(e: &GithubError) -> AppError {
        match e {
            GithubError::Unauthorized => AppError::new(Severity::Warning, s::ERR_UNAUTHORIZED),
            GithubError::SsoRequired { url } => {
                let mut a = AppError::new(Severity::Warning, s::ERR_SSO);
                a.link = Some(url.clone());
                a
            }
            GithubError::RateLimited => AppError::new(Severity::Warning, s::ERR_RATE_LIMIT),
            GithubError::Network(d) => {
                AppError::new(Severity::Warning, s::ERR_NO_NETWORK).with_detail(d)
            }
            other => AppError::new(Severity::Error, &other.to_string()),
        }
    }

    pub fn from_git(e: &GitError) -> AppError {
        match e {
            GitError::DestinationNotEmpty(p) => {
                AppError::new(Severity::Error, s::ERR_DEST_NOT_EMPTY).with_detail(p.display())
            }
            GitError::NotARepository(p) => {
                AppError::new(Severity::Error, s::ERR_NOT_A_REPO).with_detail(p.display())
            }
            GitError::Cancelled => AppError::new(Severity::Info, s::INFO_CLONE_CANCELLED),
            GitError::Auth(d) => AppError::new(Severity::Error, s::ERR_GIT_AUTH).with_detail(d),
            GitError::Network(d) => {
                AppError::new(Severity::Warning, s::ERR_NO_NETWORK).with_detail(d)
            }
            GitError::StaleSelection => AppError::new(Severity::Info, s::INFO_STALE_SELECTION),
            GitError::Unsupported(d) => AppError::new(Severity::Warning, d),
            GitError::CommitRejected { output } => {
                AppError::new(Severity::Error, s::ERR_COMMIT_REJECTED).with_detail(output)
            }
            GitError::MissingIdentity => AppError::new(Severity::Error, s::ERR_MISSING_IDENTITY),
            GitError::Other(d) => AppError::new(Severity::Error, d),
        }
    }

    pub fn from_device_flow(f: &DeviceFlowFailure) -> AppError {
        match f {
            DeviceFlowFailure::Expired => AppError::new(Severity::Warning, s::ERR_DEVICE_EXPIRED),
            DeviceFlowFailure::Denied => AppError::new(Severity::Warning, s::ERR_DEVICE_DENIED),
            DeviceFlowFailure::Other(code) => {
                AppError::new(Severity::Error, &format!("GitHub sign-in failed: {code}"))
            }
        }
    }

    pub fn from_store(e: &TokenStoreError) -> AppError {
        AppError::new(Severity::Error, s::ERR_KEYCHAIN).with_detail(e)
    }
}
```

- [ ] **Step 14: Run the tests to see them pass**

Run: `cargo test -p gitcore`

Expected: `tests/stage.rs`: 19 passed; `tests/status_diff.rs`: 7 passed; the S1 suites still pass.

- [ ] **Step 15: Lint**

Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings`

Expected: no output from fmt; clippy finishes without warnings.

- [ ] **Step 16: Full suite**

Run: `cargo test --workspace`

Expected: all tests pass (0 failed).

- [ ] **Step 17: Commit**

```bash
git add crates/gitcore crates/app/src/strings.rs crates/app/src/protocol.rs
git commit -m "feat(gitcore): status, diff and line-level staging"
```


### Task 2: `gitcore` commit (CLI with libgit2 fallback), amend helpers, .gitignore

**Files:**
- Create: `crates/gitcore/src/commit.rs`, `crates/gitcore/src/ignore.rs`
- Modify: `crates/gitcore/src/lib.rs`
- Test: `crates/gitcore/tests/commit.rs`

**Interfaces:**
- Consumes: `Repo::git`, `Repo::workdir`, `Repo::summary`, `GitError` (Task 1).
- Produces: `CommitBackend::{PreferCli, Git2}`; `CommitOutcome { commit: CommitInfo, used_cli: bool }`; `Repo::commit(&self, message: &str, amend: bool, backend: CommitBackend) -> Result<CommitOutcome, GitError>`; `Repo::last_commit_message(&self) -> Result<Option<String>, GitError>`; `Repo::head_is_pushed(&self) -> Result<bool, GitError>`; `Repo::add_to_gitignore(&self, pattern: &str) -> Result<(), GitError>`; free functions `git_available() -> bool`, `set_git_search_path(String)`, `classify_commit_failure(&str) -> GitError`.

- [ ] **Step 1: Write the failing tests**

`repo_with_identity` sets identity, disables signing and pins `core.hooksPath` locally so the developer's global config cannot interfere. The hook test is Unix-only (it needs an executable shell script).

`crates/gitcore/tests/commit.rs`:

```rust
#![allow(clippy::unwrap_used)]
mod common;

use std::path::Path;
use std::sync::atomic::AtomicBool;

use gitcore::{CommitBackend, GitError, Repo, Selection, classify_commit_failure, git_available};

/// Repo isolated from the developer's global config (identity, signing, hooks path).
fn repo_with_identity(commits: usize) -> (tempfile::TempDir, Repo) {
    let d = tempfile::tempdir().unwrap();
    let repo = common::make_repo(d.path(), commits);
    let mut cfg = repo.config().unwrap();
    cfg.set_str("user.name", "Ada").unwrap();
    cfg.set_str("user.email", "ada@example.com").unwrap();
    cfg.set_bool("commit.gpgsign", false).unwrap();
    cfg.set_str("core.hooksPath", ".git/hooks").unwrap();
    let r = Repo::open(d.path()).unwrap();
    (d, r)
}

fn stage_new_file(d: &Path, r: &Repo, name: &str) {
    std::fs::write(d.join(name), "content\n").unwrap();
    r.stage(name, &Selection::All, None).unwrap();
}

#[test]
fn git2_commit_and_amend() {
    let (d, r) = repo_with_identity(1);
    stage_new_file(d.path(), &r, "a.txt");
    let out = r
        .commit("Add a\n\nBody", false, CommitBackend::Git2)
        .unwrap();
    assert!(!out.used_cli);
    assert_eq!(out.commit.summary, "Add a");
    assert_eq!(
        r.last_commit_message().unwrap().as_deref(),
        Some("Add a\n\nBody")
    );
    assert!(r.status().unwrap().is_empty());

    let amended = r
        .commit("Add a (fixed)", true, CommitBackend::Git2)
        .unwrap();
    assert_eq!(amended.commit.summary, "Add a (fixed)");
    assert_ne!(amended.commit.short_id, out.commit.short_id);
    let repo = git2::Repository::open(d.path()).unwrap();
    let head = repo.head().unwrap().peel_to_commit().unwrap();
    assert_eq!(head.parent(0).unwrap().summary().unwrap(), Some("commit 0"));
}

#[test]
fn git2_first_commit_of_an_empty_repo() {
    let (d, r) = repo_with_identity(0);
    stage_new_file(d.path(), &r, "a.txt");
    assert_eq!(
        r.commit("first", false, CommitBackend::Git2)
            .unwrap()
            .commit
            .summary,
        "first"
    );
    assert!(matches!(
        repo_with_identity(0)
            .1
            .commit("x", true, CommitBackend::Git2),
        Err(GitError::Unsupported(_))
    ));
}

#[test]
fn cli_commit_runs_hooks_and_reports_their_output() {
    if !git_available() {
        return;
    }
    let (d, r) = repo_with_identity(1);
    stage_new_file(d.path(), &r, "a.txt");
    let ok = r
        .commit("via cli", false, CommitBackend::PreferCli)
        .unwrap();
    assert!(ok.used_cli);
    assert_eq!(ok.commit.summary, "via cli");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let hook = d.path().join(".git/hooks/pre-commit");
        std::fs::create_dir_all(hook.parent().unwrap()).unwrap();
        std::fs::write(&hook, "#!/bin/sh\necho 'lint: 3 errors' >&2\nexit 1\n").unwrap();
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
        stage_new_file(d.path(), &r, "b.txt");
        match r.commit("blocked", false, CommitBackend::PreferCli) {
            Err(GitError::CommitRejected { output }) => {
                assert!(output.contains("lint: 3 errors"), "{output}")
            }
            other => panic!("expected CommitRejected, got {other:?}"),
        }
        assert_eq!(r.summary().unwrap().last_commit.unwrap().summary, "via cli");
    }
}

#[test]
fn cli_amend_replaces_head() {
    if !git_available() {
        return;
    }
    let (d, r) = repo_with_identity(1);
    stage_new_file(d.path(), &r, "a.txt");
    r.commit("first try", false, CommitBackend::PreferCli)
        .unwrap();
    let out = r
        .commit("second try", true, CommitBackend::PreferCli)
        .unwrap();
    assert_eq!(out.commit.summary, "second try");
    let repo = git2::Repository::open(d.path()).unwrap();
    assert_eq!(
        repo.head()
            .unwrap()
            .peel_to_commit()
            .unwrap()
            .parent_count(),
        1
    );
}

#[test]
fn failure_classification() {
    assert_eq!(
        classify_commit_failure("*** Please tell me who you are.\n\nRun git config"),
        GitError::MissingIdentity
    );
    assert_eq!(
        classify_commit_failure("  error: gpg failed to sign the data\n"),
        GitError::CommitRejected {
            output: "error: gpg failed to sign the data".into()
        }
    );
    let long = "é".repeat(15_000);
    match classify_commit_failure(&long) {
        GitError::CommitRejected { output } => {
            assert!(output.len() <= 20_010 && output.ends_with("[...]"))
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn head_is_pushed_tracks_the_upstream() {
    let (src, _) = repo_with_identity(2);
    let out = tempfile::tempdir().unwrap();
    let dest = out.path().join("clone");
    let url = common::file_url(src.path());
    let req = gitcore::CloneRequest {
        url,
        dest: dest.clone(),
        credentials: None,
    };
    let cloned = gitcore::clone(&req, |_| {}, &AtomicBool::new(false)).unwrap();
    assert!(cloned.head_is_pushed().unwrap());
    let repo = git2::Repository::open(&dest).unwrap();
    let mut cfg = repo.config().unwrap();
    cfg.set_str("user.name", "Ada").unwrap();
    cfg.set_str("user.email", "ada@example.com").unwrap();
    stage_new_file(&dest, &cloned, "local.txt");
    cloned
        .commit("local only", false, CommitBackend::Git2)
        .unwrap();
    assert!(!cloned.head_is_pushed().unwrap());
    let (_d, no_upstream) = repo_with_identity(1);
    assert!(!no_upstream.head_is_pushed().unwrap());
}

#[test]
fn add_to_gitignore_creates_appends_and_dedupes() {
    let (d, r) = repo_with_identity(1);
    r.add_to_gitignore("*.log").unwrap();
    assert_eq!(
        std::fs::read_to_string(d.path().join(".gitignore")).unwrap(),
        "*.log\n"
    );
    std::fs::write(d.path().join(".gitignore"), "target/").unwrap();
    r.add_to_gitignore("*.log").unwrap();
    r.add_to_gitignore(" *.log ").unwrap();
    assert_eq!(
        std::fs::read_to_string(d.path().join(".gitignore")).unwrap(),
        "target/\n*.log\n"
    );
    assert!(matches!(
        r.add_to_gitignore("  "),
        Err(GitError::Unsupported(_))
    ));
}
```

- [ ] **Step 2: Export the new modules**

Replace the whole file with the version below (it keeps everything from sub-project 1).

`crates/gitcore/src/lib.rs`:

```rust
//! RetroGit's internal Git API. No `git2` type is ever exposed publicly.

mod clone;
mod commit;
mod diff;
mod error;
mod ignore;
mod repo;
mod stage;
mod status;

pub use clone::{CloneProgress, CloneRequest, Credentials, clone};
pub use commit::{
    CommitBackend, CommitOutcome, classify_commit_failure, git_available, set_git_search_path,
};
pub use diff::{DiffLine, FileDiff, Hunk, LineKind, Side};
pub use error::GitError;
pub use repo::{CommitInfo, Head, Repo, RepoSummary};
pub use stage::{Direction, Selection, apply_selection, selectable_lines};
pub use status::{Change, FileStatus};

/// Bound libgit2's network waits (default: infinite) so a stalled clone eventually fails
/// and can be cleaned up. Call once at startup, before any network operation.
#[allow(unsafe_code)]
pub fn configure_network_timeouts() -> Result<(), GitError> {
    // SAFETY: git2 marks these unsafe only because they mutate libgit2 global state;
    // calling them before any other thread uses libgit2 is sound.
    unsafe {
        git2::opts::set_server_connect_timeout_in_milliseconds(15_000)
            .and_then(|()| git2::opts::set_server_timeout_in_milliseconds(60_000))
            .map_err(|e| GitError::Other(e.message().to_string()))
    }
}
```

- [ ] **Step 3: Run the tests to see them fail**

Run: `cargo test -p gitcore --test commit`

Expected: compile errors: `file not found for module commit` / `ignore`.

- [ ] **Step 4: Implement commit**

- `git -C <workdir> commit --cleanup=strip -F -` with the message on stdin; `GIT_EDITOR=true` and `GIT_TERMINAL_PROMPT=0` guarantee git never waits for a terminal.
- On Windows, `CREATE_NO_WINDOW` stops a console from flashing on every commit.
- `SEARCH_PATH` (set once by the app) is used both to find `git` and as the hooks' PATH.
- Output of a failed commit is truncated to 20 000 characters on a UTF-8 boundary.

`crates/gitcore/src/commit.rs`:

```rust
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::OnceLock;

use crate::{CommitInfo, GitError, Repo};

/// How to create commits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommitBackend {
    /// `git commit` (hooks, signing, global config); falls back to libgit2 if `git` is missing.
    PreferCli,
    /// libgit2 only: no hooks, no signing.
    Git2,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitOutcome {
    pub commit: CommitInfo,
    /// `false` when the commit was made by libgit2 (hooks and signing skipped).
    pub used_cli: bool,
}

const MAX_OUTPUT: usize = 20_000;
static SEARCH_PATH: OnceLock<String> = OnceLock::new();
static GIT_AVAILABLE: OnceLock<bool> = OnceLock::new();

/// PATH used to find `git` and given to hooks. GUI apps on macOS start with a minimal PATH
/// (no Homebrew), so the app passes the login shell's PATH here once at startup.
pub fn set_git_search_path(path: String) {
    let _ = SEARCH_PATH.set(path);
}

fn git_command() -> Command {
    let mut cmd = Command::new("git");
    if let Some(p) = SEARCH_PATH.get() {
        cmd.env("PATH", p);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// Whether a `git` executable can be run.
pub fn git_available() -> bool {
    *GIT_AVAILABLE.get_or_init(|| {
        git_command()
            .arg("--version")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    })
}

/// Turn the output of a failed `git commit` into an error.
pub fn classify_commit_failure(output: &str) -> GitError {
    if output.contains("Please tell me who you are") || output.contains("Author identity unknown") {
        return GitError::MissingIdentity;
    }
    let mut output = output.trim().to_string();
    if output.len() > MAX_OUTPUT {
        let mut cut = MAX_OUTPUT;
        while !output.is_char_boundary(cut) {
            cut -= 1;
        }
        output.truncate(cut);
        output.push_str("\n[...]");
    }
    GitError::CommitRejected { output }
}

impl Repo {
    /// Commit the index. With `amend`, replace HEAD (keeping its parents).
    pub fn commit(
        &self,
        message: &str,
        amend: bool,
        backend: CommitBackend,
    ) -> Result<CommitOutcome, GitError> {
        let used_cli = backend == CommitBackend::PreferCli && git_available();
        if used_cli {
            self.commit_cli(message, amend)?;
        } else {
            self.commit_git2(message, amend)?;
        }
        let commit = self
            .summary()?
            .last_commit
            .ok_or_else(|| GitError::Other("commit not found".into()))?;
        Ok(CommitOutcome { commit, used_cli })
    }

    fn commit_cli(&self, message: &str, amend: bool) -> Result<(), GitError> {
        let mut cmd = git_command();
        cmd.arg("-C")
            .arg(self.workdir()?)
            .args(["commit", "--cleanup=strip", "-F", "-"]);
        if amend {
            cmd.arg("--amend");
        }
        let mut child = cmd
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_EDITOR", "true")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| GitError::Other(format!("cannot run git: {e}")))?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(message.as_bytes())
                .map_err(|e| GitError::Other(format!("cannot send the message to git: {e}")))?;
        }
        let out = child
            .wait_with_output()
            .map_err(|e| GitError::Other(format!("git failed: {e}")))?;
        if out.status.success() {
            return Ok(());
        }
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        Err(classify_commit_failure(&text))
    }

    fn commit_git2(&self, message: &str, amend: bool) -> Result<(), GitError> {
        let repo = self.git();
        let map = |e: git2::Error| GitError::from_git2(&e);
        let sig = repo.signature().map_err(|_| GitError::MissingIdentity)?;
        let tree = {
            let mut index = repo.index().map_err(map)?;
            let id = index.write_tree().map_err(map)?;
            repo.find_tree(id).map_err(map)?
        };
        let head = repo.head().ok().and_then(|h| h.peel_to_commit().ok());
        match (amend, head) {
            (true, Some(head)) => {
                head.amend(
                    Some("HEAD"),
                    None,
                    Some(&sig),
                    None,
                    Some(message),
                    Some(&tree),
                )
                .map_err(map)?;
            }
            (true, None) => {
                return Err(GitError::Unsupported("there is no commit to amend".into()));
            }
            (false, head) => {
                let parents: Vec<&git2::Commit<'_>> = head.iter().collect();
                repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &parents)
                    .map_err(map)?;
            }
        }
        Ok(())
    }

    /// Full message of HEAD, if any (used to pre-fill "Amend").
    pub fn last_commit_message(&self) -> Result<Option<String>, GitError> {
        Ok(self
            .git()
            .head()
            .ok()
            .and_then(|h| h.peel_to_commit().ok())
            .and_then(|c| c.message().ok().map(|m| m.trim_end().to_string())))
    }

    /// `true` if the current branch has an upstream that already contains HEAD.
    pub fn head_is_pushed(&self) -> Result<bool, GitError> {
        let repo = self.git();
        let Ok(head) = repo.head() else {
            return Ok(false);
        };
        if !head.is_branch() {
            return Ok(false);
        }
        let Some(head_id) = head.target() else {
            return Ok(false);
        };
        let Ok(name) = head.shorthand() else {
            return Ok(false);
        };
        let Ok(branch) = repo.find_branch(name, git2::BranchType::Local) else {
            return Ok(false);
        };
        let Ok(upstream) = branch.upstream() else {
            return Ok(false);
        };
        let Some(up_id) = upstream.get().target() else {
            return Ok(false);
        };
        Ok(up_id == head_id || repo.graph_descendant_of(up_id, head_id).unwrap_or(false))
    }
}
```

- [ ] **Step 5: Implement .gitignore updates**

`crates/gitcore/src/ignore.rs`:

```rust
use crate::{GitError, Repo};

impl Repo {
    /// Append `pattern` to the root `.gitignore` (created if needed, no duplicates).
    pub fn add_to_gitignore(&self, pattern: &str) -> Result<(), GitError> {
        let pattern = pattern.trim();
        if pattern.is_empty() || pattern.contains('\n') {
            return Err(GitError::Unsupported("invalid .gitignore pattern".into()));
        }
        let path = self.workdir()?.join(".gitignore");
        let io = |e: std::io::Error| GitError::Other(format!("cannot update .gitignore: {e}"));
        let mut text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(io(e)),
        };
        if text.lines().any(|l| l.trim() == pattern) {
            return Ok(());
        }
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(pattern);
        text.push('\n');
        std::fs::write(&path, text).map_err(io)
    }
}
```

- [ ] **Step 6: Run the tests to see them pass**

Run: `cargo test -p gitcore --test commit`

Expected: `7 passed`.

- [ ] **Step 7: Lint**

Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings`

Expected: no output from fmt; clippy finishes without warnings.

- [ ] **Step 8: Full suite**

Run: `cargo test --workspace`

Expected: all tests pass.

- [ ] **Step 9: Commit**

```bash
git add crates/gitcore
git commit -m "feat(gitcore): commit via git CLI with libgit2 fallback, amend helpers, gitignore"
```


### Task 3: `win95` checkbox, text area, fixed-width diff font

**Files:**
- Create: `crates/win95/src/checkbox.rs`, `crates/win95/src/text_area.rs`
- Modify: `crates/win95/src/lib.rs`, `crates/win95/src/theme.rs`, `crates/win95/tests/theme.rs`

**Interfaces:**
- Consumes: `bevel`, `bevel_frame`, `theme` (S1).
- Produces: `win95::checkbox(ui, checked: &mut bool, label: &str) -> Response` (toggles on click, `changed()` set, accessible as `WidgetType::Checkbox`; empty label = box only); `win95::text_area(ui, text: &mut String, width: f32, rows: usize) -> Response`; `FontFamily::Monospace` is egui's fixed-width font again.

- [ ] **Step 1: Write the failing font test**

Replace the whole file with the version below (it keeps everything from sub-project 1). Adds `monospace_stays_fixed_width_for_diffs`.

`crates/win95/tests/theme.rs`:

```rust
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;

#[test]
fn installing_the_theme_renders_accented_text() {
    let mut h = Harness::new_ui(|ui| {
        ui.label("Clone a repository — éàç");
    });
    win95::theme::install(&h.ctx);
    h.run();
    assert!(h.query_by_label("Clone a repository — éàç").is_some());
    let style = h.ctx.global_style();
    assert_eq!(style.visuals.panel_fill, win95::theme::SILVER);
}

#[test]
fn hovered_widgets_use_white_text_on_navy() {
    let h = Harness::new_ui(|ui| {
        ui.label("x");
    });
    win95::theme::install(&h.ctx);
    let v = h.ctx.global_style().visuals.clone();
    // A global override would force black text on the navy hover background.
    assert_eq!(v.override_text_color, None);
    assert_eq!(v.widgets.hovered.text_color(), win95::theme::WHITE);
    assert_eq!(v.widgets.hovered.weak_bg_fill, win95::theme::NAVY);
    assert_eq!(v.text_color(), win95::theme::BLACK);
}

#[test]
fn monospace_stays_fixed_width_for_diffs() {
    let mut h = Harness::new_ui(|ui| {
        ui.label("x");
    });
    win95::theme::install(&h.ctx);
    h.run();
    // W95FA is proportional: the diff view needs egui's real monospace font.
    let font = egui::FontId::monospace(win95::theme::FONT_SIZE);
    let (w_i, w_m) = h
        .ctx
        .fonts_mut(|f| (f.glyph_width(&font, 'i'), f.glyph_width(&font, 'M')));
    assert!((w_i - w_m).abs() < 0.01, "i={w_i} M={w_m}");
}
```

- [ ] **Step 2: Write the failing tests in `crates/win95/src/checkbox.rs`**

Create `crates/win95/src/checkbox.rs` containing only this test module for now (the implementation goes above it in a later step).

`crates/win95/src/checkbox.rs`:

```rust
#[cfg(test)]
mod tests {
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    #[test]
    fn clicking_toggles_and_reports_state() {
        let mut h = Harness::new_ui_state(
            |ui, on: &mut bool| {
                super::checkbox(ui, on, "Amend last commit");
            },
            false,
        );
        h.get_by_label("Amend last commit").click();
        h.run();
        assert!(*h.state());
        h.get_by_label("Amend last commit").click();
        h.run();
        assert!(!*h.state());
    }
}
```

- [ ] **Step 3: Write the failing tests in `crates/win95/src/text_area.rs`**

Create `crates/win95/src/text_area.rs` containing only this test module for now (the implementation goes above it in a later step).

`crates/win95/src/text_area.rs`:

```rust
#[cfg(test)]
mod tests {
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    #[test]
    fn typing_multiple_lines() {
        let mut h = Harness::new_ui_state(
            |ui, s: &mut String| {
                super::text_area(ui, s, 300.0, 4);
            },
            String::new(),
        );
        h.get_by_role(egui::accesskit::Role::MultilineTextInput)
            .focus();
        h.run();
        h.get_by_role(egui::accesskit::Role::MultilineTextInput)
            .type_text("Fix bug");
        h.run();
        h.key_press(egui::Key::Enter);
        h.run();
        h.get_by_role(egui::accesskit::Role::MultilineTextInput)
            .type_text("Details");
        h.run();
        assert_eq!(h.state(), "Fix bug\nDetails");
    }
}
```

- [ ] **Step 4: Export the widgets**

Replace the whole file with the version below (it keeps everything from sub-project 1).

`crates/win95/src/lib.rs`:

```rust
//! Windows 95 look-and-feel for egui. No business logic lives here.

pub mod bevel;
pub mod button;
pub mod checkbox;
pub mod dialog;
pub mod icon;
pub mod list_view;
pub mod panel;
pub mod progress;
pub mod status_bar;
pub mod tabs;
pub mod text_area;
pub mod text_field;
pub mod theme;
pub mod title_bar;
pub mod window_frame;

pub use bevel::Bevel;
pub use button::Button95;
pub use checkbox::checkbox;
pub use dialog::{Dialog, DialogResponse};
pub use icon::Icon;
pub use list_view::{Cell, Column, ListResponse, ListView};
pub use panel::bevel_frame;
pub use progress::ProgressBar95;
pub use status_bar::status_bar;
pub use tabs::tabs;
pub use text_area::text_area;
pub use text_field::text_field;
pub use title_bar::{TitleAction, TitleBar};
pub use window_frame::{above_dialogs, resize_edges};
```

- [ ] **Step 5: Run the tests to see them fail**

Run: `cargo test -p win95`

Expected: compile errors: `cannot find function checkbox` / `text_area` (and the font test fails once they compile).

- [ ] **Step 6: Keep W95FA out of the monospace family**

Replace the whole file with the version below (it keeps everything from sub-project 1).

`crates/win95/src/theme.rs`:

```rust
//! Win95 palette, font and egui style.

use std::sync::Arc;

use egui::{
    Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Margin, Shadow, Stroke,
    TextStyle, Vec2,
};

pub const SILVER: Color32 = Color32::from_rgb(0xC0, 0xC0, 0xC0);
pub const LIGHT: Color32 = Color32::from_rgb(0xDF, 0xDF, 0xDF);
pub const WHITE: Color32 = Color32::WHITE;
pub const GRAY: Color32 = Color32::from_rgb(0x80, 0x80, 0x80);
pub const BLACK: Color32 = Color32::BLACK;
pub const NAVY: Color32 = Color32::from_rgb(0x00, 0x00, 0x80);
pub const TITLE_END: Color32 = Color32::from_rgb(0x10, 0x84, 0xD0);
pub const INACTIVE_TITLE: Color32 = Color32::from_rgb(0x80, 0x80, 0x80);
pub const INACTIVE_TITLE_END: Color32 = Color32::from_rgb(0xB5, 0xB5, 0xB5);

/// Base font size in points. W95FA is a pixel font: keep this a whole number.
pub const FONT_SIZE: f32 = 13.0;
pub const FONT_NAME: &str = "W95FA";

static W95FA: &[u8] = include_bytes!("../assets/W95FA.otf");

pub fn font(size: f32) -> FontId {
    FontId::new(size, FontFamily::Proportional)
}

/// Install the Win95 font and style on `ctx`. Call once at startup.
pub fn install(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    fonts
        .font_data
        .insert(FONT_NAME.to_owned(), Arc::new(FontData::from_static(W95FA)));
    // W95FA first for UI text; egui's default fonts stay as fallback for missing glyphs.
    // Monospace keeps egui's fixed-width font: W95FA is proportional and would misalign diffs.
    fonts
        .families
        .entry(FontFamily::Proportional)
        .or_default()
        .insert(0, FONT_NAME.to_owned());
    ctx.set_fonts(fonts);
    ctx.set_theme(egui::Theme::Light);
    ctx.all_styles_mut(apply_style);
}

fn apply_style(style: &mut egui::Style) {
    style.text_styles = [
        (TextStyle::Small, font(FONT_SIZE)),
        (TextStyle::Body, font(FONT_SIZE)),
        (TextStyle::Button, font(FONT_SIZE)),
        (
            TextStyle::Monospace,
            FontId::new(FONT_SIZE, FontFamily::Monospace),
        ),
        (TextStyle::Heading, font(FONT_SIZE + 3.0)),
    ]
    .into();
    style.spacing.item_spacing = Vec2::new(6.0, 4.0);
    style.spacing.button_padding = Vec2::new(6.0, 2.0);
    style.spacing.menu_margin = Margin::same(2);
    style.spacing.window_margin = Margin::same(3);
    style.animation_time = 0.0;

    let v = &mut style.visuals;
    v.dark_mode = false;
    v.panel_fill = SILVER;
    v.window_fill = SILVER;
    v.faint_bg_color = SILVER;
    v.extreme_bg_color = WHITE;
    v.text_edit_bg_color = Some(WHITE);
    v.window_corner_radius = CornerRadius::ZERO;
    v.menu_corner_radius = CornerRadius::ZERO;
    v.window_shadow = Shadow::NONE;
    v.popup_shadow = Shadow::NONE;
    v.window_stroke = Stroke::new(1.0, GRAY);
    v.selection.bg_fill = NAVY;
    v.selection.stroke = Stroke::new(1.0, WHITE);
    v.hyperlink_color = NAVY;
    for w in [
        &mut v.widgets.noninteractive,
        &mut v.widgets.inactive,
        &mut v.widgets.open,
    ] {
        w.bg_fill = SILVER;
        w.weak_bg_fill = SILVER;
        w.bg_stroke = Stroke::NONE;
        w.fg_stroke = Stroke::new(1.0, BLACK);
        w.corner_radius = CornerRadius::ZERO;
        w.expansion = 0.0;
    }
    // Win95 menus and hovered items: navy highlight with white text.
    for w in [&mut v.widgets.hovered, &mut v.widgets.active] {
        w.bg_fill = NAVY;
        w.weak_bg_fill = NAVY;
        w.bg_stroke = Stroke::NONE;
        w.fg_stroke = Stroke::new(1.0, WHITE);
        w.corner_radius = CornerRadius::ZERO;
        w.expansion = 0.0;
    }
    v.widgets.open.weak_bg_fill = NAVY;
    v.widgets.open.fg_stroke = Stroke::new(1.0, WHITE);
}
```

- [ ] **Step 7: Implement `crates/win95/src/checkbox.rs`**

Insert this **above** the existing `#[cfg(test)]` module in `crates/win95/src/checkbox.rs`.

`crates/win95/src/checkbox.rs`:

```rust
use egui::{Align2, Rect, Response, Sense, Stroke, Ui, WidgetInfo, WidgetType, pos2, vec2};

use crate::bevel::{self, Bevel};
use crate::theme::{self, BLACK, GRAY, WHITE};

/// Win95 check box: 13×13 white well with a black tick, label on the right.
/// An empty `label` draws the box alone (used on diff lines).
pub fn checkbox(ui: &mut Ui, checked: &mut bool, label: &str) -> Response {
    let font = theme::font(theme::FONT_SIZE);
    let text_w = if label.is_empty() {
        0.0
    } else {
        ui.painter()
            .layout_no_wrap(label.to_string(), font.clone(), BLACK)
            .size()
            .x
            + 6.0
    };
    let (rect, mut resp) = ui.allocate_exact_size(vec2(13.0 + text_w, 16.0), Sense::click());
    if resp.clicked() {
        *checked = !*checked;
        resp.mark_changed();
    }
    let value = *checked;
    let owned = label.to_string();
    resp.widget_info(|| WidgetInfo::selected(WidgetType::Checkbox, true, value, &owned));
    let bx = Rect::from_min_size(pos2(rect.left(), rect.center().y - 6.5), vec2(13.0, 13.0));
    let p = ui.painter();
    p.rect_filled(bx, 0.0, WHITE);
    bevel::paint(p, bx, Bevel::Field);
    if value {
        let s = Stroke::new(2.0, BLACK);
        let (a, b, c) = (
            bx.left_top() + vec2(3.0, 6.0),
            bx.left_top() + vec2(5.5, 9.0),
            bx.left_top() + vec2(10.0, 3.5),
        );
        p.line_segment([a, b], s);
        p.line_segment([b, c], s);
    }
    if !label.is_empty() {
        p.text(
            pos2(bx.right() + 5.0, rect.center().y),
            Align2::LEFT_CENTER,
            label,
            font,
            BLACK,
        );
    }
    if resp.has_focus() {
        p.rect_stroke(
            rect.expand(1.0),
            0.0,
            Stroke::new(1.0, GRAY),
            egui::StrokeKind::Outside,
        );
    }
    resp
}
```

- [ ] **Step 8: Implement `crates/win95/src/text_area.rs`**

Insert this **above** the existing `#[cfg(test)]` module in `crates/win95/src/text_area.rs`.

`crates/win95/src/text_area.rs`:

```rust
use egui::{Response, TextEdit, Ui};

use crate::bevel::Bevel;
use crate::panel::bevel_frame;
use crate::theme::WHITE;

/// Multi-line white sunken text box, `rows` lines high.
pub fn text_area(ui: &mut Ui, text: &mut String, width: f32, rows: usize) -> Response {
    bevel_frame(ui, Bevel::Field, WHITE, 1, |ui| {
        ui.add(
            TextEdit::multiline(text)
                .frame(egui::Frame::NONE)
                .desired_width(width)
                .desired_rows(rows),
        )
    })
    .inner
}
```

- [ ] **Step 9: Run the tests to see them pass**

Run: `cargo test -p win95`

Expected: unit tests `16 passed`, `tests/theme.rs` `3 passed`.

- [ ] **Step 10: Lint**

Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings`

Expected: no output from fmt; clippy finishes without warnings.

- [ ] **Step 11: Commit**

```bash
git add crates/win95
git commit -m "feat(win95): checkbox, multi-line text area, fixed-width monospace"
```


### Task 4: File watcher and login-shell PATH

**Files:**
- Modify: `Cargo.toml` (workspace dep `notify`), `crates/app/Cargo.toml`, `crates/app/src/lib.rs`
- Create: `crates/app/src/watch.rs`, `crates/app/src/env_path.rs`
- Test: `crates/app/tests/watch.rs`

**Interfaces:**
- Consumes: nothing new.
- Produces: `retrogit::watch::{Watcher, is_relevant, DEBOUNCE}` — `Watcher::start(root: &Path, on_change: impl Fn() + Send + 'static) -> notify::Result<Watcher>` (drop to stop), `Watcher::root(&self) -> &Path`, `is_relevant(root: &Path, path: &Path) -> bool`; `retrogit::env_path::login_shell_path(timeout: Duration) -> Option<String>` (macOS only, `None` elsewhere).

- [ ] **Step 1: Add the dependency to the workspace**

Replace the whole file with the version below (it keeps everything from sub-project 1). Only addition: `notify = "8"`.

`Cargo.toml`:

```toml
[workspace]
resolver = "3"
members = ["crates/*"]

[workspace.package]
version = "0.1.0"
edition = "2024"
rust-version = "1.95"
publish = false

[workspace.dependencies]
win95 = { path = "crates/win95" }
gitcore = { path = "crates/gitcore" }
github = { path = "crates/github" }
eframe = { version = "0.36", default-features = false, features = ["glow", "accesskit", "default_fonts", "links"] }
egui = "0.36"
egui_kittest = "0.36"
git2 = { version = "0.21", default-features = false, features = ["https", "vendored-libgit2", "vendored-openssl"] }
ureq = { version = "3", features = ["json", "platform-verifier"] }
keyring = "4"
rfd = "0.17"
thiserror = "2"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
dirs = "7"
log = "0.4"
notify = "8"
mockito = "1"
tempfile = "3"

[workspace.lints.rust]
unsafe_code = "deny"

[workspace.lints.clippy]
unwrap_used = "warn"
expect_used = "warn"

[profile.release]
opt-level = 3
lto = "thin"
codegen-units = 1
strip = true
panic = "unwind"
```

- [ ] **Step 2: Use it in the app**

Replace the whole file with the version below (it keeps everything from sub-project 1).

`crates/app/Cargo.toml`:

```toml
[package]
name = "retrogit"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
publish.workspace = true
description = "A Git client with a Windows 95 look"

[lib]
name = "retrogit"
path = "src/lib.rs"

[[bin]]
name = "retrogit"
path = "src/main.rs"

[dependencies]
win95.workspace = true
gitcore.workspace = true
github.workspace = true
eframe.workspace = true
egui.workspace = true
rfd.workspace = true
serde.workspace = true
serde_json.workspace = true
dirs.workspace = true
log.workspace = true
notify.workspace = true

[dev-dependencies]
mockito.workspace = true
tempfile.workspace = true
git2.workspace = true

[lints]
workspace = true
```

- [ ] **Step 3: Write the failing watcher integration test**

The test first proves that writes under `.git/objects` do nothing, then that five quick writes give exactly one callback.

`crates/app/tests/watch.rs`:

```rust
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
```

- [ ] **Step 4: Write the failing tests in `crates/app/src/watch.rs`**

Create `crates/app/src/watch.rs` containing only this test module for now (the implementation goes above it in a later step).

`crates/app/src/watch.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::is_relevant;
    use std::path::Path;

    #[test]
    fn only_index_and_head_matter_inside_dot_git() {
        let root = Path::new("/r");
        assert!(is_relevant(root, Path::new("/r/src/main.rs")));
        assert!(is_relevant(root, Path::new("/r/.gitignore")));
        assert!(is_relevant(root, Path::new("/r/.git/index")));
        assert!(is_relevant(root, Path::new("/r/.git/HEAD")));
        assert!(!is_relevant(root, Path::new("/r/.git/objects/ab/cdef")));
        assert!(!is_relevant(root, Path::new("/r/.git/index.lock")));
        assert!(!is_relevant(root, Path::new("/elsewhere/file")));
        assert!(!is_relevant(root, Path::new("/r")));
    }
}
```

- [ ] **Step 5: Write the failing tests in `crates/app/src/env_path.rs`**

Create `crates/app/src/env_path.rs` containing only this test module for now (the implementation goes above it in a later step).

`crates/app/src/env_path.rs`:

```rust
#[cfg(test)]
mod tests {
    #[test]
    #[cfg(target_os = "macos")]
    fn login_shell_path_includes_system_dirs() {
        let p = super::login_shell_path(std::time::Duration::from_secs(20)).unwrap_or_default();
        assert!(p.split(':').any(|d| d == "/usr/bin"), "{p}");
    }
}
```

- [ ] **Step 6: Declare the modules**

Replace the whole file with the version below (it keeps everything from sub-project 1).

`crates/app/src/lib.rs`:

```rust
//! RetroGit application: state, background worker and screens.

pub mod app;
pub mod config;
pub mod env_path;
pub mod format;
pub mod logging;
pub mod protocol;
pub mod state;
pub mod strings;
pub mod ui;
pub mod watch;
pub mod worker;

/// OAuth App client ID (public, no secret). Paste yours here, or build with
/// `RETROGIT_GITHUB_CLIENT_ID=Ov23li... cargo build`.
pub const GITHUB_CLIENT_ID: &str = match option_env!("RETROGIT_GITHUB_CLIENT_ID") {
    Some(id) => id,
    None => "Ov23liy76k7uO4WBEEci",
};
```

- [ ] **Step 7: Run the tests to see them fail**

Run: `cargo test -p retrogit --lib --test watch`

Expected: compile errors: `Watcher`, `is_relevant`, `login_shell_path` not found.

- [ ] **Step 8: Implement the watcher**

Insert this **above** the existing `#[cfg(test)]` module in `crates/app/src/watch.rs`.

`crates/app/src/watch.rs`:

```rust
//! Working-tree watcher: tells the UI when the status may have changed.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{RecvTimeoutError, channel};
use std::time::{Duration, Instant};

use notify::{RecursiveMode, Watcher as _};

/// Changes arriving within this window are reported once.
pub const DEBOUNCE: Duration = Duration::from_millis(300);

/// Stops watching when dropped.
pub struct Watcher {
    _inner: notify::RecommendedWatcher,
    root: PathBuf,
}

/// Whether a change at `path` can affect `git status` for the repo at `root`.
/// Inside `.git/`, only the index and HEAD matter (objects, logs, locks are noise).
pub fn is_relevant(root: &Path, path: &Path) -> bool {
    let Ok(rel) = path.strip_prefix(root) else {
        return false;
    };
    let mut parts = rel.components();
    match parts.next() {
        Some(first) if first.as_os_str() == ".git" => {
            let rest: PathBuf = parts.collect();
            rest == Path::new("index") || rest == Path::new("HEAD")
        }
        Some(_) => true,
        None => false,
    }
}

impl Watcher {
    /// Watch `root` recursively; `on_change` runs on a background thread, at most once per
    /// `DEBOUNCE` burst.
    pub fn start(root: &Path, on_change: impl Fn() + Send + 'static) -> notify::Result<Watcher> {
        // FSEvents reports canonical paths (e.g. /private/var/... for /var/...).
        let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        let (tx, rx) = channel::<()>();
        let filter_root = root.clone();
        let mut inner = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            let Ok(event) = res else { return };
            if matches!(event.kind, notify::EventKind::Access(_)) {
                return;
            }
            if event.paths.iter().any(|p| is_relevant(&filter_root, p)) {
                let _ = tx.send(());
            }
        })?;
        inner.watch(&root, RecursiveMode::Recursive)?;
        std::thread::Builder::new()
            .name("retrogit-watch".into())
            .spawn(move || {
                // Ends when the watcher (and so `tx`) is dropped.
                while rx.recv().is_ok() {
                    let deadline = Instant::now() + DEBOUNCE;
                    loop {
                        match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                            Ok(()) => continue,
                            Err(RecvTimeoutError::Timeout) => break,
                            Err(RecvTimeoutError::Disconnected) => return,
                        }
                    }
                    on_change();
                }
            })
            .map_err(|e| notify::Error::generic(&e.to_string()))?;
        Ok(Watcher {
            _inner: inner,
            root,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}
```

The root is canonicalized because FSEvents reports `/private/var/...` for `/var/...`. The debounce thread exits when the watcher (and therefore the channel sender) is dropped.

- [ ] **Step 9: Implement the login-shell PATH lookup**

Insert this **above** the existing `#[cfg(test)]` module in `crates/app/src/env_path.rs`.

`crates/app/src/env_path.rs`:

```rust
//! PATH for `git` and its hooks.

use std::sync::mpsc::channel;
use std::time::Duration;

/// On macOS, apps started from the Finder/Dock get a minimal PATH (no Homebrew, no node),
/// which breaks `git` hooks such as husky or pre-commit. Ask the login shell for the user's
/// real PATH (shell startup can be slow: nvm, conda...). Elsewhere, the inherited PATH is
/// already right: `None`. Blocking: call it from a background thread.
pub fn login_shell_path(timeout: Duration) -> Option<String> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let out = std::process::Command::new(shell)
            .args(["-l", "-c", "printf '%s' \"$PATH\""])
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output();
        let _ = tx.send(out);
    });
    let out = rx.recv_timeout(timeout).ok()?.ok()?;
    let path = String::from_utf8(out.stdout).ok()?.trim().to_string();
    (out.status.success() && path.contains('/')).then_some(path)
}
```

- [ ] **Step 10: Run the tests to see them pass**

Run: `cargo test -p retrogit --lib --test watch`

Expected: lib tests pass (including `only_index_and_head_matter_inside_dot_git` and, on macOS, `login_shell_path_includes_system_dirs`); `tests/watch.rs` `1 passed`.

- [ ] **Step 11: Lint**

Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings`

Expected: no output from fmt; clippy finishes without warnings.

- [ ] **Step 12: Full suite**

Run: `cargo test --workspace`

Expected: all tests pass.

- [ ] **Step 13: Commit**

```bash
git add Cargo.toml Cargo.lock crates/app
git commit -m "feat(app): debounced working-tree watcher and login-shell PATH"
```


### Task 5: Protocol, `ChangesView` state and worker commands

**Files:**
- Modify: `crates/app/src/protocol.rs`, `crates/app/src/state.rs`, `crates/app/src/worker.rs`, `crates/app/src/main.rs`
- Create: `crates/app/src/worker/changes.rs`
- Test: `crates/app/tests/state.rs`, `crates/app/tests/worker.rs`

**Interfaces:**
- Consumes: everything from Tasks 1–4; `strings::*` (Task 1).
- Produces:
  - `Command` new variants: `RefreshStatus`, `LoadDiff { path: String, side: Side }`, `Stage { path: String, selection: Selection, shown: Option<FileDiff> }`, `Unstage { .. same .. }`, `Commit { message: String, amend: bool }`, `AddToGitignore(String)`, `LoadAmendInfo`.
  - `Op` new variants: `Changes`, `Commit`. `Event` new variants: `StatusLoaded(Vec<FileStatus>)`, `DiffLoaded(FileDiff)`, `Committed(CommitOutcome)`, `AmendInfo { message: Option<String>, pushed: bool }`.
  - `state::ChangesView { files, shown: Option<(String, Side)>, diff: Option<FileDiff>, selected_lines: BTreeSet<(usize, usize)>, show_large, summary, description, amend, head_pushed, committing, warned_no_cli: bool, last_commit_note: Option<String>, focus_summary: bool }` with `staged()`, `unstaged()`, `commit_message()`, `can_commit()`; `AppState.changes: ChangesView`; `state::LARGE_DIFF_LINES = 20_000`.
  - `WorkerDeps.commit_backend: CommitBackend`; `WorkerHandle::refresher(&self) -> impl Fn() + Send + 'static`; `send(RefreshStatus)` is dropped while one is already queued.
  - The worker remembers the last opened/cloned repo; every change command replies with `StatusLoaded` (+ `DiffLoaded` for the displayed file).

- [ ] **Step 1: Write the failing reducer tests**

Replace the whole file with the version below (it keeps everything from sub-project 1). Adds the `changes` test module.

`crates/app/tests/state.rs`:

```rust
#![allow(clippy::unwrap_used)]

use std::path::PathBuf;

use gitcore::{CloneProgress, Head, RepoSummary};
use github::{RepoInfo, User};
use retrogit::config::Config;
use retrogit::protocol::{AppError, Event, Op, Severity};
use retrogit::state::{AppState, Auth, CloneDialog, SignInDialog, filter_repos};

fn user() -> User {
    User {
        login: "ada".into(),
        name: None,
    }
}

fn summary(path: &str) -> RepoSummary {
    RepoSummary {
        name: "demo".into(),
        path: PathBuf::from(path),
        head: Head::Branch("main".into()),
        origin_url: None,
        last_commit: None,
    }
}

fn repo(full: &str) -> RepoInfo {
    let (owner, name) = full.split_once('/').unwrap();
    RepoInfo {
        full_name: full.into(),
        name: name.into(),
        owner: owner.into(),
        private: false,
        clone_url: format!("https://github.com/{full}.git"),
        updated_at: "2026-09-30T10:00:00Z".into(),
    }
}

fn err() -> AppError {
    AppError::new(Severity::Error, "boom")
}

#[test]
fn starts_checking_and_flags_missing_recents() {
    let mut c = Config::default();
    c.add_recent(
        "gone",
        std::path::Path::new("/definitely/not/here/retrogit"),
    );
    let s = AppState::new(c);
    assert_eq!(s.auth, Auth::Checking);
    assert!(
        s.missing
            .contains(&PathBuf::from("/definitely/not/here/retrogit"))
    );
}

#[test]
fn signed_out_opens_sign_in_and_clears_repos() {
    let mut s = AppState::new(Config::default());
    s.repos = vec![repo("a/b")];
    s.apply(Event::SignedOut);
    assert_eq!(s.auth, Auth::SignedOut);
    assert!(s.repos.is_empty());
    assert_eq!(s.sign_in, Some(SignInDialog::default()));
}

#[test]
fn device_code_then_signed_in_closes_dialog() {
    let mut s = AppState::new(Config::default());
    s.sign_in = Some(SignInDialog::default());
    s.apply(Event::DeviceCode {
        user_code: "ABCD-1234".into(),
        verification_uri: "https://github.com/login/device".into(),
    });
    assert!(matches!(s.auth, Auth::Waiting { ref user_code, .. } if user_code == "ABCD-1234"));
    s.apply(Event::SignedIn(user()));
    assert_eq!(s.user().unwrap().login, "ada");
    assert_eq!(s.sign_in, None);
}

#[test]
fn auth_error_while_waiting_resets_to_signed_out_and_queues_message() {
    let mut s = AppState::new(Config::default());
    s.sign_in = Some(SignInDialog {
        tab: 1,
        pat: "x".into(),
        pat_submitted: true,
    });
    s.auth = Auth::Waiting {
        user_code: "A".into(),
        verification_uri: "u".into(),
    };
    s.apply(Event::Error {
        during: Op::Auth,
        error: err(),
    });
    assert_eq!(s.auth, Auth::SignedOut);
    assert!(!s.sign_in.as_ref().unwrap().pat_submitted);
    assert_eq!(s.messages.len(), 1);
}

#[test]
fn clone_progress_done_and_recent_list() {
    let mut s = AppState::new(Config::default());
    s.clone = Some(CloneDialog::default());
    let p = CloneProgress {
        received_objects: 1,
        total_objects: 2,
        ..Default::default()
    };
    s.apply(Event::CloneProgress(p));
    assert_eq!(s.clone.as_ref().unwrap().progress, Some(p));
    s.apply(Event::CloneDone(summary("/tmp/demo")));
    assert!(s.clone.is_none());
    assert_eq!(s.config.recent[0].path, PathBuf::from("/tmp/demo"));
    assert!(s.config_dirty);
    assert_eq!(s.current.as_ref().unwrap().name, "demo");
}

#[test]
fn clone_cancel_or_error_keeps_dialog_but_stops_progress() {
    let mut s = AppState::new(Config::default());
    s.clone = Some(CloneDialog {
        progress: Some(CloneProgress::default()),
        ..Default::default()
    });
    s.apply(Event::CloneCancelled);
    assert_eq!(s.clone.as_ref().unwrap().progress, None);
    s.clone.as_mut().unwrap().progress = Some(CloneProgress::default());
    s.apply(Event::Error {
        during: Op::Clone,
        error: err(),
    });
    assert_eq!(s.clone.as_ref().unwrap().progress, None);
}

#[test]
fn open_error_on_vanished_recent_marks_it_missing() {
    let mut c = Config::default();
    let gone = PathBuf::from("/definitely/not/here/retrogit2");
    c.add_recent("gone", &gone);
    let mut s = AppState::new(c);
    s.missing.clear();
    s.apply(Event::Error {
        during: Op::Open(gone.clone()),
        error: err(),
    });
    assert!(s.missing.contains(&gone));
}

#[test]
fn reopening_clears_missing_and_remove_recent_clears_current() {
    let mut s = AppState::new(Config::default());
    s.missing.insert(PathBuf::from("/tmp/demo"));
    s.apply(Event::RepoOpened(summary("/tmp/demo")));
    assert!(s.missing.is_empty());
    s.remove_recent(std::path::Path::new("/tmp/demo"));
    assert!(s.config.recent.is_empty());
    assert!(s.current.is_none());
}

#[test]
fn filter_is_case_insensitive_on_owner_and_name() {
    let repos = vec![
        repo("ExampleOrg/acme-cor-lab"),
        repo("ada/retrogit"),
        repo("ada/Notes"),
    ];
    assert_eq!(filter_repos(&repos, ""), vec![0, 1, 2]);
    assert_eq!(filter_repos(&repos, "  ADA/ "), vec![1, 2]);
    assert_eq!(filter_repos(&repos, "exampleorg"), vec![0]);
    assert_eq!(filter_repos(&repos, "zzz"), Vec::<usize>::new());
}

#[test]
fn offline_does_not_open_sign_in() {
    let mut s = AppState::new(Config::default());
    s.apply(Event::Error {
        during: Op::Auth,
        error: err(),
    });
    s.apply(Event::Offline);
    assert_eq!(s.auth, Auth::Offline);
    assert_eq!(s.sign_in, None);
    assert_eq!(s.messages.len(), 1);
}

#[test]
fn cancelled_clone_shows_an_info_message() {
    let mut s = AppState::new(Config::default());
    s.clone = Some(CloneDialog {
        progress: Some(CloneProgress::default()),
        ..Default::default()
    });
    s.apply(Event::CloneCancelled);
    let m = s.messages.front().unwrap();
    assert_eq!(m.severity, Severity::Info);
    assert_eq!(m.message, retrogit::strings::INFO_CLONE_CANCELLED);
}

mod changes {
    use super::*;
    use gitcore::{Change, CommitInfo, CommitOutcome, FileDiff, FileStatus, Side};

    fn file(path: &str, staged: Option<Change>, unstaged: Option<Change>) -> FileStatus {
        FileStatus {
            path: path.into(),
            staged,
            unstaged,
        }
    }

    fn diff(path: &str, side: Side) -> FileDiff {
        FileDiff {
            path: path.into(),
            side,
            binary: false,
            hunks: vec![],
        }
    }

    fn outcome(used_cli: bool) -> CommitOutcome {
        CommitOutcome {
            commit: CommitInfo {
                short_id: "a1b2c3d".into(),
                summary: "s".into(),
                author: "Ada".into(),
                time: 0,
            },
            used_cli,
        }
    }

    #[test]
    fn new_diff_clears_the_line_selection_and_ignores_stale_diffs() {
        let mut s = AppState::new(Config::default());
        s.changes.shown = Some(("a.txt".into(), Side::Unstaged));
        s.changes.selected_lines.insert((0, 1));
        s.apply(Event::DiffLoaded(diff("b.txt", Side::Unstaged)));
        assert!(
            s.changes.diff.is_none(),
            "diff of another file must be ignored"
        );
        assert_eq!(s.changes.selected_lines.len(), 1);
        s.apply(Event::DiffLoaded(diff("a.txt", Side::Unstaged)));
        assert!(s.changes.diff.is_some());
        assert!(s.changes.selected_lines.is_empty());
    }

    #[test]
    fn status_without_the_shown_file_closes_its_diff() {
        let mut s = AppState::new(Config::default());
        s.changes.shown = Some(("a.txt".into(), Side::Unstaged));
        s.changes.diff = Some(diff("a.txt", Side::Unstaged));
        s.apply(Event::StatusLoaded(vec![file(
            "a.txt",
            Some(Change::Modified),
            None,
        )]));
        assert_eq!(s.changes.shown, None);
        assert_eq!(s.changes.diff, None);
    }

    #[test]
    fn commit_button_rules() {
        let mut s = AppState::new(Config::default());
        s.changes.summary = "Fix".into();
        assert!(!s.changes.can_commit(), "nothing staged");
        s.apply(Event::StatusLoaded(vec![file(
            "a.txt",
            Some(Change::Modified),
            None,
        )]));
        assert!(s.changes.can_commit());
        s.changes.summary = "  ".into();
        assert!(!s.changes.can_commit(), "empty summary");
        s.apply(Event::StatusLoaded(vec![]));
        s.changes.summary = "Reword".into();
        s.changes.amend = true;
        assert!(s.changes.can_commit(), "amend needs no staged change");
        s.changes.committing = true;
        assert!(!s.changes.can_commit());
    }

    #[test]
    fn commit_message_joins_summary_and_description() {
        let mut c = retrogit::state::ChangesView {
            summary: " Fix bug ".into(),
            ..Default::default()
        };
        assert_eq!(c.commit_message(), "Fix bug");
        c.description = "\nWhy it broke\n".into();
        assert_eq!(c.commit_message(), "Fix bug\n\nWhy it broke");
    }

    #[test]
    fn committed_resets_the_form_and_warns_once_without_git() {
        let mut s = AppState::new(Config::default());
        s.changes.summary = "x".into();
        s.changes.description = "y".into();
        s.changes.amend = true;
        s.changes.committing = true;
        s.apply(Event::Committed(outcome(false)));
        let c = &s.changes;
        assert!(c.summary.is_empty() && c.description.is_empty() && !c.amend && !c.committing);
        assert_eq!(c.last_commit_note.as_deref(), Some("Committed a1b2c3d"));
        assert_eq!(s.messages.len(), 1);
        s.apply(Event::Committed(outcome(false)));
        assert_eq!(s.messages.len(), 1, "warning only once");
    }

    #[test]
    fn commit_error_keeps_the_message() {
        let mut s = AppState::new(Config::default());
        s.changes.summary = "keep me".into();
        s.changes.committing = true;
        s.apply(Event::Error {
            during: Op::Commit,
            error: err(),
        });
        assert!(!s.changes.committing);
        assert_eq!(s.changes.summary, "keep me");
    }

    #[test]
    fn amend_info_prefills_only_empty_fields() {
        let mut s = AppState::new(Config::default());
        s.changes.amend = true;
        s.apply(Event::AmendInfo {
            message: Some("Title\n\nBody text".into()),
            pushed: true,
        });
        assert_eq!(
            (s.changes.summary.as_str(), s.changes.description.as_str()),
            ("Title", "Body text")
        );
        assert!(s.changes.head_pushed);
        s.changes.summary = "Mine".into();
        s.apply(Event::AmendInfo {
            message: Some("Other".into()),
            pushed: false,
        });
        assert_eq!(s.changes.summary, "Mine");
    }

    #[test]
    fn opening_another_repo_resets_the_changes_view() {
        let mut s = AppState::new(Config::default());
        s.apply(Event::RepoOpened(summary("/tmp/a")));
        s.changes.summary = "draft".into();
        s.apply(Event::RepoOpened(summary("/tmp/a")));
        assert_eq!(s.changes.summary, "draft", "same repo keeps the draft");
        s.apply(Event::RepoOpened(summary("/tmp/b")));
        assert!(s.changes.summary.is_empty());
    }
}
```

- [ ] **Step 2: Write the failing worker tests**

Replace the whole file with the version below (it keeps everything from sub-project 1). `start` now passes `commit_backend: CommitBackend::Git2`; new tests at the end. `repo_for_changes` resets the index to HEAD because `make_source_repo` commits without writing the index file.

`crates/app/tests/worker.rs`:

```rust
#![allow(clippy::unwrap_used)]

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use github::{Client, MemoryStore, TokenStore};
use mockito::Matcher;
use retrogit::protocol::{Command, Event, Op};
use retrogit::worker::{WorkerDeps, WorkerHandle, spawn};

fn start(server: &mockito::Server, store: Arc<MemoryStore>, client_id: &str) -> WorkerHandle {
    let deps = WorkerDeps {
        client: Client::with_bases(&server.url(), &server.url()),
        store,
        client_id: client_id.into(),
        commit_backend: gitcore::CommitBackend::Git2,
    };
    spawn(deps, || {})
}

/// Collect events until `done` matches one (or panic after 10 s).
fn until(w: &WorkerHandle, done: impl Fn(&Event) -> bool) -> Vec<Event> {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut seen = Vec::new();
    while Instant::now() < deadline {
        if let Ok(ev) = w.events.recv_timeout(Duration::from_millis(100)) {
            let stop = done(&ev);
            seen.push(ev);
            if stop {
                return seen;
            }
        }
    }
    panic!("timed out; events so far: {seen:?}");
}

fn mock_user(server: &mut mockito::Server, token: &str) -> mockito::Mock {
    server
        .mock("GET", "/user")
        .match_header("authorization", format!("Bearer {token}").as_str())
        .with_body(r#"{"login":"ada","name":null}"#)
        .create()
}

#[test]
fn validate_without_token_signs_out() {
    let server = mockito::Server::new();
    let w = start(&server, Arc::new(MemoryStore::default()), "");
    w.send(Command::ValidateToken);
    until(&w, |e| matches!(e, Event::SignedOut));
}

#[test]
fn validate_with_good_token_signs_in() {
    let mut server = mockito::Server::new();
    let _m = mock_user(&mut server, "gho_good");
    let w = start(&server, Arc::new(MemoryStore::with_token("gho_good")), "");
    w.send(Command::ValidateToken);
    until(&w, |e| matches!(e, Event::SignedIn(u) if u.login == "ada"));
}

#[test]
fn validate_with_revoked_token_clears_it() {
    let mut server = mockito::Server::new();
    server.mock("GET", "/user").with_status(401).create();
    let store = Arc::new(MemoryStore::with_token("gho_old"));
    let w = start(&server, store.clone(), "");
    w.send(Command::ValidateToken);
    until(&w, |e| matches!(e, Event::SignedOut));
    assert_eq!(store.load(), Ok(None));
}

#[test]
fn pat_is_validated_then_stored() {
    let mut server = mockito::Server::new();
    let _m = mock_user(&mut server, "ghp_pat");
    let store = Arc::new(MemoryStore::default());
    let w = start(&server, store.clone(), "");
    w.send(Command::SavePat("  ghp_pat \n".into()));
    until(&w, |e| matches!(e, Event::SignedIn(_)));
    assert_eq!(store.load(), Ok(Some("ghp_pat".into())));
}

#[test]
fn rejected_pat_is_not_stored() {
    let mut server = mockito::Server::new();
    server.mock("GET", "/user").with_status(401).create();
    let store = Arc::new(MemoryStore::default());
    let w = start(&server, store.clone(), "");
    w.send(Command::SavePat("ghp_bad".into()));
    until(&w, |e| {
        matches!(
            e,
            Event::Error {
                during: Op::Auth,
                ..
            }
        )
    });
    assert_eq!(store.load(), Ok(None));
}

#[test]
fn device_flow_without_client_id_explains_pat_fallback() {
    let server = mockito::Server::new();
    let w = start(&server, Arc::new(MemoryStore::default()), "");
    w.send(Command::StartDeviceFlow);
    until(
        &w,
        |e| matches!(e, Event::Error { during: Op::Auth, error } if error.message.contains("Advanced")),
    );
}

fn mock_device_code(server: &mut mockito::Server) -> mockito::Mock {
    server
        .mock("POST", "/login/device/code")
        .with_body(r#"{"device_code":"dc1","user_code":"ABCD-1234","verification_uri":"https://github.com/login/device","expires_in":900,"interval":1}"#)
        .create()
}

#[test]
fn device_flow_happy_path_stores_token() {
    let mut server = mockito::Server::new();
    let _c = mock_device_code(&mut server);
    let _t = server
        .mock("POST", "/login/oauth/access_token")
        .match_body(Matcher::UrlEncoded("device_code".into(), "dc1".into()))
        .with_body(r#"{"access_token":"gho_new","token_type":"bearer","scope":"repo,read:org"}"#)
        .create();
    let _u = mock_user(&mut server, "gho_new");
    let store = Arc::new(MemoryStore::default());
    let w = start(&server, store.clone(), "Iv1.test");
    w.send(Command::StartDeviceFlow);
    let evs = until(&w, |e| matches!(e, Event::SignedIn(_)));
    assert!(matches!(&evs[0], Event::DeviceCode { user_code, .. } if user_code == "ABCD-1234"));
    assert_eq!(store.load(), Ok(Some("gho_new".into())));
}

#[test]
fn device_flow_can_be_cancelled_while_waiting() {
    let mut server = mockito::Server::new();
    let _c = mock_device_code(&mut server);
    let _t = server
        .mock("POST", "/login/oauth/access_token")
        .with_body(r#"{"error":"authorization_pending"}"#)
        .create();
    let w = start(&server, Arc::new(MemoryStore::default()), "Iv1.test");
    w.send(Command::StartDeviceFlow);
    until(&w, |e| matches!(e, Event::DeviceCode { .. }));
    w.cancel_device_flow();
    until(&w, |e| matches!(e, Event::DeviceFlowCancelled));
}

#[test]
fn network_failure_while_polling_reports_auth_error() {
    let mut server = mockito::Server::new();
    let _c = mock_device_code(&mut server);
    let _t = server
        .mock("POST", "/login/oauth/access_token")
        .with_status(502)
        .create();
    let w = start(&server, Arc::new(MemoryStore::default()), "Iv1.test");
    w.send(Command::StartDeviceFlow);
    until(&w, |e| {
        matches!(
            e,
            Event::Error {
                during: Op::Auth,
                ..
            }
        )
    });
}

#[test]
fn shutdown_interrupts_a_running_device_flow() {
    let mut server = mockito::Server::new();
    let _c = mock_device_code(&mut server);
    let _t = server
        .mock("POST", "/login/oauth/access_token")
        .with_body(r#"{"error":"authorization_pending"}"#)
        .create();
    let w = start(&server, Arc::new(MemoryStore::default()), "Iv1.test");
    w.send(Command::StartDeviceFlow);
    until(&w, |e| matches!(e, Event::DeviceCode { .. }));
    assert!(w.shutdown(Duration::from_secs(2)));
}

#[test]
fn token_revoked_while_running_signs_out_on_next_call() {
    let mut server = mockito::Server::new();
    let _u = mock_user(&mut server, "ghp_pat");
    let _r = server
        .mock("GET", "/user/repos")
        .match_query(Matcher::Any)
        .with_status(401)
        .create();
    let store = Arc::new(MemoryStore::default());
    let w = start(&server, store.clone(), "");
    w.send(Command::SavePat("ghp_pat".into()));
    until(&w, |e| matches!(e, Event::SignedIn(_)));
    w.send(Command::ListRepos);
    let evs = until(&w, |e| matches!(e, Event::SignedOut));
    assert!(evs.iter().any(|e| matches!(
        e,
        Event::Error {
            during: Op::Repos,
            ..
        }
    )));
    assert_eq!(store.load(), Ok(None));
}

#[test]
fn list_repos_requires_sign_in() {
    let server = mockito::Server::new();
    let w = start(&server, Arc::new(MemoryStore::default()), "");
    w.send(Command::ListRepos);
    until(&w, |e| matches!(e, Event::SignedOut));
}

fn make_source_repo(dir: &Path) {
    let mut opts = git2::RepositoryInitOptions::new();
    opts.initial_head("main");
    let repo = git2::Repository::init_opts(dir, &opts).unwrap();
    std::fs::write(dir.join("README.md"), "hello\n").unwrap();
    let mut idx = repo.index().unwrap();
    idx.add_path(Path::new("README.md")).unwrap();
    let tree = repo.find_tree(idx.write_tree().unwrap()).unwrap();
    let sig = git2::Signature::now("Ada", "ada@example.com").unwrap();
    repo.commit(Some("HEAD"), &sig, &sig, "init", &tree, &[])
        .unwrap();
}

fn file_url(p: &Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    if s.starts_with('/') {
        format!("file://{s}")
    } else {
        format!("file:///{s}")
    }
}

#[test]
fn clone_then_open_report_summaries() {
    let server = mockito::Server::new();
    let src = tempfile::tempdir().unwrap();
    make_source_repo(src.path());
    let out = tempfile::tempdir().unwrap();
    let dest = out.path().join("demo");
    let w = start(&server, Arc::new(MemoryStore::default()), "");
    w.send(Command::Clone {
        url: file_url(src.path()),
        dest: dest.clone(),
    });
    let evs = until(&w, |e| matches!(e, Event::CloneDone(_)));
    assert!(evs.iter().any(|e| matches!(e, Event::CloneProgress(_))));
    w.send(Command::OpenRepo(dest.clone()));
    until(
        &w,
        |e| matches!(e, Event::RepoOpened(s) if s.name == "demo"),
    );
    w.send(Command::OpenRepo(out.path().to_path_buf()));
    until(&w, |e| {
        matches!(
            e,
            Event::Error {
                during: Op::Open(_),
                ..
            }
        )
    });
}

fn signed_in_with_pat(
    server: &mut mockito::Server,
    store: Arc<MemoryStore>,
    client_id: &str,
) -> (WorkerHandle, mockito::Mock) {
    let user = mock_user(server, "ghp_pat");
    let w = start(server, store, client_id);
    w.send(Command::SavePat("ghp_pat".into()));
    until(&w, |e| matches!(e, Event::SignedIn(_)));
    (w, user)
}

#[test]
fn repos_hidden_by_sso_are_listed_with_a_warning_and_link() {
    let mut server = mockito::Server::new();
    let _r = server
        .mock("GET", "/user/repos")
        .match_query(Matcher::Any)
        .with_header("X-GitHub-SSO", "partial-results; organizations=42")
        .with_body("[]")
        .create();
    let (w, _u) = signed_in_with_pat(&mut server, Arc::new(MemoryStore::default()), "Ov23test");
    w.send(Command::ListRepos);
    let evs = until(&w, |e| {
        matches!(
            e,
            Event::Error {
                during: Op::Repos,
                ..
            }
        )
    });
    assert!(evs.iter().any(|e| matches!(e, Event::ReposLoaded(_))));
    let Some(Event::Error { error, .. }) = evs.last() else {
        unreachable!()
    };
    assert_eq!(
        error.link.as_deref(),
        Some("https://github.com/settings/connections/applications/Ov23test")
    );
}

#[test]
fn offline_start_keeps_the_token_for_later_calls() {
    let mut server = mockito::Server::new();
    let _u = server.mock("GET", "/user").with_status(503).create();
    let _r = server
        .mock("GET", "/user/repos")
        .match_query(Matcher::Any)
        .match_header("authorization", "Bearer gho_kept")
        .with_body("[]")
        .create();
    let store = Arc::new(MemoryStore::with_token("gho_kept"));
    let w = start(&server, store.clone(), "");
    w.send(Command::ValidateToken);
    until(&w, |e| matches!(e, Event::Offline));
    assert_eq!(store.load(), Ok(Some("gho_kept".into())));
    w.send(Command::ListRepos);
    until(&w, |e| matches!(e, Event::ReposLoaded(_)));
}

fn git_401(server: &mut mockito::Server) -> mockito::Mock {
    server
        .mock("GET", Matcher::Regex("^/org/demo.git/".into()))
        .with_status(401)
        .with_header("WWW-Authenticate", "Basic realm=\"GitHub\"")
        .create()
}

#[test]
fn clone_auth_failure_with_revoked_token_signs_out() {
    let mut server = mockito::Server::new();
    let store = Arc::new(MemoryStore::default());
    let (w, user_ok) = signed_in_with_pat(&mut server, store.clone(), "");
    user_ok.remove();
    let _u = server.mock("GET", "/user").with_status(401).create();
    let _g = git_401(&mut server);
    let out = tempfile::tempdir().unwrap();
    w.send(Command::Clone {
        url: format!("{}/org/demo.git", server.url()),
        dest: out.path().join("demo"),
    });
    let evs = until(&w, |e| matches!(e, Event::SignedOut));
    assert!(evs.iter().any(|e| matches!(e, Event::Error { during: Op::Clone, error } if error.message == retrogit::strings::ERR_UNAUTHORIZED)));
    assert_eq!(store.load(), Ok(None));
}

#[test]
fn clone_auth_failure_with_valid_token_points_to_sso() {
    let mut server = mockito::Server::new();
    let store = Arc::new(MemoryStore::default());
    let (w, _user_ok) = signed_in_with_pat(&mut server, store.clone(), "");
    let _g = git_401(&mut server);
    let out = tempfile::tempdir().unwrap();
    w.send(Command::Clone {
        url: format!("{}/org/demo.git", server.url()),
        dest: out.path().join("demo"),
    });
    let evs = until(&w, |e| {
        matches!(
            e,
            Event::Error {
                during: Op::Clone,
                ..
            }
        )
    });
    let Some(Event::Error { error, .. }) = evs.last() else {
        unreachable!()
    };
    assert_eq!(error.message, retrogit::strings::ERR_GIT_AUTH);
    assert_eq!(
        error.link.as_deref(),
        Some("https://github.com/settings/tokens")
    );
    assert_eq!(store.load(), Ok(Some("ghp_pat".into())));
}

fn repo_for_changes() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    make_source_repo(d.path());
    let repo = git2::Repository::open(d.path()).unwrap();
    // make_source_repo commits without writing the index file: sync it with HEAD.
    let head = repo.head().unwrap().peel_to_commit().unwrap();
    repo.reset(head.as_object(), git2::ResetType::Mixed, None)
        .unwrap();
    let mut cfg = repo.config().unwrap();
    cfg.set_str("user.name", "Ada").unwrap();
    cfg.set_str("user.email", "ada@example.com").unwrap();
    d
}

#[test]
fn open_stage_commit_flow() {
    let server = mockito::Server::new();
    let d = repo_for_changes();
    std::fs::write(d.path().join("README.md"), "hello\nworld\n").unwrap();
    let w = start(&server, Arc::new(MemoryStore::default()), "");
    w.send(Command::OpenRepo(d.path().to_path_buf()));
    let evs = until(&w, |e| matches!(e, Event::StatusLoaded(_)));
    assert!(evs.iter().any(|e| matches!(e, Event::RepoOpened(_))));
    let Some(Event::StatusLoaded(files)) = evs.last() else {
        unreachable!()
    };
    assert_eq!(files.len(), 1);
    assert!(
        files[0].unstaged.is_some() && files[0].staged.is_none(),
        "{files:?}"
    );

    w.send(Command::LoadDiff {
        path: "README.md".into(),
        side: gitcore::Side::Unstaged,
    });
    let evs = until(&w, |e| matches!(e, Event::DiffLoaded(_)));
    let Some(Event::DiffLoaded(diff)) = evs.last() else {
        unreachable!()
    };
    assert_eq!(diff.hunks.len(), 1);

    w.send(Command::Stage {
        path: "README.md".into(),
        selection: gitcore::Selection::All,
        shown: None,
    });
    let evs = until(&w, |e| matches!(e, Event::DiffLoaded(_)));
    let status = evs.iter().find_map(|e| match e {
        Event::StatusLoaded(f) => Some(f.clone()),
        _ => None,
    });
    assert!(status.unwrap()[0].staged.is_some());

    w.send(Command::Commit {
        message: "Say world".into(),
        amend: false,
    });
    let evs = until(&w, |e| matches!(e, Event::StatusLoaded(_)));
    assert!(
        evs.iter()
            .any(|e| matches!(e, Event::Committed(o) if o.commit.summary == "Say world"))
    );
    assert!(evs.iter().any(|e| matches!(e, Event::RepoOpened(s) if s.last_commit.as_ref().unwrap().summary == "Say world")));
    let Some(Event::StatusLoaded(files)) = evs.last() else {
        unreachable!()
    };
    assert!(files.is_empty());

    w.send(Command::LoadAmendInfo);
    until(
        &w,
        |e| matches!(e, Event::AmendInfo { message: Some(m), pushed: false } if m == "Say world"),
    );
}

#[test]
fn stale_selection_reports_and_resyncs() {
    let server = mockito::Server::new();
    let d = repo_for_changes();
    std::fs::write(d.path().join("README.md"), "hello\nnew\n").unwrap();
    let w = start(&server, Arc::new(MemoryStore::default()), "");
    w.send(Command::OpenRepo(d.path().to_path_buf()));
    until(&w, |e| matches!(e, Event::StatusLoaded(_)));
    w.send(Command::LoadDiff {
        path: "README.md".into(),
        side: gitcore::Side::Unstaged,
    });
    let evs = until(&w, |e| matches!(e, Event::DiffLoaded(_)));
    let Some(Event::DiffLoaded(shown)) = evs.last().cloned() else {
        unreachable!()
    };
    std::fs::write(d.path().join("README.md"), "hello\nnew\nmore\n").unwrap();
    w.send(Command::Stage {
        path: "README.md".into(),
        selection: gitcore::Selection::Lines(vec![(0, 1)]),
        shown: Some(shown),
    });
    let evs = until(&w, |e| matches!(e, Event::DiffLoaded(_)));
    assert!(evs.iter().any(|e| matches!(e, Event::Error { during: Op::Changes, error } if error.message == retrogit::strings::INFO_STALE_SELECTION)));
}

#[test]
fn changes_commands_without_an_open_repo_do_nothing() {
    let server = mockito::Server::new();
    let w = start(&server, Arc::new(MemoryStore::default()), "");
    w.send(Command::RefreshStatus);
    w.send(Command::ValidateToken);
    // The first event is the reply to ValidateToken: RefreshStatus was ignored.
    let evs = until(&w, |e| matches!(e, Event::SignedOut));
    assert_eq!(evs.len(), 1);
}

#[test]
fn refresh_requests_are_deduplicated_while_one_is_pending() {
    let mut server = mockito::Server::new();
    let _c = mock_device_code(&mut server);
    let _t = server
        .mock("POST", "/login/oauth/access_token")
        .with_body(r#"{"error":"authorization_pending"}"#)
        .create();
    let d = repo_for_changes();
    let w = start(&server, Arc::new(MemoryStore::default()), "Iv1.test");
    w.send(Command::OpenRepo(d.path().to_path_buf()));
    until(&w, |e| matches!(e, Event::StatusLoaded(_)));
    // Keep the worker busy (waiting for authorization) while refreshes pile up.
    w.send(Command::StartDeviceFlow);
    until(&w, |e| matches!(e, Event::DeviceCode { .. }));
    let refresh = w.refresher();
    for _ in 0..10 {
        refresh();
        w.send(Command::RefreshStatus);
    }
    w.cancel_device_flow();
    w.send(Command::ValidateToken); // marker: replies SignedOut
    let evs = until(&w, |e| matches!(e, Event::SignedOut));
    let refreshes = evs
        .iter()
        .filter(|e| matches!(e, Event::StatusLoaded(_)))
        .count();
    assert_eq!(refreshes, 1, "{evs:?}");
}
```

- [ ] **Step 3: Run the tests to see them fail**

Run: `cargo test -p retrogit --tests`

Expected: compile errors: unknown `Event::StatusLoaded`, `Command::Stage`, field `commit_backend`, `state.changes`, etc.

- [ ] **Step 4: Extend the protocol**

Replace the whole file with the version below (it keeps everything from sub-project 1).

`crates/app/src/protocol.rs`:

```rust
//! Messages between the UI thread and the worker thread.

use std::path::PathBuf;

use gitcore::{
    CloneProgress, CommitOutcome, FileDiff, FileStatus, GitError, RepoSummary, Selection, Side,
};
use github::{DeviceFlowFailure, GithubError, RepoInfo, TokenStoreError, User};

use crate::strings as s;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    ValidateToken,
    StartDeviceFlow,
    SavePat(String),
    SignOut,
    ListRepos,
    Clone {
        url: String,
        dest: PathBuf,
    },
    OpenRepo(PathBuf),
    // --- Sub-project 2: all apply to the repository opened last. ---
    RefreshStatus,
    LoadDiff {
        path: String,
        side: Side,
    },
    /// `shown` is the diff the selection was made on (stale-selection check).
    Stage {
        path: String,
        selection: Selection,
        shown: Option<FileDiff>,
    },
    Unstage {
        path: String,
        selection: Selection,
        shown: Option<FileDiff>,
    },
    Commit {
        message: String,
        amend: bool,
    },
    AddToGitignore(String),
    /// Reply: `Event::AmendInfo`.
    LoadAmendInfo,
}

/// Which operation an error belongs to, so the state can reset the right thing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Op {
    Auth,
    Repos,
    Clone,
    Open(PathBuf),
    /// Status, diff, staging, .gitignore.
    Changes,
    Commit,
    Internal,
}

#[derive(Debug, Clone)]
pub enum Event {
    SignedIn(User),
    SignedOut,
    /// A token is stored but GitHub could not be reached; it is kept for later calls.
    Offline,
    DeviceCode {
        user_code: String,
        verification_uri: String,
    },
    DeviceFlowCancelled,
    ReposLoaded(Vec<RepoInfo>),
    CloneProgress(CloneProgress),
    CloneDone(RepoSummary),
    CloneCancelled,
    RepoOpened(RepoSummary),
    StatusLoaded(Vec<FileStatus>),
    DiffLoaded(FileDiff),
    Committed(CommitOutcome),
    /// Last commit message and whether HEAD is already on its upstream.
    AmendInfo {
        message: Option<String>,
        pushed: bool,
    },
    Error {
        during: Op,
        error: AppError,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
    Info,
}

/// What the message box shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppError {
    pub severity: Severity,
    pub message: String,
    /// Technical detail (already redacted), shown in small text.
    pub detail: Option<String>,
    /// Clickable link (e.g. SSO authorization page).
    pub link: Option<String>,
}

impl AppError {
    pub fn new(severity: Severity, message: &str) -> AppError {
        AppError {
            severity,
            message: message.to_string(),
            detail: None,
            link: None,
        }
    }

    fn with_detail(mut self, detail: impl ToString) -> AppError {
        self.detail = Some(detail.to_string());
        self
    }

    pub fn from_github(e: &GithubError) -> AppError {
        match e {
            GithubError::Unauthorized => AppError::new(Severity::Warning, s::ERR_UNAUTHORIZED),
            GithubError::SsoRequired { url } => {
                let mut a = AppError::new(Severity::Warning, s::ERR_SSO);
                a.link = Some(url.clone());
                a
            }
            GithubError::RateLimited => AppError::new(Severity::Warning, s::ERR_RATE_LIMIT),
            GithubError::Network(d) => {
                AppError::new(Severity::Warning, s::ERR_NO_NETWORK).with_detail(d)
            }
            other => AppError::new(Severity::Error, &other.to_string()),
        }
    }

    pub fn from_git(e: &GitError) -> AppError {
        match e {
            GitError::DestinationNotEmpty(p) => {
                AppError::new(Severity::Error, s::ERR_DEST_NOT_EMPTY).with_detail(p.display())
            }
            GitError::NotARepository(p) => {
                AppError::new(Severity::Error, s::ERR_NOT_A_REPO).with_detail(p.display())
            }
            GitError::Cancelled => AppError::new(Severity::Info, s::INFO_CLONE_CANCELLED),
            GitError::Auth(d) => AppError::new(Severity::Error, s::ERR_GIT_AUTH).with_detail(d),
            GitError::Network(d) => {
                AppError::new(Severity::Warning, s::ERR_NO_NETWORK).with_detail(d)
            }
            GitError::StaleSelection => AppError::new(Severity::Info, s::INFO_STALE_SELECTION),
            GitError::Unsupported(d) => AppError::new(Severity::Warning, d),
            GitError::CommitRejected { output } => {
                AppError::new(Severity::Error, s::ERR_COMMIT_REJECTED).with_detail(output)
            }
            GitError::MissingIdentity => AppError::new(Severity::Error, s::ERR_MISSING_IDENTITY),
            GitError::Other(d) => AppError::new(Severity::Error, d),
        }
    }

    pub fn from_device_flow(f: &DeviceFlowFailure) -> AppError {
        match f {
            DeviceFlowFailure::Expired => AppError::new(Severity::Warning, s::ERR_DEVICE_EXPIRED),
            DeviceFlowFailure::Denied => AppError::new(Severity::Warning, s::ERR_DEVICE_DENIED),
            DeviceFlowFailure::Other(code) => {
                AppError::new(Severity::Error, &format!("GitHub sign-in failed: {code}"))
            }
        }
    }

    pub fn from_store(e: &TokenStoreError) -> AppError {
        AppError::new(Severity::Error, s::ERR_KEYCHAIN).with_detail(e)
    }
}
```

- [ ] **Step 5: Add the Changes view to the state**

Replace the whole file with the version below (it keeps everything from sub-project 1). `switch_repo` resets the view only when a *different* repository is opened, so a draft message survives the refresh that follows each commit.

`crates/app/src/state.rs`:

```rust
//! All UI state, updated by the pure `apply` function.

use std::collections::{BTreeSet, HashSet, VecDeque};
use std::path::{Path, PathBuf};

use gitcore::{CloneProgress, FileDiff, FileStatus, RepoSummary, Side};
use github::{RepoInfo, User};

use crate::config::Config;
use crate::protocol::{AppError, Event, Op, Severity};
use crate::strings as s;

/// Diffs longer than this are only shown on request.
pub const LARGE_DIFF_LINES: usize = 20_000;

/// The "Changes" screen of the open repository (sub-project 2).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ChangesView {
    pub files: Vec<FileStatus>,
    /// File whose diff is displayed, and which side.
    pub shown: Option<(String, Side)>,
    pub diff: Option<FileDiff>,
    /// Checked `(hunk, line)` pairs of `diff`; cleared whenever a new diff arrives.
    pub selected_lines: BTreeSet<(usize, usize)>,
    pub show_large: bool,
    pub summary: String,
    pub description: String,
    pub amend: bool,
    pub head_pushed: bool,
    pub committing: bool,
    /// The "git not found" warning was already shown once.
    pub warned_no_cli: bool,
    /// Status bar note after a commit, e.g. "Committed a1b2c3d".
    pub last_commit_note: Option<String>,
    /// Ask the UI to focus the Summary field on the next frame (Repository > Commit...).
    pub focus_summary: bool,
}

impl ChangesView {
    pub fn staged(&self) -> impl Iterator<Item = &FileStatus> {
        self.files.iter().filter(|f| f.staged.is_some())
    }

    pub fn unstaged(&self) -> impl Iterator<Item = &FileStatus> {
        self.files.iter().filter(|f| f.unstaged.is_some())
    }

    /// Commit message: summary, then a blank line and the description if any.
    pub fn commit_message(&self) -> String {
        let summary = self.summary.trim();
        let description = self.description.trim();
        if description.is_empty() {
            summary.to_string()
        } else {
            format!("{summary}\n\n{description}")
        }
    }

    pub fn can_commit(&self) -> bool {
        !self.committing
            && !self.summary.trim().is_empty()
            && (self.amend || self.staged().next().is_some())
    }

    /// Whether `(path, side)` still has changes on that side.
    fn has(&self, path: &str, side: Side) -> bool {
        self.files.iter().any(|f| {
            f.path == path
                && match side {
                    Side::Staged => f.staged.is_some(),
                    Side::Unstaged => f.unstaged.is_some(),
                }
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Auth {
    /// Validating the stored token at startup.
    Checking,
    SignedOut,
    /// Token stored but GitHub unreachable at startup.
    Offline,
    /// Device Flow requested, code not received yet.
    Starting,
    /// Device Flow code shown, waiting for the user on github.com.
    Waiting {
        user_code: String,
        verification_uri: String,
    },
    SignedIn(User),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SignInDialog {
    pub tab: usize,
    pub pat: String,
    pub pat_submitted: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct CloneDialog {
    pub filter: String,
    /// `full_name` of the selected repository (stable across filtering).
    pub selected: Option<String>,
    pub dest_parent: String,
    /// `Some` while a clone is running.
    pub progress: Option<CloneProgress>,
    pub cloning_name: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AppState {
    pub config: Config,
    pub config_dirty: bool,
    pub auth: Auth,
    pub repos: Vec<RepoInfo>,
    pub repos_loading: bool,
    pub sign_in: Option<SignInDialog>,
    pub clone: Option<CloneDialog>,
    pub about: bool,
    pub current: Option<RepoSummary>,
    /// Recent entries whose folder is gone.
    pub missing: HashSet<PathBuf>,
    /// Message boxes waiting to be shown, oldest first.
    pub messages: VecDeque<AppError>,
    pub changes: ChangesView,
}

impl AppState {
    pub fn new(config: Config) -> AppState {
        let missing = config
            .recent
            .iter()
            .filter(|r| !r.path.exists())
            .map(|r| r.path.clone())
            .collect();
        AppState {
            config,
            config_dirty: false,
            auth: Auth::Checking,
            repos: Vec::new(),
            repos_loading: false,
            sign_in: None,
            clone: None,
            about: false,
            current: None,
            missing,
            messages: VecDeque::new(),
            changes: ChangesView::default(),
        }
    }

    pub fn user(&self) -> Option<&User> {
        match &self.auth {
            Auth::SignedIn(u) => Some(u),
            _ => None,
        }
    }

    pub fn apply(&mut self, event: Event) {
        match event {
            Event::SignedIn(user) => {
                self.auth = Auth::SignedIn(user);
                self.sign_in = None;
            }
            Event::SignedOut => {
                self.auth = Auth::SignedOut;
                self.repos.clear();
                self.clone = None;
                self.sign_in.get_or_insert_with(SignInDialog::default);
            }
            Event::Offline => {
                self.auth = Auth::Offline;
            }
            Event::DeviceCode {
                user_code,
                verification_uri,
            } => {
                self.auth = Auth::Waiting {
                    user_code,
                    verification_uri,
                };
            }
            Event::DeviceFlowCancelled => {
                if !matches!(self.auth, Auth::SignedIn(_)) {
                    self.auth = Auth::SignedOut;
                }
            }
            Event::ReposLoaded(repos) => {
                self.repos = repos;
                self.repos_loading = false;
            }
            Event::CloneProgress(p) => {
                if let Some(c) = self.clone.as_mut() {
                    c.progress = Some(p);
                }
            }
            Event::CloneDone(summary) => {
                self.clone = None;
                self.switch_repo(summary);
            }
            Event::CloneCancelled => {
                if let Some(c) = self.clone.as_mut() {
                    c.progress = None;
                }
                self.messages.push_back(AppError::new(
                    crate::protocol::Severity::Info,
                    crate::strings::INFO_CLONE_CANCELLED,
                ));
            }
            Event::RepoOpened(summary) => self.switch_repo(summary),
            Event::StatusLoaded(files) => {
                let c = &mut self.changes;
                c.files = files;
                if let Some((path, side)) = c.shown.clone()
                    && !c.has(&path, side)
                {
                    c.shown = None;
                    c.diff = None;
                    c.selected_lines.clear();
                }
            }
            Event::DiffLoaded(diff) => {
                let c = &mut self.changes;
                if c.shown
                    .as_ref()
                    .is_some_and(|(p, side)| *p == diff.path && *side == diff.side)
                {
                    c.diff = Some(diff);
                    c.selected_lines.clear();
                    c.show_large = false;
                }
            }
            Event::Committed(outcome) => {
                let c = &mut self.changes;
                c.committing = false;
                c.summary.clear();
                c.description.clear();
                c.amend = false;
                c.head_pushed = false;
                c.last_commit_note = Some(format!("{} {}", s::COMMITTED, outcome.commit.short_id));
                if !outcome.used_cli && !c.warned_no_cli {
                    c.warned_no_cli = true;
                    self.messages
                        .push_back(AppError::new(Severity::Warning, s::WARN_NO_GIT_CLI));
                }
            }
            Event::AmendInfo { message, pushed } => {
                let c = &mut self.changes;
                c.head_pushed = pushed;
                if c.amend
                    && c.summary.is_empty()
                    && c.description.is_empty()
                    && let Some(m) = message
                {
                    let (summary, description) = m.split_once('\n').unwrap_or((m.as_str(), ""));
                    c.summary = summary.trim().to_string();
                    c.description = description.trim().to_string();
                }
            }
            Event::Error { during, error } => {
                self.on_error(during);
                self.messages.push_back(error);
            }
        }
    }

    fn on_error(&mut self, during: Op) {
        match during {
            Op::Auth => {
                if !matches!(self.auth, Auth::SignedIn(_)) {
                    self.auth = Auth::SignedOut;
                }
                if let Some(d) = self.sign_in.as_mut() {
                    d.pat_submitted = false;
                }
            }
            Op::Repos => self.repos_loading = false,
            Op::Clone => {
                if let Some(c) = self.clone.as_mut() {
                    c.progress = None;
                }
            }
            Op::Open(path) => {
                if self.config.recent.iter().any(|r| r.path == path) && !path.exists() {
                    self.missing.insert(path);
                }
            }
            Op::Changes => {}
            Op::Commit => self.changes.committing = false,
            Op::Internal => {
                self.repos_loading = false;
                self.changes.committing = false;
                if let Some(c) = self.clone.as_mut() {
                    c.progress = None;
                }
            }
        }
    }

    /// Show `summary` as the current repo; the Changes screen restarts if it is another repo.
    fn switch_repo(&mut self, summary: RepoSummary) {
        self.remember(&summary);
        if self.current.as_ref().map(|c| &c.path) != Some(&summary.path) {
            let warned = self.changes.warned_no_cli;
            self.changes = ChangesView {
                warned_no_cli: warned,
                ..ChangesView::default()
            };
        }
        self.current = Some(summary);
    }

    fn remember(&mut self, summary: &RepoSummary) {
        self.config.add_recent(&summary.name, &summary.path);
        self.missing.remove(&summary.path);
        self.config_dirty = true;
    }

    pub fn remove_recent(&mut self, path: &Path) {
        self.config.remove_recent(path);
        self.missing.remove(path);
        if self.current.as_ref().is_some_and(|c| c.path == path) {
            self.current = None;
        }
        self.config_dirty = true;
    }
}

/// Indexes of repos whose `owner/name` contains `filter` (case-insensitive).
pub fn filter_repos(repos: &[RepoInfo], filter: &str) -> Vec<usize> {
    let needle = filter.trim().to_lowercase();
    repos
        .iter()
        .enumerate()
        .filter(|(_, r)| needle.is_empty() || r.full_name.to_lowercase().contains(&needle))
        .map(|(i, _)| i)
        .collect()
}
```

- [ ] **Step 6: Route the new commands in the worker**

Replace the whole file with the version below (it keeps everything from sub-project 1).

`crates/app/src/worker.rs`:

```rust
//! The single background thread doing all network and Git work.

mod changes;

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use gitcore::{CloneRequest, CommitBackend, Credentials, GitError, Repo, Side};
use github::{Client, DeviceFlow, GithubError, Step, TokenStore};

use crate::logging;
use crate::protocol::{AppError, Command, Event, Op, Severity};
use crate::strings as s;

pub struct WorkerDeps {
    pub client: Client,
    pub store: Arc<dyn TokenStore>,
    /// Empty = Device Flow unavailable (PAT only).
    pub client_id: String,
    pub commit_backend: CommitBackend,
}

/// UI-side handle. Cancellation flags bypass the command queue so they act immediately.
pub struct WorkerHandle {
    tx: Sender<Command>,
    pub events: Receiver<Event>,
    cancel_flow: Arc<AtomicBool>,
    cancel_clone: Arc<AtomicBool>,
    busy: Arc<AtomicBool>,
    refresh_pending: Arc<AtomicBool>,
}

impl WorkerHandle {
    pub fn send(&self, cmd: Command) {
        match cmd {
            Command::StartDeviceFlow => self.cancel_flow.store(false, Ordering::SeqCst),
            Command::Clone { .. } => self.cancel_clone.store(false, Ordering::SeqCst),
            // At most one refresh waiting in the queue.
            Command::RefreshStatus if self.refresh_pending.swap(true, Ordering::SeqCst) => return,
            _ => {}
        }
        if self.tx.send(cmd).is_err() {
            log::error!("worker thread is gone");
        }
    }

    /// Thread-safe "please refresh the status" callback (for the file watcher).
    pub fn refresher(&self) -> impl Fn() + Send + 'static {
        let tx = self.tx.clone();
        let pending = self.refresh_pending.clone();
        move || {
            if !pending.swap(true, Ordering::SeqCst) {
                let _ = tx.send(Command::RefreshStatus);
            }
        }
    }

    pub fn cancel_device_flow(&self) {
        self.cancel_flow.store(true, Ordering::SeqCst);
    }

    pub fn cancel_clone(&self) {
        self.cancel_clone.store(true, Ordering::SeqCst);
    }

    /// Cancel whatever is running and wait (up to `timeout`) for the worker to be idle,
    /// so a clone interrupted by quitting still removes its partial folder.
    /// Returns `true` if the worker became idle in time.
    pub fn shutdown(&self, timeout: Duration) -> bool {
        self.cancel_device_flow();
        self.cancel_clone();
        let deadline = Instant::now() + timeout;
        while self.busy.load(Ordering::SeqCst) {
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        true
    }
}

/// Emit at most one progress event per `every` (the last one is always sent separately).
pub struct Throttle {
    every: Duration,
    last: Option<Instant>,
}

impl Throttle {
    pub fn new(every: Duration) -> Throttle {
        Throttle { every, last: None }
    }

    pub fn ready(&mut self, now: Instant) -> bool {
        match self.last {
            Some(t) if now.duration_since(t) < self.every => false,
            _ => {
                self.last = Some(now);
                true
            }
        }
    }
}

/// Start the worker thread. `notify` is called after each event (wakes up egui).
pub fn spawn(deps: WorkerDeps, notify: impl Fn() + Send + 'static) -> WorkerHandle {
    let (tx, rx) = channel::<Command>();
    let (etx, erx) = channel::<Event>();
    let cancel_flow = Arc::new(AtomicBool::new(false));
    let cancel_clone = Arc::new(AtomicBool::new(false));
    let busy = Arc::new(AtomicBool::new(false));
    let worker_busy = busy.clone();
    let refresh_pending = Arc::new(AtomicBool::new(false));
    let mut worker = Worker {
        deps,
        token: None,
        repo: None,
        shown: None,
        refresh_pending: refresh_pending.clone(),
        cancel_flow: cancel_flow.clone(),
        cancel_clone: cancel_clone.clone(),
        emit: Box::new(move |ev| {
            let _ = etx.send(ev);
            notify();
        }),
    };
    let spawned = std::thread::Builder::new()
        .name("retrogit-worker".into())
        .spawn(move || {
            for cmd in rx {
                worker_busy.store(true, Ordering::SeqCst);
                let name = format!("{cmd:?}");
                let outcome = catch_unwind(AssertUnwindSafe(|| worker.handle(cmd)));
                worker_busy.store(false, Ordering::SeqCst);
                if outcome.is_err() {
                    log::error!(
                        "worker panicked while handling {}",
                        logging::redact(&name, &[])
                    );
                    (worker.emit)(Event::Error {
                        during: Op::Internal,
                        error: AppError::new(Severity::Error, s::ERR_INTERNAL),
                    });
                }
            }
        });
    if let Err(e) = spawned {
        log::error!("could not start worker thread: {e}");
    }
    WorkerHandle {
        tx,
        events: erx,
        cancel_flow,
        cancel_clone,
        busy,
        refresh_pending,
    }
}

struct Worker {
    deps: WorkerDeps,
    token: Option<String>,
    /// Repository opened last (target of all sub-project 2 commands).
    repo: Option<std::path::PathBuf>,
    /// File whose diff the UI displays; its diff is re-sent after every change.
    shown: Option<(String, Side)>,
    refresh_pending: Arc<AtomicBool>,
    cancel_flow: Arc<AtomicBool>,
    cancel_clone: Arc<AtomicBool>,
    emit: Box<dyn Fn(Event) + Send>,
}

impl Worker {
    fn emit(&self, ev: Event) {
        (self.emit)(ev);
    }

    fn fail(&self, during: Op, error: AppError) {
        log::warn!("{during:?}: {} {:?}", error.message, error.detail);
        self.emit(Event::Error { during, error });
    }

    fn handle(&mut self, cmd: Command) {
        match cmd {
            Command::ValidateToken => self.validate(),
            Command::StartDeviceFlow => self.device_flow(),
            Command::SavePat(t) => self.sign_in_with(t.trim().to_string(), true),
            Command::SignOut => self.sign_out(),
            Command::ListRepos => self.list_repos(),
            Command::Clone { url, dest } => self.clone(url, dest),
            Command::OpenRepo(path) => match Repo::open(&path).and_then(|r| r.summary()) {
                Ok(summary) => self.opened(summary, false),
                Err(e) => self.fail(Op::Open(path), AppError::from_git(&e)),
            },
            Command::RefreshStatus => {
                self.refresh_pending.store(false, Ordering::SeqCst);
                self.refresh();
            }
            Command::LoadDiff { path, side } => self.load_diff(path, side),
            Command::Stage {
                path,
                selection,
                shown,
            } => self.stage(&path, &selection, shown.as_ref(), true),
            Command::Unstage {
                path,
                selection,
                shown,
            } => self.stage(&path, &selection, shown.as_ref(), false),
            Command::Commit { message, amend } => self.commit(&message, amend),
            Command::AddToGitignore(pattern) => self.add_to_gitignore(&pattern),
            Command::LoadAmendInfo => self.amend_info(),
        }
    }

    fn validate(&mut self) {
        let token = match self.deps.store.load() {
            Ok(Some(t)) => t,
            Ok(None) => return self.emit(Event::SignedOut),
            Err(e) => {
                self.fail(Op::Auth, AppError::from_store(&e));
                return self.emit(Event::SignedOut);
            }
        };
        logging::add_secret(&token);
        match self.deps.client.current_user(&token) {
            Ok(user) => {
                self.token = Some(token);
                self.emit(Event::SignedIn(user));
            }
            Err(GithubError::Unauthorized) => {
                let _ = self.deps.store.clear();
                self.emit(Event::SignedOut);
            }
            Err(e) => {
                // Keep the token: the network may come back (VPN, Wi-Fi).
                self.token = Some(token);
                self.fail(Op::Auth, AppError::from_github(&e));
                self.emit(Event::Offline);
            }
        }
    }

    /// Validate `token` with `GET /user`, then store it.
    fn sign_in_with(&mut self, token: String, is_pat: bool) {
        logging::add_secret(&token);
        match self.deps.client.current_user(&token) {
            Ok(user) => {
                if let Err(e) = self.deps.store.save(&token) {
                    // Still signed in for this session; warn that it won't persist.
                    self.fail(Op::Auth, AppError::from_store(&e));
                }
                self.token = Some(token);
                self.emit(Event::SignedIn(user));
            }
            Err(GithubError::Unauthorized) if is_pat => {
                self.fail(
                    Op::Auth,
                    AppError::new(Severity::Warning, s::ERR_PAT_REJECTED),
                );
            }
            Err(e) => self.fail(Op::Auth, AppError::from_github(&e)),
        }
    }

    fn device_flow(&mut self) {
        if self.deps.client_id.is_empty() {
            return self.fail(Op::Auth, AppError::new(Severity::Info, s::ERR_NO_CLIENT_ID));
        }
        let code = match self
            .deps
            .client
            .request_device_code(&self.deps.client_id, github::SCOPES)
        {
            Ok(c) => c,
            Err(e) => return self.fail(Op::Auth, AppError::from_github(&e)),
        };
        let started = Instant::now();
        self.emit(Event::DeviceCode {
            user_code: code.user_code.clone(),
            verification_uri: code.verification_uri.clone(),
        });
        let mut flow = DeviceFlow::new(&code);
        let mut wait = flow.first_wait();
        loop {
            if !sleep_unless_cancelled(wait, &self.cancel_flow) {
                return self.emit(Event::DeviceFlowCancelled);
            }
            let response = match self
                .deps
                .client
                .poll_token(&self.deps.client_id, &code.device_code)
            {
                Ok(r) => r,
                Err(e) => return self.fail(Op::Auth, AppError::from_github(&e)),
            };
            match flow.on_response(response, started.elapsed()) {
                Step::Wait(d) => wait = d,
                Step::Done(token) => return self.sign_in_with(token, false),
                Step::Failed(f) => return self.fail(Op::Auth, AppError::from_device_flow(&f)),
            }
        }
    }

    fn sign_out(&mut self) {
        self.token = None;
        if let Err(e) = self.deps.store.clear() {
            self.fail(Op::Auth, AppError::from_store(&e));
        }
        self.emit(Event::SignedOut);
    }

    fn list_repos(&mut self) {
        let Some(token) = self.token.clone() else {
            return self.emit(Event::SignedOut);
        };
        match self.deps.client.list_repos(&token) {
            Ok(listing) => {
                self.emit(Event::ReposLoaded(listing.repos));
                if !listing.sso_hidden_orgs.is_empty() {
                    let mut warning = AppError::new(Severity::Warning, s::ERR_SSO_PARTIAL);
                    warning.link = Some(self.sso_settings_link());
                    self.fail(Op::Repos, warning);
                }
            }
            Err(GithubError::Unauthorized) => self.drop_token(Op::Repos),
            Err(e) => self.fail(Op::Repos, AppError::from_github(&e)),
        }
    }

    fn clone(&mut self, url: String, dest: std::path::PathBuf) {
        let credentials = self.token.clone().map(|t| Credentials {
            username: "x-access-token".into(),
            password: t,
        });
        let req = CloneRequest {
            url,
            dest,
            credentials,
        };
        let mut throttle = Throttle::new(Duration::from_millis(50));
        let mut last = None;
        let emit = &self.emit;
        let result = gitcore::clone(
            &req,
            |p| {
                last = Some(p);
                if throttle.ready(Instant::now()) {
                    emit(Event::CloneProgress(p));
                }
            },
            &self.cancel_clone,
        );
        if let Some(p) = last {
            self.emit(Event::CloneProgress(p));
        }
        match result.and_then(|repo| repo.summary()) {
            Ok(summary) => self.opened(summary, true),
            Err(GitError::Cancelled) => self.emit(Event::CloneCancelled),
            Err(e @ GitError::Auth(_)) => self.clone_auth_failed(&e),
            Err(e) => self.fail(Op::Clone, AppError::from_git(&e)),
        }
    }
}

impl Worker {
    /// Where the user grants SSO access: the OAuth App's connection page, or token settings.
    fn sso_settings_link(&self) -> String {
        if self.deps.client_id.is_empty() {
            "https://github.com/settings/tokens".to_string()
        } else {
            format!(
                "https://github.com/settings/connections/applications/{}",
                self.deps.client_id
            )
        }
    }

    /// The token was rejected: forget it everywhere and ask to sign in again.
    fn drop_token(&mut self, during: Op) {
        self.token = None;
        let _ = self.deps.store.clear();
        self.fail(during, AppError::from_github(&GithubError::Unauthorized));
        self.emit(Event::SignedOut);
    }

    /// Git refused our credentials: a revoked token or a missing SSO authorization.
    fn clone_auth_failed(&mut self, e: &GitError) {
        if let Some(token) = self.token.clone()
            && self.deps.client.current_user(&token) == Err(GithubError::Unauthorized)
        {
            return self.drop_token(Op::Clone);
        }
        let mut error = AppError::from_git(e);
        error.link = Some(self.sso_settings_link());
        self.fail(Op::Clone, error);
    }
}

/// Sleep `d` in small steps. Returns `false` if cancelled.
fn sleep_unless_cancelled(d: Duration, cancel: &AtomicBool) -> bool {
    let end = Instant::now() + d;
    while Instant::now() < end {
        if cancel.load(Ordering::SeqCst) {
            return false;
        }
        std::thread::sleep(
            Duration::from_millis(50).min(end.saturating_duration_since(Instant::now())),
        );
    }
    !cancel.load(Ordering::SeqCst)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn throttle_limits_rate() {
        let mut t = Throttle::new(Duration::from_millis(50));
        let t0 = Instant::now();
        assert!(t.ready(t0));
        assert!(!t.ready(t0 + Duration::from_millis(10)));
        assert!(t.ready(t0 + Duration::from_millis(60)));
    }

    #[test]
    fn sleep_stops_when_cancelled() {
        let flag = AtomicBool::new(true);
        let t0 = Instant::now();
        assert!(!sleep_unless_cancelled(Duration::from_secs(5), &flag));
        assert!(t0.elapsed() < Duration::from_secs(1));
        assert!(sleep_unless_cancelled(
            Duration::from_millis(1),
            &AtomicBool::new(false)
        ));
    }
}
```

- [ ] **Step 7: Implement the worker side of the Changes screen**

`crates/app/src/worker/changes.rs`:

```rust
//! Worker side of sub-project 2: status, diff, staging, commit, .gitignore.

use gitcore::{FileDiff, Repo, RepoSummary, Selection, Side};

use super::Worker;
use crate::protocol::{AppError, Event, Op};

impl Worker {
    /// A repository was opened or cloned: it becomes the target of later commands.
    pub(super) fn opened(&mut self, summary: RepoSummary, cloned: bool) {
        if self.repo.as_ref() != Some(&summary.path) {
            self.shown = None;
        }
        self.repo = Some(summary.path.clone());
        self.emit(if cloned {
            Event::CloneDone(summary)
        } else {
            Event::RepoOpened(summary)
        });
        self.refresh();
    }

    fn open_current(&self, during: Op) -> Option<Repo> {
        let path = self.repo.as_ref()?;
        match Repo::open(path) {
            Ok(r) => Some(r),
            Err(e) => {
                self.fail(during, AppError::from_git(&e));
                None
            }
        }
    }

    /// Send the status, and the diff of the displayed file.
    pub(super) fn refresh(&mut self) {
        let Some(repo) = self.open_current(Op::Changes) else {
            return;
        };
        match repo.status() {
            Ok(files) => self.emit(Event::StatusLoaded(files)),
            Err(e) => return self.fail(Op::Changes, AppError::from_git(&e)),
        }
        if let Some((path, side)) = self.shown.clone() {
            self.send_diff(&repo, &path, side);
        }
    }

    fn send_diff(&self, repo: &Repo, path: &str, side: Side) {
        match repo.diff_file(path, side) {
            Ok(diff) => self.emit(Event::DiffLoaded(diff)),
            Err(e) => self.fail(Op::Changes, AppError::from_git(&e)),
        }
    }

    pub(super) fn load_diff(&mut self, path: String, side: Side) {
        let Some(repo) = self.open_current(Op::Changes) else {
            return;
        };
        self.send_diff(&repo, &path, side);
        self.shown = Some((path, side));
    }

    pub(super) fn stage(
        &mut self,
        path: &str,
        selection: &Selection,
        shown: Option<&FileDiff>,
        stage: bool,
    ) {
        let Some(repo) = self.open_current(Op::Changes) else {
            return;
        };
        let result = if stage {
            repo.stage(path, selection, shown)
        } else {
            repo.unstage(path, selection, shown)
        };
        if let Err(e) = result {
            self.fail(Op::Changes, AppError::from_git(&e));
        }
        // Always resync: on a stale selection this reloads the diff the user must redo.
        self.refresh();
    }

    pub(super) fn commit(&mut self, message: &str, amend: bool) {
        let Some(repo) = self.open_current(Op::Commit) else {
            return;
        };
        match repo.commit(message, amend, self.deps.commit_backend) {
            Ok(outcome) => {
                self.emit(Event::Committed(outcome));
                if let Ok(summary) = repo.summary() {
                    self.emit(Event::RepoOpened(summary));
                }
            }
            Err(e) => self.fail(Op::Commit, AppError::from_git(&e)),
        }
        self.refresh();
    }

    pub(super) fn add_to_gitignore(&mut self, pattern: &str) {
        let Some(repo) = self.open_current(Op::Changes) else {
            return;
        };
        if let Err(e) = repo.add_to_gitignore(pattern) {
            self.fail(Op::Changes, AppError::from_git(&e));
        }
        self.refresh();
    }

    pub(super) fn amend_info(&mut self) {
        let Some(repo) = self.open_current(Op::Commit) else {
            return;
        };
        let message = repo.last_commit_message().unwrap_or(None);
        let pushed = repo.head_is_pushed().unwrap_or(false);
        self.emit(Event::AmendInfo { message, pushed });
    }
}
```

- [ ] **Step 8: Pass the commit backend and PATH from main**

Replace the whole file with the version below (it keeps everything from sub-project 1). The login-shell PATH is resolved on a background thread so a slow shell (nvm, conda) never delays the window.

`crates/app/src/main.rs`:

```rust
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::sync::Arc;

use retrogit::app::RetroGitApp;
use retrogit::config::Config;
use retrogit::state::AppState;
use retrogit::worker::{WorkerDeps, spawn};
use retrogit::{GITHUB_CLIENT_ID, logging, strings};

fn main() -> eframe::Result {
    if let Some(dir) = dirs::data_local_dir() {
        logging::init(&dir.join("RetroGit").join("retrogit.log"));
    }
    log::info!("RetroGit {} starting", env!("CARGO_PKG_VERSION"));
    // Resolved in the background so a slow shell never delays the window.
    std::thread::spawn(|| {
        if let Some(path) = retrogit::env_path::login_shell_path(std::time::Duration::from_secs(10))
        {
            gitcore::set_git_search_path(path);
        }
    });
    if let Err(e) = gitcore::configure_network_timeouts() {
        log::warn!("could not set git network timeouts: {e}");
    }

    let config_path = Config::default_path();
    let config = config_path
        .as_deref()
        .map(Config::load_from)
        .unwrap_or_default();

    let mut viewport = egui::ViewportBuilder::default()
        .with_title(strings::APP_NAME)
        .with_decorations(false)
        .with_resizable(true)
        .with_min_inner_size([520.0, 360.0])
        .with_inner_size([900.0, 600.0]);
    if let Some(g) = config.window {
        viewport = viewport.with_inner_size([g.width.max(520.0), g.height.max(360.0)]);
        if let (Some(x), Some(y)) = (g.x, g.y) {
            viewport = viewport.with_position([x, y]);
        }
    }
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    eframe::run_native(
        strings::APP_NAME,
        options,
        Box::new(move |cc| {
            win95::theme::install(&cc.egui_ctx);
            let repaint = cc.egui_ctx.clone();
            let deps = WorkerDeps {
                client: github::Client::github_com(),
                store: Arc::new(github::KeyringStore::new("RetroGit", "github.com")),
                client_id: GITHUB_CLIENT_ID.to_string(),
                commit_backend: gitcore::CommitBackend::PreferCli,
            };
            let worker = spawn(deps, move || repaint.request_repaint());
            Ok(Box::new(RetroGitApp::new(
                AppState::new(config),
                worker,
                config_path,
            )))
        }),
    )
}
```

- [ ] **Step 9: Run the tests to see them pass**

Run: `cargo test -p retrogit --tests`

Expected: `tests/state.rs` `19 passed`; `tests/worker.rs` `21 passed`.

- [ ] **Step 10: Lint**

Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings`

Expected: no output from fmt; clippy finishes without warnings.

- [ ] **Step 11: Full suite**

Run: `cargo test --workspace`

Expected: all tests pass.

- [ ] **Step 12: Commit**

```bash
git add crates/app
git commit -m "feat(app): Changes state and worker commands for status, diff, staging and commit"
```


### Task 6: "Changes" screen

**Files:**
- Create: `crates/app/src/ui/changes.rs`, `crates/app/src/ui/diff_view.rs`
- Modify: `crates/app/src/ui/mod.rs`, `crates/app/src/ui/main_window.rs`, `crates/app/src/app.rs`

**Interfaces:**
- Consumes: `ChangesView`, the new `Command`s, `WorkerHandle::refresher`, `Watcher`, `win95::{checkbox, text_area, ListView, ...}`.
- Produces: `ui::changes::{show, describe(&str, &Change) -> String, paths_of(&FileStatus, Side) -> Vec<String>, extension_pattern(&str) -> Option<String>}`; `ui::diff_view::{show, rows(&FileDiff) -> Vec<Row>, Row::{Hunk(usize), Line(usize, usize)}}`; the main window's central panel shows the Changes screen for an open repo; `RetroGitApp` keeps a watcher on the current repo and refreshes when the window regains focus.

- [ ] **Step 1: Write the failing tests in `crates/app/src/ui/changes.rs`**

Create `crates/app/src/ui/changes.rs` containing only this test module for now (the implementation goes above it in a later step).

`crates/app/src/ui/changes.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn describes_changes_with_win95_style_codes() {
        assert_eq!(describe("a.rs", &Change::Modified), "[M] a.rs");
        assert_eq!(
            describe(
                "new.rs",
                &Change::Renamed {
                    from: "old.rs".into()
                }
            ),
            "[R] old.rs → new.rs"
        );
        assert_eq!(describe("x", &Change::Untracked), "[?] x");
    }

    #[test]
    fn renames_touch_both_paths() {
        let f = FileStatus {
            path: "new.rs".into(),
            staged: Some(Change::Renamed {
                from: "old.rs".into(),
            }),
            unstaged: None,
        };
        assert_eq!(
            paths_of(&f, Side::Staged),
            vec!["new.rs".to_string(), "old.rs".to_string()]
        );
        assert_eq!(paths_of(&f, Side::Unstaged), vec!["new.rs".to_string()]);
    }

    #[test]
    fn extension_patterns() {
        assert_eq!(extension_pattern("logs/app.log").as_deref(), Some("*.log"));
        assert_eq!(extension_pattern("Makefile"), None);
        assert_eq!(extension_pattern(".env"), None);
        assert_eq!(extension_pattern("dir.d/file"), None);
    }
}
```

- [ ] **Step 2: Write the failing tests in `crates/app/src/ui/diff_view.rs`**

Create `crates/app/src/ui/diff_view.rs` containing only this test module for now (the implementation goes above it in a later step).

`crates/app/src/ui/diff_view.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use gitcore::{DiffLine, Hunk};

    fn line(kind: LineKind) -> DiffLine {
        DiffLine {
            kind,
            old_no: None,
            new_no: None,
            text: "x\n".into(),
            no_newline_at_eof: false,
        }
    }

    #[test]
    fn rows_interleave_hunk_headers_and_lines() {
        let hunk = |n| Hunk {
            header: "@@".into(),
            old_start: 1,
            old_lines: 1,
            new_start: 1,
            new_lines: 1,
            lines: (0..n).map(|_| line(LineKind::Context)).collect(),
        };
        let d = FileDiff {
            path: "f".into(),
            side: Side::Unstaged,
            binary: false,
            hunks: vec![hunk(2), hunk(1)],
        };
        assert_eq!(
            rows(&d),
            vec![
                Row::Hunk(0),
                Row::Line(0, 0),
                Row::Line(0, 1),
                Row::Hunk(1),
                Row::Line(1, 0)
            ]
        );
    }
}
```

- [ ] **Step 3: Declare the screens**

Replace the whole file with the version below (it keeps everything from sub-project 1).

`crates/app/src/ui/mod.rs`:

```rust
//! Screens. Each function draws from `AppState` and sends `Command`s to the worker.

pub mod about;
pub mod changes;
pub mod clone_dialog;
pub mod diff_view;
pub mod main_window;
pub mod message;
pub mod sign_in;

use crate::state::AppState;
use crate::worker::WorkerHandle;

/// Everything a screen needs.
pub struct Ctx<'a> {
    pub state: &'a mut AppState,
    pub worker: &'a WorkerHandle,
}
```

- [ ] **Step 4: Run the tests to see them fail**

Run: `cargo test -p retrogit --lib`

Expected: compile errors: `describe`, `paths_of`, `extension_pattern`, `rows`, `Row` not found.

- [ ] **Step 5: Implement the file lists and the commit form**

Insert this **above** the existing `#[cfg(test)]` module in `crates/app/src/ui/changes.rs`.

`crates/app/src/ui/changes.rs`:

```rust
//! The "Changes" screen: file lists, diff, commit form.

use egui::{Panel, RichText};
use gitcore::{Change, FileStatus, Head, Selection, Side};
use win95::{
    Bevel, Button95, Cell, Column, ListView, ProgressBar95, bevel_frame, checkbox, text_area,
    text_field,
};

use super::{Ctx, diff_view};
use crate::protocol::Command;
use crate::strings as s;

pub fn show(ui: &mut egui::Ui, cx: &mut Ctx<'_>) {
    Panel::top("changes_header")
        .frame(egui::Frame::NONE)
        .show(ui, |ui| header(ui, cx));
    Panel::bottom("commit_box")
        .frame(egui::Frame::NONE.inner_margin(egui::Margin::same(2)))
        .show(ui, |ui| commit_box(ui, cx));
    Panel::left("changed_files")
        .frame(egui::Frame::NONE.inner_margin(egui::Margin::same(2)))
        .resizable(true)
        .default_size(260.0)
        .show(ui, |ui| file_lists(ui, cx));
    egui::CentralPanel::default()
        .frame(egui::Frame::NONE.inner_margin(egui::Margin::same(2)))
        .show(ui, |ui| diff_view::show(ui, cx));
    toggle_with_space(ui, cx);
}

fn header(ui: &mut egui::Ui, cx: &mut Ctx<'_>) {
    let Some(c) = &cx.state.current else { return };
    let branch = match &c.head {
        Head::Branch(b) => b.clone(),
        Head::Unborn(b) => format!("{b} {}", s::NO_COMMITS),
        Head::Detached(id) => format!("{id} {}", s::DETACHED),
    };
    let last = c
        .last_commit
        .as_ref()
        .map(|lc| format!(" · {} \"{}\"", lc.short_id, lc.summary))
        .unwrap_or_default();
    ui.horizontal(|ui| {
        ui.label(RichText::new(format!("{} · {branch}{last}", c.name)).strong());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.add(Button95::new(s::REFRESH)).clicked() {
                cx.worker.send(Command::RefreshStatus);
            }
        });
    });
    ui.add_space(2.0);
}

/// `[M]`, `[A]`, ... and the displayed path (`old → new` for renames).
pub fn describe(path: &str, change: &Change) -> String {
    let (code, shown) = match change {
        Change::Added => ("A", path.to_string()),
        Change::Modified => ("M", path.to_string()),
        Change::Deleted => ("D", path.to_string()),
        Change::Renamed { from } => ("R", format!("{from} → {path}")),
        Change::TypeChange => ("T", path.to_string()),
        Change::Untracked => ("?", path.to_string()),
        Change::Conflicted => ("!", path.to_string()),
    };
    format!("[{code}] {shown}")
}

/// Paths a whole-file stage/unstage must touch (both sides of a rename).
pub fn paths_of(file: &FileStatus, side: Side) -> Vec<String> {
    let change = match side {
        Side::Staged => &file.staged,
        Side::Unstaged => &file.unstaged,
    };
    match change {
        Some(Change::Renamed { from }) => vec![file.path.clone(), from.clone()],
        _ => vec![file.path.clone()],
    }
}

/// `*.ext` pattern for a path, if it has an extension.
pub fn extension_pattern(path: &str) -> Option<String> {
    let name = path.rsplit('/').next()?;
    let (stem, ext) = name.rsplit_once('.')?;
    (!stem.is_empty() && !ext.is_empty()).then(|| format!("*.{ext}"))
}

fn toggle_file(cx: &Ctx<'_>, file: &FileStatus, side: Side) {
    if file.unstaged == Some(Change::Conflicted) {
        return;
    }
    for path in paths_of(file, side) {
        let cmd = match side {
            Side::Unstaged => Command::Stage {
                path,
                selection: Selection::All,
                shown: None,
            },
            Side::Staged => Command::Unstage {
                path,
                selection: Selection::All,
                shown: None,
            },
        };
        cx.worker.send(cmd);
    }
}

fn select_file(cx: &mut Ctx<'_>, path: &str, side: Side) {
    let c = &mut cx.state.changes;
    if c.shown.as_ref() != Some(&(path.to_string(), side)) {
        c.shown = Some((path.to_string(), side));
        c.diff = None;
        c.selected_lines.clear();
    }
    cx.worker.send(Command::LoadDiff {
        path: path.to_string(),
        side,
    });
}

const FILE_COLUMNS: &[Column] = &[Column {
    title: "File",
    width: 1000.0,
}];

fn file_lists(ui: &mut egui::Ui, cx: &mut Ctx<'_>) {
    let staged: Vec<FileStatus> = cx.state.changes.staged().cloned().collect();
    let unstaged: Vec<FileStatus> = cx.state.changes.unstaged().cloned().collect();
    let half = ((ui.available_height() - 60.0) / 2.0).max(60.0);
    group(ui, cx, &staged, Side::Staged, half);
    ui.add_space(4.0);
    group(ui, cx, &unstaged, Side::Unstaged, half);
}

fn group(ui: &mut egui::Ui, cx: &mut Ctx<'_>, files: &[FileStatus], side: Side, height: f32) {
    let (title, all_label) = match side {
        Side::Staged => (s::STAGED_CHANGES, s::UNSTAGE_ALL),
        Side::Unstaged => (s::CHANGES, s::STAGE_ALL),
    };
    ui.horizontal(|ui| {
        ui.label(format!("{title} ({})", files.len()));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let b = Button95::new(all_label)
                .min_size(egui::vec2(80.0, 20.0))
                .enabled(!files.is_empty());
            if ui.add(b).clicked() {
                for f in files {
                    toggle_file(cx, f, side);
                }
            }
        });
    });
    let selected = cx
        .state
        .changes
        .shown
        .as_ref()
        .filter(|(_, s)| *s == side)
        .and_then(|(p, _)| files.iter().position(|f| &f.path == p));
    let mut menu_action: Option<(usize, MenuAction)> = None;
    let id = match side {
        Side::Staged => "staged_files",
        Side::Unstaged => "unstaged_files",
    };
    let resp = ListView::new(id, FILE_COLUMNS, files.len())
        .header(false)
        .height(height)
        .context_menu(|row, ui| {
            let toggle = if side == Side::Staged {
                s::UNSTAGE
            } else {
                s::STAGE
            };
            if ui.button(toggle).clicked() {
                menu_action = Some((row, MenuAction::Toggle));
            }
            if side == Side::Unstaged {
                if ui.button(s::IGNORE_FILE).clicked() {
                    menu_action = Some((row, MenuAction::IgnorePath));
                }
                if let Some(pattern) = extension_pattern(&files[row].path) {
                    let label = s::IGNORE_EXT.replace("{ext}", pattern.trim_start_matches("*."));
                    if ui.button(label).clicked() {
                        menu_action = Some((row, MenuAction::Ignore(pattern)));
                    }
                }
            }
        })
        .show(ui, selected, |row, _| {
            let f = &files[row];
            let change = match side {
                Side::Staged => f.staged.as_ref(),
                Side::Unstaged => f.unstaged.as_ref(),
            };
            Cell::from(change.map(|c| describe(&f.path, c)).unwrap_or_default())
        });
    if let Some(row) = resp.double_clicked {
        toggle_file(cx, &files[row], side);
    } else if let Some(row) = resp.clicked {
        select_file(cx, &files[row].path, side);
    }
    match menu_action {
        Some((row, MenuAction::Toggle)) => toggle_file(cx, &files[row], side),
        Some((row, MenuAction::IgnorePath)) => cx
            .worker
            .send(Command::AddToGitignore(files[row].path.clone())),
        Some((_, MenuAction::Ignore(pattern))) => cx.worker.send(Command::AddToGitignore(pattern)),
        None => {}
    }
}

enum MenuAction {
    Toggle,
    IgnorePath,
    Ignore(String),
}

/// Space toggles the displayed file when no text field has the keyboard.
fn toggle_with_space(ui: &egui::Ui, cx: &mut Ctx<'_>) {
    if ui.ctx().egui_wants_keyboard_input() || !ui.input(|i| i.key_pressed(egui::Key::Space)) {
        return;
    }
    let Some((path, side)) = cx.state.changes.shown.clone() else {
        return;
    };
    if let Some(file) = cx
        .state
        .changes
        .files
        .iter()
        .find(|f| f.path == path)
        .cloned()
    {
        toggle_file(cx, &file, side);
    }
}

fn commit_box(ui: &mut egui::Ui, cx: &mut Ctx<'_>) {
    let committing = cx.state.changes.committing;
    let can_commit = cx.state.changes.can_commit();
    let focus = std::mem::take(&mut cx.state.changes.focus_summary);
    let c = &mut cx.state.changes;
    let mut amend_toggled = false;
    let mut commit_clicked = false;
    bevel_frame(ui, Bevel::Sunken, win95::theme::SILVER, 4, |ui| {
        ui.set_width(ui.available_width());
        ui.add_enabled_ui(!committing, |ui| {
            ui.horizontal(|ui| {
                ui.label(s::SUMMARY);
                let r = text_field(
                    ui,
                    &mut c.summary,
                    (ui.available_width() - 190.0).max(120.0),
                    false,
                );
                if focus {
                    r.request_focus();
                }
                let before = c.amend;
                checkbox(ui, &mut c.amend, s::AMEND);
                amend_toggled = c.amend && !before;
            });
            if c.amend && c.head_pushed {
                ui.label(
                    RichText::new(s::AMEND_PUSHED_WARNING)
                        .color(egui::Color32::from_rgb(0x80, 0, 0)),
                );
            }
            ui.horizontal(|ui| {
                ui.label(s::DESCRIPTION);
                text_area(
                    ui,
                    &mut c.description,
                    (ui.available_width() - 100.0).max(120.0),
                    3,
                );
                ui.vertical(|ui| {
                    commit_clicked = ui
                        .add(Button95::new(s::COMMIT).enabled(can_commit))
                        .clicked();
                });
            });
        });
        if committing {
            ui.horizontal(|ui| {
                ui.label(s::COMMITTING);
                ui.add(ProgressBar95::new(None).width(200.0));
            });
        }
    });
    if amend_toggled {
        cx.worker.send(Command::LoadAmendInfo);
    }
    if commit_clicked && can_commit {
        let c = &mut cx.state.changes;
        c.committing = true;
        let message = c.commit_message();
        let amend = c.amend;
        cx.worker.send(Command::Commit { message, amend });
    }
}
```

- [ ] **Step 6: Implement the diff view**

Insert this **above** the existing `#[cfg(test)]` module in `crates/app/src/ui/diff_view.rs`.

`crates/app/src/ui/diff_view.rs`:

```rust
//! Unified diff with per-line check boxes and per-hunk buttons.

use egui::{Color32, RichText, ScrollArea};
use gitcore::{Change, FileDiff, LineKind, Selection, Side};
use win95::{Bevel, Button95, bevel_frame, checkbox};

use super::Ctx;
use crate::protocol::Command;
use crate::state::LARGE_DIFF_LINES;
use crate::strings as s;

const ADDED_BG: Color32 = Color32::from_rgb(0xE6, 0xFF, 0xE6);
const REMOVED_BG: Color32 = Color32::from_rgb(0xFF, 0xE6, 0xE6);
const HUNK_BG: Color32 = Color32::from_rgb(0xE0, 0xE0, 0xF0);
const ROW_HEIGHT: f32 = 17.0;

/// One displayed row: a hunk header or a line of a hunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row {
    Hunk(usize),
    Line(usize, usize),
}

pub fn rows(diff: &FileDiff) -> Vec<Row> {
    let mut out = Vec::with_capacity(diff.line_count() + diff.hunks.len());
    for (h, hunk) in diff.hunks.iter().enumerate() {
        out.push(Row::Hunk(h));
        out.extend((0..hunk.lines.len()).map(|l| Row::Line(h, l)));
    }
    out
}

enum Action {
    Lines,
    Hunk(usize),
    File,
    ShowLarge,
}

pub fn show(ui: &mut egui::Ui, cx: &mut Ctx<'_>) {
    let mut action: Option<Action> = None;
    let c = &mut cx.state.changes;
    bevel_frame(ui, Bevel::Field, win95::theme::WHITE, 4, |ui| {
        ui.set_min_size(ui.available_size());
        let Some((path, side)) = c.shown.clone() else {
            let empty = c.files.is_empty();
            ui.label(if empty {
                s::WORKING_TREE_CLEAN
            } else {
                s::SELECT_A_FILE
            });
            return;
        };
        let (verb_lines, verb_hunk, verb_file, side_label) = match side {
            Side::Unstaged => (
                s::STAGE_LINES,
                s::STAGE_HUNK,
                s::STAGE_FILE,
                s::SIDE_UNSTAGED,
            ),
            Side::Staged => (
                s::UNSTAGE_LINES,
                s::UNSTAGE_HUNK,
                s::UNSTAGE_FILE,
                s::SIDE_STAGED,
            ),
        };
        let conflicted = c
            .files
            .iter()
            .any(|f| f.path == path && f.unstaged == Some(Change::Conflicted));
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("{path} {side_label}")).strong());
            let has_lines = !c.selected_lines.is_empty();
            if !conflicted
                && ui
                    .add(
                        Button95::new(verb_lines)
                            .min_size(egui::vec2(140.0, 20.0))
                            .enabled(has_lines),
                    )
                    .clicked()
            {
                action = Some(Action::Lines);
            }
        });
        ui.separator();
        if conflicted {
            ui.label(s::RESOLVE_CONFLICTS);
            return;
        }
        let Some(diff) = c.diff.as_ref() else { return };
        if diff.binary {
            ui.label(s::BINARY_FILE);
            if ui.add(Button95::new(verb_file)).clicked() {
                action = Some(Action::File);
            }
            return;
        }
        if diff.hunks.is_empty() {
            ui.label(s::NO_DIFF);
            return;
        }
        if diff.line_count() > LARGE_DIFF_LINES && !c.show_large {
            ui.label(s::DIFF_TOO_LARGE);
            if ui.add(Button95::new(s::SHOW_ANYWAY)).clicked() {
                action = Some(Action::ShowLarge);
            }
            return;
        }
        let all_rows = rows(diff);
        let mono = egui::FontId::monospace(win95::theme::FONT_SIZE);
        ScrollArea::both().auto_shrink([false, false]).show_rows(
            ui,
            ROW_HEIGHT,
            all_rows.len(),
            |ui, range| {
                ui.spacing_mut().item_spacing.y = 0.0;
                for row in &all_rows[range] {
                    match *row {
                        Row::Hunk(h) => {
                            egui::Frame::NONE.fill(HUNK_BG).show(ui, |ui| {
                                ui.set_min_width(ui.available_width());
                                ui.horizontal(|ui| {
                                    ui.set_height(ROW_HEIGHT);
                                    ui.label(
                                        RichText::new(&diff.hunks[h].header)
                                            .font(mono.clone())
                                            .color(win95::theme::NAVY),
                                    );
                                    if ui
                                        .add(
                                            Button95::new(verb_hunk)
                                                .min_size(egui::vec2(90.0, 16.0)),
                                        )
                                        .clicked()
                                    {
                                        action = Some(Action::Hunk(h));
                                    }
                                });
                            });
                        }
                        Row::Line(h, l) => {
                            let line = &diff.hunks[h].lines[l];
                            let (bg, sign) = match line.kind {
                                LineKind::Added => (ADDED_BG, "+"),
                                LineKind::Removed => (REMOVED_BG, "-"),
                                LineKind::Context => (win95::theme::WHITE, " "),
                            };
                            egui::Frame::NONE.fill(bg).show(ui, |ui| {
                                ui.set_min_width(ui.available_width());
                                ui.horizontal(|ui| {
                                    ui.set_height(ROW_HEIGHT);
                                    if line.kind == LineKind::Context {
                                        ui.add_space(13.0);
                                    } else {
                                        let mut on = c.selected_lines.contains(&(h, l));
                                        if checkbox(ui, &mut on, "").changed() {
                                            if on {
                                                c.selected_lines.insert((h, l));
                                            } else {
                                                c.selected_lines.remove(&(h, l));
                                            }
                                        }
                                    }
                                    let num = |n: Option<u32>| {
                                        n.map(|v| format!("{v:>5}"))
                                            .unwrap_or_else(|| "     ".into())
                                    };
                                    let text = line.text.trim_end_matches(['\n', '\r']);
                                    let eof = if line.no_newline_at_eof {
                                        "  ⏎̸"
                                    } else {
                                        ""
                                    };
                                    ui.label(
                                        RichText::new(format!(
                                            "{} {} {sign} {text}{eof}",
                                            num(line.old_no),
                                            num(line.new_no)
                                        ))
                                        .font(mono.clone())
                                        .color(win95::theme::BLACK),
                                    );
                                });
                            });
                        }
                    }
                }
            },
        );
    });

    let Some((path, side)) = cx.state.changes.shown.clone() else {
        return;
    };
    let shown = cx.state.changes.diff.clone();
    let selection = match action {
        None => return,
        Some(Action::ShowLarge) => {
            cx.state.changes.show_large = true;
            return;
        }
        Some(Action::Lines) => {
            Selection::Lines(cx.state.changes.selected_lines.iter().copied().collect())
        }
        Some(Action::Hunk(h)) => Selection::Hunks(vec![h]),
        Some(Action::File) => Selection::All,
    };
    let cmd = match side {
        Side::Unstaged => Command::Stage {
            path,
            selection,
            shown,
        },
        Side::Staged => Command::Unstage {
            path,
            selection,
            shown,
        },
    };
    cx.worker.send(cmd);
}
```

Rows are virtualised with `ScrollArea::show_rows`; each click only records an `Action`, and the command is sent after drawing, so the UI never mutates state while iterating the diff.

- [ ] **Step 7: Show the Changes screen in the main window**

Replace the whole file with the version below (it keeps everything from sub-project 1). The S1 `summary` panel is removed (its information moves to the Changes header); the Repository menu gains Commit…, Stage all, Unstage all; the status bar shows "Committing..." / "Committed <id>".

`crates/app/src/ui/main_window.rs`:

```rust
use egui::{Panel, UiBuilder, ViewportCommand};
use win95::{
    Bevel, Button95, Cell, Column, ListView, TitleAction, TitleBar, bevel_frame, status_bar,
};

use super::{Ctx, clone_dialog};
use crate::protocol::Command;
use crate::state::Auth;
use crate::strings as s;

pub fn show(ui: &mut egui::Ui, cx: &mut Ctx<'_>) {
    let full = ui.max_rect();
    ui.painter().rect_filled(full, 0.0, win95::theme::SILVER);
    win95::bevel::paint(ui.painter(), full, Bevel::Window);
    let inner = full.shrink(3.0);
    ui.scope_builder(UiBuilder::new().max_rect(inner), |ui| {
        Panel::top("chrome")
            .frame(egui::Frame::NONE)
            .show(ui, |ui| {
                title(ui, cx);
                menu(ui, cx);
                toolbar(ui, cx);
                ui.add_space(2.0);
            });
        Panel::bottom("status")
            .frame(egui::Frame::NONE)
            .show(ui, |ui| status(ui, cx));
        Panel::left("recents")
            .frame(egui::Frame::NONE.inner_margin(egui::Margin::same(2)))
            .resizable(true)
            .default_size(200.0)
            .show(ui, |ui| recents(ui, cx));
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.inner_margin(egui::Margin::same(2)))
            .show(ui, |ui| {
                if cx.state.current.is_some() {
                    super::changes::show(ui, cx);
                } else {
                    bevel_frame(ui, Bevel::Field, win95::theme::WHITE, 8, |ui| {
                        ui.set_min_size(ui.available_size());
                        ui.label(s::NO_REPO);
                    });
                }
            });
    });
    win95::resize_edges(ui, full);
}

fn title(ui: &mut egui::Ui, cx: &mut Ctx<'_>) {
    let text = match &cx.state.current {
        Some(c) => format!("{} - {}", s::APP_NAME, c.name),
        None => s::APP_NAME.to_string(),
    };
    let focused = ui.input(|i| i.viewport().focused.unwrap_or(true));
    let maximized = ui.input(|i| i.viewport().maximized.unwrap_or(false));
    // Drawn above dialogs so the window can still be moved/minimized/closed while one is open.
    let action = win95::above_dialogs(ui, "main_title", win95::title_bar::HEIGHT, |ui| {
        TitleBar::new(&text).active(focused).show(ui)
    });
    match action {
        TitleAction::StartDrag => ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag),
        TitleAction::Minimize => ui.ctx().send_viewport_cmd(ViewportCommand::Minimized(true)),
        TitleAction::ToggleMaximize => ui
            .ctx()
            .send_viewport_cmd(ViewportCommand::Maximized(!maximized)),
        TitleAction::Close => ui.ctx().send_viewport_cmd(ViewportCommand::Close),
        TitleAction::None => {}
    }
}

fn open_folder(cx: &mut Ctx<'_>) {
    if let Some(folder) = rfd::FileDialog::new().pick_folder() {
        cx.worker.send(Command::OpenRepo(folder));
    }
}

fn menu(ui: &mut egui::Ui, cx: &mut Ctx<'_>) {
    egui::MenuBar::new().ui(ui, |ui| {
        ui.menu_button(s::MENU_FILE, |ui| {
            if matches!(cx.state.auth, Auth::SignedIn(_)) {
                if ui.button(s::SIGN_OUT).clicked() {
                    cx.worker.send(Command::SignOut);
                }
            } else if ui.button(s::SIGN_IN_MENU).clicked() {
                if cx.state.auth == Auth::Offline {
                    cx.state.auth = Auth::Checking;
                    cx.worker.send(Command::ValidateToken);
                } else {
                    cx.state.sign_in.get_or_insert_with(Default::default);
                }
            }
            ui.separator();
            if ui.button(s::CLONE_MENU).clicked() {
                clone_dialog::open(cx);
            }
            if ui.button(s::OPEN_MENU).clicked() {
                open_folder(cx);
            }
            ui.separator();
            if ui.button(s::EXIT).clicked() {
                ui.ctx().send_viewport_cmd(ViewportCommand::Close);
            }
        });
        ui.menu_button(s::MENU_REPOSITORY, |ui| {
            let open = cx.state.current.is_some();
            if ui
                .add_enabled(open, egui::Button::new(s::COMMIT_MENU))
                .clicked()
            {
                cx.state.changes.focus_summary = true;
            }
            if ui
                .add_enabled(open, egui::Button::new(s::STAGE_ALL))
                .clicked()
            {
                let files: Vec<_> = cx.state.changes.unstaged().cloned().collect();
                for f in files
                    .iter()
                    .filter(|f| f.unstaged != Some(gitcore::Change::Conflicted))
                {
                    for path in super::changes::paths_of(f, gitcore::Side::Unstaged) {
                        cx.worker.send(Command::Stage {
                            path,
                            selection: gitcore::Selection::All,
                            shown: None,
                        });
                    }
                }
            }
            if ui
                .add_enabled(open, egui::Button::new(s::UNSTAGE_ALL))
                .clicked()
            {
                let files: Vec<_> = cx.state.changes.staged().cloned().collect();
                for f in &files {
                    for path in super::changes::paths_of(f, gitcore::Side::Staged) {
                        cx.worker.send(Command::Unstage {
                            path,
                            selection: gitcore::Selection::All,
                            shown: None,
                        });
                    }
                }
            }
            ui.separator();
            for label in [s::FETCH, s::PULL, s::PUSH] {
                ui.add_enabled(false, egui::Button::new(label));
            }
        });
        ui.menu_button(s::MENU_VIEW, |ui| {
            let current = cx.state.current.as_ref().map(|c| c.path.clone());
            if ui
                .add_enabled(current.is_some(), egui::Button::new(s::REFRESH_REPO))
                .clicked()
                && let Some(path) = current
            {
                cx.worker.send(Command::OpenRepo(path));
            }
        });
        ui.menu_button(s::MENU_HELP, |ui| {
            if ui.button(s::ABOUT_MENU).clicked() {
                cx.state.about = true;
            }
        });
    });
}

fn toolbar(ui: &mut egui::Ui, cx: &mut Ctx<'_>) {
    let size = egui::vec2(60.0, 22.0);
    ui.horizontal(|ui| {
        if ui.add(Button95::new(s::CLONE).min_size(size)).clicked() {
            clone_dialog::open(cx);
        }
        if ui.add(Button95::new(s::OPEN).min_size(size)).clicked() {
            open_folder(cx);
        }
        ui.separator();
        for label in [s::FETCH, s::PULL, s::PUSH] {
            ui.add(Button95::new(label).min_size(size).enabled(false));
        }
    });
}

const RECENT_COLUMNS: &[Column] = &[Column {
    title: s::REPOSITORIES,
    width: 400.0,
}];

fn recents(ui: &mut egui::Ui, cx: &mut Ctx<'_>) {
    ui.label(s::REPOSITORIES);
    let recent = cx.state.config.recent.clone();
    let selected = cx
        .state
        .current
        .as_ref()
        .and_then(|c| recent.iter().position(|r| r.path == c.path));
    let missing = &cx.state.missing;
    let mut remove: Option<usize> = None;
    let height = ui.available_height();
    let resp = ListView::new("recents", RECENT_COLUMNS, recent.len())
        .header(false)
        .height(height)
        .context_menu(|row, ui| {
            if ui.button(s::REMOVE_FROM_LIST).clicked() {
                remove = Some(row);
            }
        })
        .show(ui, selected, |row, _| Cell {
            text: recent[row].name.clone(),
            dimmed: missing.contains(&recent[row].path),
        });
    if let Some(row) = resp.clicked {
        cx.worker.send(Command::OpenRepo(recent[row].path.clone()));
    }
    if let Some(row) = remove {
        cx.state.remove_recent(&recent[row].path);
    }
}

fn status(ui: &mut egui::Ui, cx: &mut Ctx<'_>) {
    let who = match &cx.state.auth {
        Auth::SignedIn(u) => format!("Signed in: @{}", u.login),
        Auth::Checking => s::CHECKING.to_string(),
        Auth::Offline => s::OFFLINE.to_string(),
        _ => s::NOT_SIGNED_IN.to_string(),
    };
    let activity = if cx.state.repos_loading {
        s::LOADING
    } else if cx.state.changes.committing {
        s::COMMITTING
    } else if let Some(note) = cx.state.changes.last_commit_note.as_deref() {
        note
    } else {
        s::READY
    };
    status_bar(ui, &[(&who, Some(260.0)), (activity, None)]);
}
```

- [ ] **Step 8: Watch the open repository and refresh on focus**

Replace the whole file with the version below (it keeps everything from sub-project 1). A failed watcher start is remembered for that path (logged once, not retried every frame).

`crates/app/src/app.rs`:

```rust
//! The eframe application: drains worker events, draws screens, persists config.

use std::path::PathBuf;

use crate::config::WindowGeometry;
use crate::protocol::Command;
use crate::state::AppState;
use crate::ui::{self, Ctx};
use crate::watch::Watcher;
use crate::worker::WorkerHandle;

pub struct RetroGitApp {
    state: AppState,
    worker: WorkerHandle,
    config_path: Option<PathBuf>,
    geometry: Option<WindowGeometry>,
    /// Watches the open repository; replaced when another one is opened.
    /// `None` inside means watching failed for that path (not retried every frame).
    watcher: Option<(PathBuf, Option<Watcher>)>,
    was_focused: bool,
}

impl RetroGitApp {
    pub fn new(state: AppState, worker: WorkerHandle, config_path: Option<PathBuf>) -> RetroGitApp {
        worker.send(Command::ValidateToken);
        RetroGitApp {
            state,
            worker,
            config_path,
            geometry: None,
            watcher: None,
            was_focused: true,
        }
    }

    /// Keep the file watcher on the current repository.
    fn sync_watcher(&mut self) {
        let current = self.state.current.as_ref().map(|c| c.path.clone());
        if self.watcher.as_ref().map(|(p, _)| p) == current.as_ref() {
            return;
        }
        self.watcher = current.map(|path| {
            let w = match Watcher::start(&path, self.worker.refresher()) {
                Ok(w) => Some(w),
                Err(e) => {
                    log::warn!(
                        "cannot watch {}: {e}; refresh on focus and with Refresh",
                        path.display()
                    );
                    None
                }
            };
            (path, w)
        });
    }

    fn save_config(&mut self) {
        if let Some(g) = self.geometry {
            self.state.config.window = Some(g);
        }
        if let Some(path) = &self.config_path
            && let Err(e) = self.state.config.save_to(path)
        {
            log::warn!("could not save config: {e}");
        }
        self.state.config_dirty = false;
    }
}

impl eframe::App for RetroGitApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        while let Ok(ev) = self.worker.events.try_recv() {
            self.state.apply(ev);
        }
        if self.state.config_dirty {
            self.save_config();
        }
        self.sync_watcher();
        let focused = ctx.input(|i| i.viewport().focused.unwrap_or(true));
        if focused && !self.was_focused && self.state.current.is_some() {
            self.worker.send(Command::RefreshStatus);
        }
        self.was_focused = focused;
        ctx.input(|i| {
            let vp = i.viewport();
            if vp.maximized != Some(true)
                && let Some(inner) = vp.inner_rect
            {
                let outer = vp.outer_rect;
                self.geometry = Some(WindowGeometry {
                    width: inner.width(),
                    height: inner.height(),
                    x: outer.map(|r| r.left()),
                    y: outer.map(|r| r.top()),
                });
            }
        });
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let egui_ctx = ui.ctx().clone();
        let mut cx = Ctx {
            state: &mut self.state,
            worker: &self.worker,
        };
        ui::main_window::show(ui, &mut cx);
        ui::clone_dialog::show(&egui_ctx, &mut cx);
        ui::sign_in::show(&egui_ctx, &mut cx);
        ui::about::show(&egui_ctx, &mut cx);
        ui::message::show(&egui_ctx, &mut cx);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if !self.worker.shutdown(std::time::Duration::from_secs(10)) {
            log::warn!("worker still busy at exit");
        }
        self.save_config();
    }
}
```

- [ ] **Step 9: Run the tests to see them pass**

Run: `cargo test -p retrogit --lib`

Expected: all lib tests pass, including `describes_changes_with_win95_style_codes`, `renames_touch_both_paths`, `extension_patterns`, `rows_interleave_hunk_headers_and_lines`.

- [ ] **Step 10: Lint**

Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings`

Expected: no output from fmt; clippy finishes without warnings.

- [ ] **Step 11: Full suite**

Run: `cargo test --workspace`

Expected: 148 passed, 0 failed.

- [ ] **Step 12: Commit**

```bash
git add crates/app
git commit -m "feat(app): Changes screen with line-level staging and commit form"
```


### Task 7: Release check and manual verification

**Files:**
- No code changes expected (fix and commit anything the checks reveal).

**Interfaces:**
- Consumes: the finished feature.
- Produces: a verified build.

- [ ] **Step 1: Release build**

```bash
cargo build --release -p retrogit
ls -lh target/release/retrogit
otool -L target/release/retrogit
```

Expected: under 15 MB; only `/System/Library/...` and `/usr/lib/...` libraries.

- [ ] **Step 2: Manual checks (user)**

Run `cargo run --release -p retrogit`, open a real repository, then check:
1. Edit two files in your editor: both appear under **Changes** within ~1 s, without clicking Refresh.
2. Click a file: its diff shows with line numbers and green/red lines, in a fixed-width font.
3. Tick two `+` lines → **Stage selected lines**: the file now appears in both groups; `git diff --cached` in a terminal shows exactly those lines.
4. **Stage hunk**, **Unstage hunk**, double-click a file, Space on the displayed file, **Stage all** / **Unstage all**, right-click → **Add to .gitignore** and **Add \*.ext to .gitignore**.
5. Commit with Summary + Description: `git log -1` shows both, separated by a blank line; with commit signing configured, `git log --show-signature -1` shows the signature.
6. In a repo with a failing `pre-commit` hook (e.g. husky/lint), launched **from the Dock**: the hook runs with your usual PATH, and its output appears in the error box; the message is kept.
7. Tick **Amend last commit**: the last message is pre-filled; on a pushed branch the red warning appears.
8. A binary file shows "Binary file" and only whole-file staging.
9. Idle CPU stays at ~0 % with a repository open.

- [ ] **Step 3: Windows check (when available)**

On Windows x86_64: `cargo test --workspace`, then repeat checks 1–5 in a repository with `core.autocrlf=true`: no line-ending-only diffs, and no console window flashes on commit.
