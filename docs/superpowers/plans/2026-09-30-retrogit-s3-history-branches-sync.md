# RetroGit S3 — History, Branches, Sync Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A History tab with a commit graph and commit details, branch management (create, switch with optional stash, rename, delete, check out remote, publish), and fetch / pull / push through the installed `git` — with the RetroGit token handed to git for github.com HTTPS remotes, no password prompt ever blocking, and no unsigned commit when signing is required.

**Architecture:** `gitcore` gains read-only history/branches over libgit2 (`log`, pure `graph::layout`, `commit_detail`, `branches`) and git-CLI operations for everything that writes (`branch`, `stash`, `ops`, `remote`), built on a small `cli` helper; `net` builds the environment (`GIT_TERMINAL_PROMPT=0`, SSH BatchMode, `GIT_ASKPASS` = the RetroGit binary itself). `win95` gains `combo_box` and `splitter`. The app adds reducer state (`state/sync.rs`), worker handlers (`worker/sync.rs`), an `--askpass` mode and a panic hook in `main`, and the UI (tabs, History, toolbar, dialogs, merge/rebase banner, signing indicator).

**Tech Stack:** Existing stack; no new dependency. `git` CLI required at runtime for writes and network (verified with git 2.50).

**Spec:** `docs/superpowers/specs/2026-09-30-retrogit-s3-history-branches-sync-design.md`

> Every code block was compiled, formatted, linted (`clippy -D warnings`) and tested on macOS arm64 with Rust 1.98.1 before this plan was written (220 tests), then the plan itself was replayed task by task on a clean checkout of `main`. Copy code verbatim; if an `Expected:` differs, stop and investigate.

## Global Constraints

- Work on branch `feat/s3-history-sync` created from `main` in `~/perso/retrogit`.
- Targets: `aarch64-apple-darwin`, `x86_64-pc-windows-msvc`. No new dependency; release binary under 15 MB (13 MB when written).
- `win95`, `gitcore`, `github` never depend on each other; no `git2` type in `gitcore`'s public API.
- Every git CLI call runs without a terminal: `GIT_TERMINAL_PROMPT=0`, `GIT_EDITOR=true`, stdin closed; network calls also get `GIT_SSH_COMMAND="ssh -o BatchMode=yes"` unless the user configured `GIT_SSH_COMMAND`/`core.sshCommand`.
- The token only goes to `git` for remotes starting with `https://github.com/`, via `GIT_ASKPASS=<RetroGit executable>` + `RETROGIT_ASKPASS_TOKEN` in that child's environment, with `-c credential.helper=`; never on the command line, never on disk.
- Never create an unsigned commit when `commit.gpgsign=true`: the libgit2 fallback returns `GitError::SigningRequiresGit`.
- All user-visible strings in `crates/app/src/strings.rs` (English).
- One worker thread for all Git work; network operations are cancellable (`WorkerHandle::cancel_network`, also triggered by `shutdown`).
- Tests needing `git` return early when it is absent; test repos set identity, `commit.gpgsign=false`, `core.hooksPath=.git/hooks`, `core.autocrlf=false` locally.
- Every task ends with fmt + clippy `-D warnings` clean and `cargo test --workspace` green.
- Ruled deviations from the spec (decided while verifying the code):
  - `CommitDetail.files` is `Vec<ChangedFile { path, change }>` (not `FileStatus`): a commit has one change per file.
  - Branch-name validation in the UI is the pure `state::branch_name_error` (Git's `check-ref-format` rules + existing names), so typing never spawns a process; `Repo::validate_branch_name` (CLI) stays available in `gitcore`.
  - Worker events are finer-grained than the spec's sketch: `SyncStarted/SyncProgress/SyncFinished`, `Pulled`, `Diverged`, `PushRejected`, `WouldOverwrite`, `NotMerged`, `OperationChanged`, `SigningLoaded`, `LogLoaded`, `CommitLoaded`, `SignatureLoaded`, `CommitFileDiffLoaded`, `BranchesLoaded`.
  - Pull = `fetch` then `merge --ff-only @{u}` / `merge --no-edit @{u}` / `rebase @{u}` (instead of `git pull`), so divergence is detected before anything changes.
  - Branch rename/delete live in the Repository menu (Rename acts on the current branch; Delete opens a dialog listing the other local branches), not in a right-click menu inside the branch drop-down.
  - The background fetch at open is skipped for `https://github.com/` remotes when not signed in (it would fail anyway) and its errors are only logged.

## Review Focus

1. **HTTPS remote on github.com with a stale password in the macOS Keychain**: fetch/push must use the RetroGit token, not the stale helper credential. Pinned by `github_https_gets_the_token_through_askpass` (Task 3) and `askpass_mode_answers_username_then_token` (Task 5); final check is manual (Task 7).
2. **SSH remote whose key needs a passphrase (no agent)**: the operation must fail fast with help text instead of hanging. Pinned by `ssh_and_other_hosts_never_see_the_token` (BatchMode, Task 3) and `classifies_network_failures` (Task 3).
3. **Pull when both sides changed**: nothing changes until the user picks Merge or Rebase; conflicts leave an abortable operation. Pinned by `pull_fast_forward_divergence_merge_and_rebase`, `pull_conflicts_leave_an_operation_that_can_be_aborted` (Task 3) and `diverged_pull_asks_then_rebase_succeeds` (Task 5).
4. **Switching branch with local edits** (conflicting or not, untracked included): edits follow when possible; otherwise stash → switch → pop, with the stash kept on conflict. Pinned by `switching_with_conflicting_changes_reports_files_then_stash_switch_pop_works` (Task 2) and `rejected_push_and_switch_with_stash` (Task 5).
5. **Signed-commit policy (ExampleOrg)**: RetroGit must never create an unsigned commit when signing is on, and must show whether the next commit will be signed. Pinned by `unsigned_fallback_is_refused_when_signing_is_required` (Task 2) and `signing_label_says_whether_commits_are_signed` (Task 6); GPG pinentry from the Dock is a manual check (Task 7).

---


### Task 1: `gitcore` history: log, graph layout, commit detail

**Files:**
- Create: `crates/gitcore/src/cli.rs`, `crates/gitcore/src/log.rs`, `crates/gitcore/src/graph.rs`, `crates/gitcore/src/commit_detail.rs`, `crates/gitcore/tests/common/remote.rs`
- Modify: `crates/gitcore/src/error.rs`, `crates/gitcore/src/diff.rs` (extract `file_diff_from`), `crates/gitcore/src/lib.rs`, `crates/gitcore/tests/common/mod.rs`, `crates/app/src/strings.rs`, `crates/app/src/protocol.rs`
- Test: `crates/gitcore/tests/history.rs` (+ unit tests in `graph.rs`)

**Interfaces:**
- Consumes: S1/S2 `Repo`, `GitError`, `FileDiff`, `Change`, `git_available`, `git_command` (crate-private).
- Produces:
  - `GitError` new variants: `WouldOverwrite { files: Vec<String> }`, `NotMerged(String)`, `Diverged { ahead: usize, behind: usize }`, `PushRejected`, `StashConflict`, `SigningRequiresGit`, `GitMissing`.
  - crate-private `Repo::run_git(&self, &[&str]) -> Result<GitOutput, GitError>` / `git_ok(..)` (`GitOutput { success, stdout, text }`); `diff::file_diff_from(&git2::Diff, path, side)`.
  - `LogEntry { id, short_id, parents: Vec<String>, author, email, time: i64, summary, refs: Vec<RefLabel> }`, `RefLabel { name, kind: RefKind::{Head, LocalBranch, RemoteBranch, Tag} }`, `Repo::log(skip, limit) -> Result<Vec<LogEntry>, GitError>`.
  - `graph::{layout(&[LogEntry]) -> Vec<GraphRow>, GraphState::layout_more(&mut self, &[LogEntry]) -> Vec<GraphRow>, GraphRow { column, color, up: Vec<Edge>, down: Vec<Edge> } + width(), Edge { from, to, color }}`.
  - `CommitDetail { id, short_id, parents, author, email, time, committer, message, files: Vec<ChangedFile> }`, `ChangedFile { path, change: Change }`, `Repo::commit_detail(id)`, `Repo::commit_file_diff(id, path) -> FileDiff`, `SignatureStatus::{Good { signer }, Bad, Unknown, Unsigned}`, `parse_signature_status(&str)`, `Repo::signature_status(id)`.
  - Test helper `common::remote::{Env, git, configure, no_cancel}` (bare `origin.git` + tracking clone `work`).
  - `strings.rs`: every S3 string; `AppError::from_git` maps the new variants.

- [ ] **Step 1: Create the branch**

Run: `git checkout -b feat/s3-history-sync`

Expected: `Switched to a new branch 'feat/s3-history-sync'`.

- [ ] **Step 2: Add the remote test helper**

`Env::new()` returns `None` when `git` is not installed, so every test using it starts with `let Some(env) = Env::new() else { return };`.

`crates/gitcore/tests/common/remote.rs`:

```rust
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]
//! A bare "origin" remote and clones of it, driven with the git command line.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::AtomicBool;

use gitcore::{CommitBackend, Repo, Selection, git_available};

pub fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

pub fn configure(dir: &Path) {
    for (k, v) in [
        ("user.name", "Ada"),
        ("user.email", "ada@example.com"),
        ("commit.gpgsign", "false"),
        ("core.hooksPath", ".git/hooks"),
        ("core.autocrlf", "false"),
        ("pull.rebase", "false"),
    ] {
        git(dir, &["config", k, v]);
    }
}

/// `origin.git` (bare) with 2 commits on main, and a clone `work` tracking it.
pub struct Env {
    pub _tmp: tempfile::TempDir,
    pub root: PathBuf,
    pub work: PathBuf,
}

impl Env {
    pub fn new() -> Option<Env> {
        if !git_available() {
            return None;
        }
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let seed = root.join("seed");
        super::make_repo(&seed, 2);
        git(
            &root,
            &[
                "clone",
                "-q",
                "--bare",
                seed.to_str().unwrap(),
                "origin.git",
            ],
        );
        let work = root.join("work");
        git(&root, &["clone", "-q", "origin.git", "work"]);
        configure(&work);
        Some(Env {
            _tmp: tmp,
            root,
            work,
        })
    }

    pub fn repo(&self) -> Repo {
        Repo::open(&self.work).unwrap()
    }

    /// Another clone that pushes `file` to origin/main.
    pub fn remote_commit(&self, file: &str, content: &str) {
        let other = self.root.join(format!("other-{file}"));
        git(
            &self.root,
            &["clone", "-q", "origin.git", other.to_str().unwrap()],
        );
        configure(&other);
        std::fs::write(other.join(file), content).unwrap();
        git(&other, &["add", file]);
        git(&other, &["commit", "-q", "-m", &format!("remote {file}")]);
        git(&other, &["push", "-q"]);
    }

    pub fn local_commit(&self, file: &str, content: &str) {
        std::fs::write(self.work.join(file), content).unwrap();
        let r = self.repo();
        r.stage(file, &Selection::All, None).unwrap();
        r.commit(&format!("local {file}"), false, CommitBackend::PreferCli)
            .unwrap();
    }
}

pub fn no_cancel() -> AtomicBool {
    AtomicBool::new(false)
}
```

- [ ] **Step 3: Declare it**

Replace the whole file with the version below (it keeps everything from sub-projects 1 and 2). Only change: `pub mod remote;`.

`crates/gitcore/tests/common/mod.rs`:

```rust
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;

pub mod remote;

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

- [ ] **Step 4: Write the failing history tests**

`crates/gitcore/tests/history.rs`:

```rust
#![allow(clippy::unwrap_used)]
//! History and commit details (needs `git`).
mod common;

use common::remote::{Env, git};
use gitcore::{RefKind, SignatureStatus};

#[test]
fn log_lists_all_branches_with_ref_labels() {
    let Some(env) = Env::new() else { return };
    git(&env.work, &["switch", "-q", "-c", "feature"]);
    env.local_commit("f.txt", "f\n");
    git(&env.work, &["switch", "-q", "main"]);
    let log = env.repo().log(0, 100).unwrap();
    assert_eq!(log.len(), 3);
    assert_eq!(log[0].summary, "local f.txt");
    assert!(
        log[0]
            .refs
            .iter()
            .any(|r| r.name == "feature" && r.kind == RefKind::LocalBranch)
    );
    let main = log
        .iter()
        .find(|e| e.refs.iter().any(|r| r.name == "main"))
        .unwrap();
    assert!(main.refs.iter().any(|r| r.kind == RefKind::Head));
    assert!(
        main.refs
            .iter()
            .any(|r| r.name == "origin/main" && r.kind == RefKind::RemoteBranch)
    );
    assert_eq!(env.repo().log(1, 100).unwrap().len(), 2);
    assert_eq!(env.repo().log(0, 1).unwrap().len(), 1);
}

#[test]
fn commit_detail_files_diff_and_signature() {
    let Some(env) = Env::new() else { return };
    env.local_commit("new.txt", "hello\n");
    let r = env.repo();
    let head = r.log(0, 1).unwrap().remove(0);
    let d = r.commit_detail(&head.id).unwrap();
    assert_eq!(d.message, "local new.txt");
    assert_eq!(d.files.len(), 1);
    assert_eq!(d.files[0].path, "new.txt");
    assert_eq!(d.files[0].change, gitcore::Change::Added);
    let diff = r.commit_file_diff(&head.id, "new.txt").unwrap();
    assert_eq!(diff.hunks[0].lines[0].text, "hello\n");
    assert_eq!(
        r.signature_status(&head.id).unwrap(),
        SignatureStatus::Unsigned
    );
    // Root commit: everything added.
    let root = r.log(0, 100).unwrap().pop().unwrap();
    assert!(
        r.commit_detail(&root.id)
            .unwrap()
            .files
            .iter()
            .all(|f| f.change == gitcore::Change::Added)
    );
}
```

- [ ] **Step 5: Write the failing tests in `crates/gitcore/src/graph.rs`**

Create `crates/gitcore/src/graph.rs` containing only this test module for now (the implementation goes above it in a later step).

`crates/gitcore/src/graph.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn e(id: &str, parents: &[&str]) -> LogEntry {
        LogEntry {
            id: id.into(),
            short_id: id.into(),
            parents: parents.iter().map(|p| p.to_string()).collect(),
            author: String::new(),
            email: String::new(),
            time: 0,
            summary: String::new(),
            refs: vec![],
        }
    }

    fn cols(rows: &[GraphRow]) -> Vec<usize> {
        rows.iter().map(|r| r.column).collect()
    }

    #[test]
    fn linear_history_stays_in_column_zero() {
        let rows = layout(&[e("c", &["b"]), e("b", &["a"]), e("a", &[])]);
        assert_eq!(cols(&rows), vec![0, 0, 0]);
        assert_eq!(rows[0].up, vec![]);
        assert_eq!(
            rows[0].down,
            vec![Edge {
                from: 0,
                to: 0,
                color: 0
            }]
        );
        assert_eq!(rows[2].down, vec![], "root: no line below");
        assert!(rows.iter().all(|r| r.color == 0));
    }

    #[test]
    fn merge_opens_a_second_lane_that_joins_back() {
        // m merges x (feature) into b; x and b both come from a.
        let rows = layout(&[
            e("m", &["b", "x"]),
            e("x", &["a"]),
            e("b", &["a"]),
            e("a", &[]),
        ]);
        assert_eq!(cols(&rows), vec![0, 1, 0, 0]);
        assert_eq!(
            rows[0].down,
            vec![
                Edge {
                    from: 0,
                    to: 0,
                    color: 0
                },
                Edge {
                    from: 0,
                    to: 1,
                    color: 1
                }
            ]
        );
        // x keeps lane 1 for its parent a; b continues lane 0 towards a too.
        assert_eq!(
            rows[1].down,
            vec![
                Edge {
                    from: 0,
                    to: 0,
                    color: 0
                },
                Edge {
                    from: 1,
                    to: 1,
                    color: 1
                }
            ]
        );
        // at a, lane 1 joins lane 0.
        assert_eq!(
            rows[3].up,
            vec![
                Edge {
                    from: 0,
                    to: 0,
                    color: 0
                },
                Edge {
                    from: 1,
                    to: 0,
                    color: 1
                }
            ]
        );
        assert_eq!(rows[3].down, vec![]);
    }

    #[test]
    fn parallel_branch_tips_get_their_own_lanes() {
        let rows = layout(&[e("t1", &["a"]), e("t2", &["a"]), e("a", &[])]);
        assert_eq!(cols(&rows), vec![0, 1, 0]);
        assert_ne!(rows[0].color, rows[1].color);
        assert_eq!(rows[2].up.len(), 2);
    }

    #[test]
    fn octopus_merge_and_multiple_roots() {
        let rows = layout(&[
            e("m", &["a", "b", "c"]),
            e("a", &[]),
            e("b", &[]),
            e("c", &[]),
        ]);
        assert_eq!(rows[0].down.len(), 3);
        assert_eq!(cols(&rows), vec![0, 0, 1, 2]);
        assert!(rows.iter().all(|r| r.width() <= 3));
    }

    #[test]
    fn freed_lanes_are_reused() {
        // x merged and gone, then y branches later and reuses column 1.
        let rows = layout(&[
            e("m2", &["m1", "y"]),
            e("y", &["m1"]),
            e("m1", &["b", "x"]),
            e("x", &["b"]),
            e("b", &[]),
        ]);
        assert_eq!(cols(&rows), vec![0, 1, 0, 1, 0]);
        assert!(rows.iter().all(|r| r.width() <= 2));
    }

    #[test]
    fn incremental_layout_matches_one_shot_layout() {
        let entries = vec![
            e("m2", &["m1", "y"]),
            e("y", &["m1"]),
            e("m1", &["b", "x"]),
            e("x", &["b"]),
            e("b", &["a"]),
            e("a", &[]),
        ];
        let mut state = GraphState::default();
        let mut paged = state.layout_more(&entries[..3]);
        paged.extend(state.layout_more(&entries[3..]));
        assert_eq!(paged, layout(&entries));
    }
}
```

- [ ] **Step 6: Declare the modules (Task 1 version)**

Replace the whole file with the version below (it keeps everything from sub-projects 1 and 2).

`crates/gitcore/src/lib.rs`:

```rust
//! RetroGit's internal Git API. No `git2` type is ever exposed publicly.

mod cli;
mod clone;
mod commit;
mod commit_detail;
mod diff;
mod discard;
mod error;
mod graph;
mod ignore;
mod log;
mod repo;
mod stage;
mod status;

pub use commit_detail::{ChangedFile, CommitDetail, SignatureStatus, parse_signature_status};
pub use graph::{Edge, GraphRow, GraphState, layout};
pub use log::{LogEntry, RefKind, RefLabel};

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

- [ ] **Step 7: Run the tests to see them fail**

Run: `cargo test -p gitcore`

Expected: compile errors: `file not found for module cli`, `commit_detail`, `log`; `layout` not found.

- [ ] **Step 8: Add the new error variants**

Replace the whole file with the version below (it keeps everything from sub-projects 1 and 2).

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
    /// Switching branch would overwrite these locally modified files.
    #[error("local changes would be overwritten")]
    WouldOverwrite { files: Vec<String> },
    #[error("branch '{0}' is not fully merged")]
    NotMerged(String),
    #[error("the branch and its upstream have diverged")]
    Diverged { ahead: usize, behind: usize },
    /// The remote has commits the local branch does not have (non fast-forward).
    #[error("push rejected: the remote has new commits")]
    PushRejected,
    /// Re-applying stashed changes created conflicts; the stash was kept.
    #[error("re-applying your stashed changes caused conflicts")]
    StashConflict,
    /// Commit signing is on but only libgit2 is available: refusing to commit unsigned.
    #[error("commit signing requires the git command line")]
    SigningRequiresGit,
    #[error("this feature needs the git command line")]
    GitMissing,
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

- [ ] **Step 9: Add the git CLI helper**

`crates/gitcore/src/cli.rs`:

```rust
//! Running the `git` command line inside a repository.

use std::process::Stdio;

use crate::{GitError, Repo};

/// stdout + stderr of a finished `git` process.
pub(crate) struct GitOutput {
    pub success: bool,
    pub stdout: String,
    pub text: String,
}

impl Repo {
    /// Run `git -C <workdir> <args>` without a terminal and return its output.
    pub(crate) fn run_git(&self, args: &[&str]) -> Result<GitOutput, GitError> {
        if !crate::git_available() {
            return Err(GitError::GitMissing);
        }
        let out = crate::commit::git_command()
            .arg("-C")
            .arg(self.workdir()?)
            .args(args)
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_EDITOR", "true")
            .stdin(Stdio::null())
            .output()
            .map_err(|e| GitError::Other(format!("cannot run git: {e}")))?;
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        let text = format!("{stdout}{}", String::from_utf8_lossy(&out.stderr))
            .trim()
            .to_string();
        Ok(GitOutput {
            success: out.status.success(),
            stdout,
            text,
        })
    }

    /// Like `run_git`, but a failure becomes `GitError::Other(output)`.
    pub(crate) fn git_ok(&self, args: &[&str]) -> Result<GitOutput, GitError> {
        let out = self.run_git(args)?;
        if out.success {
            Ok(out)
        } else {
            Err(GitError::Other(out.text))
        }
    }
}
```

- [ ] **Step 10: Make the diff conversion reusable**

Replace the whole file with the version below (it keeps everything from sub-projects 1 and 2). `diff_file` now ends with `file_diff_from(&diff, path, side)`; the conversion loop moved unchanged into that crate-private function (used by `commit_file_diff`).

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
    /// Line content for display (lossy UTF-8), including its line ending when it has one.
    pub text: String,
    /// Exact bytes of the line (what staging writes: never lossy).
    pub raw: Vec<u8>,
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
        file_diff_from(&diff, path, side)
    }

    pub(crate) fn workdir(&self) -> Result<&Path, GitError> {
        self.git()
            .workdir()
            .ok_or_else(|| GitError::Unsupported("bare repositories are not supported".into()))
    }
}

/// Convert a libgit2 diff (limited to one file) into our model.
pub(crate) fn file_diff_from(
    diff: &git2::Diff<'_>,
    path: &str,
    side: Side,
) -> Result<FileDiff, GitError> {
    let map = |e: git2::Error| GitError::from_git2(&e);
    let mut out = FileDiff {
        path: path.to_string(),
        side,
        binary: false,
        hunks: Vec::new(),
    };
    for idx in 0..diff.deltas().len() {
        let Some(patch) = git2::Patch::from_diff(diff, idx).map_err(map)? else {
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
                let raw = line.content().to_vec();
                lines.push(DiffLine {
                    kind,
                    old_no: line.old_lineno(),
                    new_no: line.new_lineno(),
                    no_newline_at_eof: raw.last() != Some(&b'\n'),
                    text: String::from_utf8_lossy(&raw).into_owned(),
                    raw,
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
```

- [ ] **Step 11: Implement the history walk**

Topological + time order over HEAD, `refs/heads/*` and `refs/remotes/*`; `origin/HEAD` symbolic refs are skipped.

`crates/gitcore/src/log.rs`:

```rust
//! Commit history across all branches.

use std::collections::HashMap;

use crate::{GitError, Repo};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefKind {
    Head,
    LocalBranch,
    RemoteBranch,
    Tag,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefLabel {
    pub name: String,
    pub kind: RefKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogEntry {
    pub id: String,
    pub short_id: String,
    pub parents: Vec<String>,
    pub author: String,
    pub email: String,
    /// Author time, seconds since the Unix epoch.
    pub time: i64,
    pub summary: String,
    pub refs: Vec<RefLabel>,
}

impl Repo {
    /// `limit` commits after skipping `skip`, from every local and remote branch and HEAD,
    /// in topological then time order (children before parents).
    pub fn log(&self, skip: usize, limit: usize) -> Result<Vec<LogEntry>, GitError> {
        let repo = self.git();
        let map = |e: git2::Error| GitError::from_git2(&e);
        let mut walk = repo.revwalk().map_err(map)?;
        walk.set_sorting(git2::Sort::TOPOLOGICAL | git2::Sort::TIME)
            .map_err(map)?;
        if repo.head().is_err() {
            return Ok(Vec::new()); // no commit yet
        }
        walk.push_head().map_err(map)?;
        walk.push_glob("refs/heads").map_err(map)?;
        walk.push_glob("refs/remotes").map_err(map)?;
        let labels = self.ref_labels()?;
        let mut out = Vec::with_capacity(limit);
        for oid in walk.skip(skip).take(limit) {
            let oid = oid.map_err(map)?;
            let c = repo.find_commit(oid).map_err(map)?;
            let id = oid.to_string();
            out.push(LogEntry {
                short_id: id.chars().take(7).collect(),
                parents: c.parent_ids().map(|p| p.to_string()).collect(),
                author: c.author().name().unwrap_or("").to_string(),
                email: c.author().email().unwrap_or("").to_string(),
                time: c.author().when().seconds(),
                summary: c.summary().ok().flatten().unwrap_or("").to_string(),
                refs: labels.get(&id).cloned().unwrap_or_default(),
                id,
            });
        }
        Ok(out)
    }

    /// Commit id -> refs pointing at it (HEAD first, then branches, then tags).
    fn ref_labels(&self) -> Result<HashMap<String, Vec<RefLabel>>, GitError> {
        let repo = self.git();
        let mut labels: HashMap<String, Vec<RefLabel>> = HashMap::new();
        if let Ok(head) = repo.head()
            && let Ok(c) = head.peel_to_commit()
        {
            labels
                .entry(c.id().to_string())
                .or_default()
                .push(RefLabel {
                    name: "HEAD".into(),
                    kind: RefKind::Head,
                });
        }
        let refs = repo.references().map_err(|e| GitError::from_git2(&e))?;
        for r in refs.flatten() {
            let Ok(full) = r.name() else { continue };
            let (kind, name) = if let Some(n) = full.strip_prefix("refs/heads/") {
                (RefKind::LocalBranch, n)
            } else if let Some(n) = full.strip_prefix("refs/remotes/") {
                if n.ends_with("/HEAD") {
                    continue;
                }
                (RefKind::RemoteBranch, n)
            } else if let Some(n) = full.strip_prefix("refs/tags/") {
                (RefKind::Tag, n)
            } else {
                continue;
            };
            let Ok(c) = r.peel_to_commit() else { continue };
            labels
                .entry(c.id().to_string())
                .or_default()
                .push(RefLabel {
                    name: name.to_string(),
                    kind,
                });
        }
        for v in labels.values_mut() {
            v.sort_by_key(|l| (l.kind as u8, l.name.clone()));
        }
        Ok(labels)
    }
}
```

- [ ] **Step 12: Implement the lane layout**

Insert this **above** the existing `#[cfg(test)]` module in `crates/gitcore/src/graph.rs`.

Each row describes its top half (`up`: lanes from the row above to the dot's height, merging lanes bend into the dot) and bottom half (`down`: from the dot to the lanes below; the first parent keeps the column, other parents join an existing lane or open a free one). `GraphState` carries lanes between pages so paged layout equals one-shot layout.

`crates/gitcore/src/graph.rs`:

```rust
//! Lane layout for the commit graph (pure: no I/O).

use crate::LogEntry;

/// One segment drawn inside a row: from column `from` to column `to`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Edge {
    pub from: usize,
    pub to: usize,
    pub color: usize,
}

/// Drawing instructions for one commit row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphRow {
    /// Column of the commit dot.
    pub column: usize,
    pub color: usize,
    /// Top half: from the lanes above (`from`) to their position at the dot's height (`to`).
    pub up: Vec<Edge>,
    /// Bottom half: from the dot's height (`from`) to the lanes below (`to`).
    pub down: Vec<Edge>,
}

impl GraphRow {
    /// Number of columns this row touches.
    pub fn width(&self) -> usize {
        self.up
            .iter()
            .chain(&self.down)
            .map(|e| e.from.max(e.to) + 1)
            .chain([self.column + 1])
            .max()
            .unwrap_or(1)
    }
}

/// Lanes carried from one page of history to the next.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GraphState {
    /// For each column: the commit id this lane is waiting for, and its color.
    lanes: Vec<Option<(String, usize)>>,
    next_color: usize,
}

fn free_slot(lanes: &mut Vec<Option<(String, usize)>>, avoid: usize) -> usize {
    match lanes
        .iter()
        .enumerate()
        .position(|(i, l)| l.is_none() && i != avoid)
    {
        Some(i) => i,
        None => {
            lanes.push(None);
            lanes.len() - 1
        }
    }
}

impl GraphState {
    fn new_color(&mut self) -> usize {
        let c = self.next_color;
        self.next_color += 1;
        c
    }

    /// Lay out the next entries, continuing from the previous call.
    pub fn layout_more(&mut self, entries: &[LogEntry]) -> Vec<GraphRow> {
        entries.iter().map(|e| self.row(e)).collect()
    }

    fn row(&mut self, entry: &LogEntry) -> GraphRow {
        let before = self.lanes.clone();
        let waiting =
            |l: &Option<(String, usize)>| l.as_ref().is_some_and(|(id, _)| *id == entry.id);
        let found = before.iter().position(waiting);
        let (column, color) = match found {
            Some(c) => (c, before[c].as_ref().map_or(0, |l| l.1)),
            None => {
                let c = before
                    .iter()
                    .position(Option::is_none)
                    .unwrap_or(before.len());
                (c, self.new_color())
            }
        };
        let up = before
            .iter()
            .enumerate()
            .filter_map(|(i, l)| l.as_ref().map(|(id, col)| (i, id, *col)))
            .map(|(i, id, c)| Edge {
                from: i,
                to: if *id == entry.id { column } else { i },
                color: c,
            })
            .collect();

        // Lanes that were waiting for this commit end here.
        for l in self.lanes.iter_mut() {
            if waiting(l) {
                *l = None;
            }
        }
        if self.lanes.len() <= column {
            self.lanes.resize(column + 1, None);
        }
        let mut targets = Vec::new();
        for (k, parent) in entry.parents.iter().enumerate() {
            if k == 0 {
                self.lanes[column] = Some((parent.clone(), color));
                targets.push(column);
                continue;
            }
            let existing = self
                .lanes
                .iter()
                .position(|l| l.as_ref().is_some_and(|(id, _)| id == parent));
            let t = match existing {
                Some(t) => t,
                None => {
                    let t = free_slot(&mut self.lanes, column);
                    let c = self.new_color();
                    self.lanes[t] = Some((parent.clone(), c));
                    t
                }
            };
            targets.push(t);
        }
        while self.lanes.last().is_some_and(Option::is_none) {
            self.lanes.pop();
        }
        let down = self
            .lanes
            .iter()
            .enumerate()
            .filter_map(|(i, l)| l.as_ref().map(|(_, c)| (i, *c)))
            .map(|(i, c)| Edge {
                from: if targets.contains(&i) { column } else { i },
                to: i,
                color: c,
            })
            .collect();
        GraphRow {
            column,
            color,
            up,
            down,
        }
    }
}

/// Lay out a whole list at once (same result as successive `layout_more` calls).
pub fn layout(entries: &[LogEntry]) -> Vec<GraphRow> {
    GraphState::default().layout_more(entries)
}
```

- [ ] **Step 13: Implement commit details**

Renames are detected (`find_similar`) for the file list; `signature_status` runs `git log -1 --format=%G?%n%GS` (it invokes gpg, so it is only called for the selected commit).

`crates/gitcore/src/commit_detail.rs`:

```rust
//! One commit: message, files and per-file diffs; signature status.

use crate::diff::{FileDiff, Side, file_diff_from};
use crate::status::Change;
use crate::{GitError, LogEntry, Repo};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangedFile {
    pub path: String,
    pub change: Change,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitDetail {
    pub id: String,
    pub short_id: String,
    pub parents: Vec<String>,
    pub author: String,
    pub email: String,
    pub time: i64,
    pub committer: String,
    /// Full message (summary, blank line, body).
    pub message: String,
    /// Changes against the first parent (everything is "added" for a root commit).
    pub files: Vec<ChangedFile>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignatureStatus {
    Good {
        signer: String,
    },
    Bad,
    /// Signed, but the signature could not be checked (missing key, expired...).
    Unknown,
    Unsigned,
}

/// Map `git log --format=%G?%n%GS` output to a status.
pub fn parse_signature_status(output: &str) -> SignatureStatus {
    let mut lines = output.lines();
    let code = lines.next().unwrap_or("").trim();
    let signer = lines.next().unwrap_or("").trim().to_string();
    match code {
        "G" | "U" => SignatureStatus::Good { signer },
        "B" | "R" => SignatureStatus::Bad,
        "N" | "" => SignatureStatus::Unsigned,
        _ => SignatureStatus::Unknown,
    }
}

impl Repo {
    fn find_commit(&self, id: &str) -> Result<git2::Commit<'_>, GitError> {
        let map = |e: git2::Error| GitError::from_git2(&e);
        let oid = git2::Oid::from_str(id).map_err(map)?;
        self.git().find_commit(oid).map_err(map)
    }

    fn commit_diff(
        &self,
        c: &git2::Commit<'_>,
        path: Option<&str>,
    ) -> Result<git2::Diff<'_>, GitError> {
        let map = |e: git2::Error| GitError::from_git2(&e);
        let tree = c.tree().map_err(map)?;
        let parent_tree = match c.parent(0) {
            Ok(p) => Some(p.tree().map_err(map)?),
            Err(_) => None,
        };
        let mut opts = git2::DiffOptions::new();
        if let Some(p) = path {
            opts.pathspec(p).disable_pathspec_match(true);
        }
        let mut diff = self
            .git()
            .diff_tree_to_tree(parent_tree.as_ref(), Some(&tree), Some(&mut opts))
            .map_err(map)?;
        if path.is_none() {
            diff.find_similar(None).map_err(map)?;
        }
        Ok(diff)
    }

    pub fn commit_detail(&self, id: &str) -> Result<CommitDetail, GitError> {
        let c = self.find_commit(id)?;
        let diff = self.commit_diff(&c, None)?;
        let files = diff
            .deltas()
            .filter_map(|d| {
                let path = d.new_file().path().or_else(|| d.old_file().path())?;
                let path = path.to_string_lossy().replace('\\', "/");
                let change = match d.status() {
                    git2::Delta::Added => Change::Added,
                    git2::Delta::Deleted => Change::Deleted,
                    git2::Delta::Renamed => Change::Renamed {
                        from: d
                            .old_file()
                            .path()
                            .map(|p| p.to_string_lossy().replace('\\', "/"))
                            .unwrap_or_default(),
                    },
                    git2::Delta::Typechange => Change::TypeChange,
                    _ => Change::Modified,
                };
                Some(ChangedFile { path, change })
            })
            .collect();
        let full = c.id().to_string();
        Ok(CommitDetail {
            short_id: full.chars().take(7).collect(),
            id: full,
            parents: c.parent_ids().map(|p| p.to_string()).collect(),
            author: c.author().name().unwrap_or("").to_string(),
            email: c.author().email().unwrap_or("").to_string(),
            time: c.author().when().seconds(),
            committer: c.committer().name().unwrap_or("").to_string(),
            message: c.message().ok().unwrap_or("").trim_end().to_string(),
            files,
        })
    }

    /// Diff of one file in a commit, against its first parent.
    pub fn commit_file_diff(&self, id: &str, path: &str) -> Result<FileDiff, GitError> {
        let c = self.find_commit(id)?;
        let diff = self.commit_diff(&c, Some(path))?;
        file_diff_from(&diff, path, Side::Staged)
    }

    /// Signature status of a commit (runs gpg/ssh through `git`: call for one commit at a time).
    pub fn signature_status(&self, id: &str) -> Result<SignatureStatus, GitError> {
        let out = self.git_ok(&["log", "-1", "--format=%G?%n%GS", id])?;
        Ok(parse_signature_status(&out.stdout))
    }
}

impl From<&CommitDetail> for LogEntry {
    fn from(d: &CommitDetail) -> LogEntry {
        LogEntry {
            id: d.id.clone(),
            short_id: d.short_id.clone(),
            parents: d.parents.clone(),
            author: d.author.clone(),
            email: d.email.clone(),
            time: d.time,
            summary: d.message.lines().next().unwrap_or("").to_string(),
            refs: vec![],
        }
    }
}
```

- [ ] **Step 14: Add every S3 string**

Replace the whole file with the version below (it keeps everything from sub-projects 1 and 2).

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
pub const DISCARD_ALL: &str = "Discard all";
pub const DISCARD_MENU: &str = "Discard changes...";
pub const DISCARD_HUNK: &str = "Discard hunk";
pub const DISCARD_LINES: &str = "Discard selected lines";
pub const DISCARD: &str = "Discard";
pub const DISCARD_TITLE: &str = "Discard changes";
pub const DISCARD_WARNING: &str = "This cannot be undone.";
pub const DISCARD_UNTRACKED_NOTE: &str = "Untracked files are moved to the trash.";
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
pub const TAB_CHANGES: &str = "Changes";
pub const TAB_HISTORY: &str = "History";
pub const BRANCH_LABEL: &str = "Branch:";
pub const NEW_BRANCH: &str = "New branch";
pub const NEW_BRANCH_MENU: &str = "New branch...";
pub const RENAME_BRANCH_MENU: &str = "Rename branch...";
pub const DELETE_BRANCH_MENU: &str = "Delete branch...";
pub const PUBLISH: &str = "Publish";
pub const REMOTE_BRANCHES: &str = "Remote branches";
pub const DETACHED_AT: &str = "(detached)";
pub const NEW_BRANCH_TITLE: &str = "New branch";
pub const RENAME_BRANCH_TITLE: &str = "Rename branch";
pub const DELETE_BRANCH_TITLE: &str = "Delete branch";
pub const BRANCH_NAME: &str = "Branch name:";
pub const SWITCH_TO_IT: &str = "Switch to it";
pub const DELETE: &str = "Delete";
pub const DELETE_ANYWAY: &str = "Delete anyway";
pub const CANNOT_DELETE_CURRENT: &str = "You are on this branch. Switch to another branch first.";
pub const NOT_MERGED_QUESTION: &str = "This branch has commits that are not merged anywhere. Delete it anyway? Those commits may be lost.";
pub const DIVERGED_TITLE: &str = "Pull";
pub const MERGE: &str = "Merge";
pub const REBASE: &str = "Rebase";
pub const PUSH_REJECTED_TITLE: &str = "Push rejected";
pub const FORCE_PUSH: &str = "Force push (with lease)...";
pub const FORCE_PUSH_CONFIRM: &str = "Force push will replace the commits on the remote branch with yours (only if nobody pushed since your last fetch). Continue?";
pub const FORCE: &str = "Force push";
pub const SWITCH_TITLE: &str = "Switch branch";
pub const STASH_SWITCH: &str = "Stash, switch and re-apply";
pub const CANCELLED: &str = "Cancelled.";
pub const FETCHING: &str = "Fetching...";
pub const PULLING: &str = "Pulling...";
pub const PUSHING: &str = "Pushing...";
pub const UP_TO_DATE: &str = "Already up to date";
pub const PULLED: &str = "Pulled";
pub const PUSHED: &str = "Pushed";
pub const FETCHED: &str = "Fetched";
pub const MERGE_IN_PROGRESS: &str = "Merge in progress: resolve conflicts, stage, then commit.";
pub const REBASE_IN_PROGRESS: &str = "Rebase in progress: resolve conflicts, stage, then continue.";
pub const ABORT_MERGE: &str = "Abort merge";
pub const ABORT_REBASE: &str = "Abort rebase";
pub const CONTINUE_REBASE: &str = "Continue rebase";
pub const SIGNED_WITH: &str = "Signed with";
pub const NOT_SIGNED: &str = "Commits will NOT be signed";
pub const LOADING_HISTORY: &str = "Loading history...";
pub const NO_HISTORY: &str = "No commits yet.";
pub const SELECT_A_COMMIT: &str = "Select a commit to see its details.";
pub const SIG_GOOD: &str = "Good signature";
pub const SIG_BAD: &str = "BAD signature";
pub const SIG_UNKNOWN: &str = "Signature cannot be checked";
pub const SIG_UNSIGNED: &str = "Not signed";
pub const SIG_CHECKING: &str = "Checking signature...";
pub const FILES: &str = "Files:";
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
pub const ERR_WOULD_OVERWRITE: &str =
    "Your local changes to these files would be overwritten by switching branch:";
pub const ERR_PUSH_REJECTED: &str =
    "The remote has changes you don't have. Pull first, then push again.";
pub const ERR_STASH_CONFLICT: &str = "Re-applying your changes caused conflicts. Resolve them in the Changes tab; your changes are also kept in the stash.";
pub const ERR_SIGNING_REQUIRES_GIT: &str = "Commit signing is enabled but git was not found: RetroGit will not create an unsigned commit. Install Git.";
pub const ERR_GIT_MISSING: &str = "Install Git to use this feature.";
pub const ERR_NET_AUTH_HELP: &str = "Git could not authenticate. For SSH remotes, add your key to ssh-agent (ssh-add). For HTTPS remotes outside github.com, configure a credential helper.";
pub const INFO_CONFLICTS: &str = "There are conflicts. Resolve them in your editor, stage the files, then commit (or continue the rebase).";
pub const INFO_CLONE_CANCELLED: &str = "Clone cancelled.";
```

- [ ] **Step 15: Map the new errors**

Replace the whole file with the version below (it keeps everything from sub-projects 1 and 2). Only change: new arms in `AppError::from_git`.

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
    /// Revert unstaged working-tree changes (needs a confirmation in the UI).
    Discard {
        path: String,
        selection: Selection,
        shown: Option<FileDiff>,
    },
    /// Whole files back to their index version; untracked files go to the trash.
    DiscardFiles(Vec<String>),
    /// Whole files, one index operation and one refresh (Stage all / Unstage all).
    StageFiles(Vec<String>),
    UnstageFiles(Vec<String>),
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
            GitError::WouldOverwrite { files } => {
                AppError::new(Severity::Warning, s::ERR_WOULD_OVERWRITE)
                    .with_detail(files.join("\n"))
            }
            GitError::NotMerged(b) => AppError::new(
                Severity::Warning,
                &format!("Branch '{b}' is not fully merged."),
            ),
            GitError::Diverged { ahead, behind } => AppError::new(
                Severity::Info,
                &format!("Your branch and its upstream have diverged (↑{ahead} ↓{behind})."),
            ),
            GitError::PushRejected => AppError::new(Severity::Warning, s::ERR_PUSH_REJECTED),
            GitError::StashConflict => AppError::new(Severity::Warning, s::ERR_STASH_CONFLICT),
            GitError::SigningRequiresGit => {
                AppError::new(Severity::Error, s::ERR_SIGNING_REQUIRES_GIT)
            }
            GitError::GitMissing => AppError::new(Severity::Warning, s::ERR_GIT_MISSING),
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

- [ ] **Step 16: Run the tests to see them pass**

Run: `cargo test -p gitcore`

Expected: unit tests include 6 graph tests; `tests/history.rs` 2 passed; S1/S2 suites still pass.

- [ ] **Step 17: Lint**

Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings`

Expected: no output from fmt; clippy finishes without warnings.

- [ ] **Step 18: Full suite**

Run: `cargo test --workspace`

Expected: all tests pass (0 failed).

- [ ] **Step 19: Commit**

```bash
git add crates/gitcore crates/app/src/strings.rs crates/app/src/protocol.rs
git commit -m "feat(gitcore): history walk, commit graph layout and commit details"
```


### Task 2: `gitcore` branches, stash, merge/rebase state, signing policy

**Files:**
- Create: `crates/gitcore/src/branch.rs`, `crates/gitcore/src/stash.rs`, `crates/gitcore/src/ops.rs`, `crates/gitcore/src/signing.rs`
- Modify: `crates/gitcore/src/lib.rs`, `crates/gitcore/src/commit.rs`
- Test: `crates/gitcore/tests/branches.rs`

**Interfaces:**
- Consumes: `run_git`, `git_ok`, `classify_commit_failure`, `GitError` (Task 1).
- Produces: `Branch { name, remote, is_head, upstream: Option<String>, ahead, behind }`; `Repo::{branches, current_branch, validate_branch_name, create_branch(name, switch), switch_branch(name), checkout_remote_branch(remote_name), rename_branch(old, new), delete_branch(name, force)}`; `parse_overwritten_files(&str) -> Vec<String>`; `Repo::{stash_push(message) -> Result<bool, _>, stash_pop()}`; `Operation::{Merge, Rebase}`, `Repo::{operation_in_progress, abort_operation, continue_rebase}`; `SigningConfig { enabled, format: SigningFormat::{Gpg, Ssh, X509}, key: Option<String> }`, `Repo::signing_config()`; `Repo::commit` returns `SigningRequiresGit` instead of committing unsigned through libgit2.

- [ ] **Step 1: Write the failing tests**

`crates/gitcore/tests/branches.rs`:

```rust
#![allow(clippy::unwrap_used)]
//! Branches, stash and signing policy (needs `git`).
mod common;

use common::remote::{Env, git};
use gitcore::{CommitBackend, GitError, Selection};

#[test]
fn branches_create_switch_rename_delete() {
    let Some(env) = Env::new() else { return };
    let r = env.repo();
    let b = r.branches().unwrap();
    assert!(
        b.iter()
            .any(|b| b.name == "main" && b.is_head && b.upstream.as_deref() == Some("origin/main"))
    );
    assert!(b.iter().any(|b| b.name == "origin/main" && b.remote));
    assert!(b.iter().all(|b| b.name != "origin/HEAD"));

    assert!(r.validate_branch_name("bad name").is_err());
    assert!(r.validate_branch_name("main").is_err());
    assert!(r.validate_branch_name("feat/ok").is_ok());
    r.create_branch("feat/ok", true).unwrap();
    assert_eq!(r.current_branch().unwrap().name, "feat/ok");
    env.local_commit("x.txt", "x\n");
    r.switch_branch("main").unwrap();
    r.rename_branch("feat/ok", "feat/renamed").unwrap();
    assert_eq!(
        r.delete_branch("feat/renamed", false),
        Err(GitError::NotMerged("feat/renamed".into()))
    );
    r.delete_branch("feat/renamed", true).unwrap();
    assert!(
        r.branches()
            .unwrap()
            .iter()
            .all(|b| b.name != "feat/renamed")
    );
}

#[test]
fn switching_with_conflicting_changes_reports_files_then_stash_switch_pop_works() {
    let Some(env) = Env::new() else { return };
    let r = env.repo();
    r.create_branch("other", true).unwrap();
    env.local_commit("file0.txt", "changed on other\n");
    r.switch_branch("main").unwrap();
    std::fs::write(env.work.join("file0.txt"), "local edit\n").unwrap();
    match r.switch_branch("other") {
        Err(GitError::WouldOverwrite { files }) => assert_eq!(files, vec!["file0.txt".to_string()]),
        other => panic!("{other:?}"),
    }
    // Non-conflicting edits simply follow.
    std::fs::write(env.work.join("file0.txt"), "content 0\n".repeat(50)).unwrap();
    std::fs::write(env.work.join("file1.txt"), "follows me\n").unwrap();
    r.switch_branch("other").unwrap();
    assert_eq!(
        std::fs::read_to_string(env.work.join("file1.txt")).unwrap(),
        "follows me\n"
    );
    r.switch_branch("main").unwrap();

    std::fs::write(env.work.join("file0.txt"), "local edit\n").unwrap();
    assert!(r.stash_push("RetroGit: switch to other").unwrap());
    r.switch_branch("other").unwrap();
    assert_eq!(r.stash_pop(), Err(GitError::StashConflict));
    assert!(
        !git(&env.work, &["stash", "list"]).is_empty(),
        "the stash is kept on conflict"
    );
}

#[test]
fn stash_push_reports_when_there_is_nothing() {
    let Some(env) = Env::new() else { return };
    assert!(!env.repo().stash_push("nothing").unwrap());
    std::fs::write(env.work.join("u.txt"), "untracked\n").unwrap();
    let r = env.repo();
    assert!(r.stash_push("with untracked").unwrap());
    assert!(!env.work.join("u.txt").exists());
    r.stash_pop().unwrap();
    assert!(env.work.join("u.txt").exists());
}

#[test]
fn unsigned_fallback_is_refused_when_signing_is_required() {
    let Some(env) = Env::new() else { return };
    git(&env.work, &["config", "commit.gpgsign", "true"]);
    git(&env.work, &["config", "user.signingkey", "ABCDEF"]);
    let r = env.repo();
    let s = r.signing_config().unwrap();
    assert!(s.enabled);
    assert_eq!(s.key.as_deref(), Some("ABCDEF"));
    assert_eq!(s.format, gitcore::SigningFormat::Gpg);
    std::fs::write(env.work.join("s.txt"), "s\n").unwrap();
    r.stage("s.txt", &Selection::All, None).unwrap();
    assert_eq!(
        r.commit("x", false, CommitBackend::Git2),
        Err(GitError::SigningRequiresGit)
    );
}
```

- [ ] **Step 2: Declare the modules (Task 2 version)**

Replace the whole file with the version below (it keeps everything from sub-projects 1 and 2).

`crates/gitcore/src/lib.rs`:

```rust
//! RetroGit's internal Git API. No `git2` type is ever exposed publicly.

mod branch;
mod cli;
mod clone;
mod commit;
mod commit_detail;
mod diff;
mod discard;
mod error;
mod graph;
mod ignore;
mod log;
mod ops;
mod repo;
mod signing;
mod stage;
mod stash;
mod status;

pub use branch::{Branch, parse_overwritten_files};
pub use commit_detail::{ChangedFile, CommitDetail, SignatureStatus, parse_signature_status};
pub use graph::{Edge, GraphRow, GraphState, layout};
pub use log::{LogEntry, RefKind, RefLabel};
pub use ops::Operation;
pub use signing::{SigningConfig, SigningFormat};

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

Run: `cargo test -p gitcore --test branches`

Expected: compile errors: modules `branch`, `ops`, `signing`, `stash` not found.

- [ ] **Step 4: Implement branches**

Listing uses libgit2 (fast, ahead/behind via `graph_ahead_behind`); every change goes through `git switch` / `git branch` so hooks and config apply. "would be overwritten by checkout" errors are parsed into `WouldOverwrite { files }`.

`crates/gitcore/src/branch.rs`:

```rust
//! Branches: listing with libgit2, changes through the git command line.

use crate::{GitError, Repo};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Branch {
    /// `main` for a local branch, `origin/main` for a remote one.
    pub name: String,
    pub remote: bool,
    pub is_head: bool,
    /// Upstream of a local branch, e.g. `origin/main`.
    pub upstream: Option<String>,
    /// Commits on the branch that its upstream lacks.
    pub ahead: usize,
    /// Commits on the upstream that the branch lacks.
    pub behind: usize,
}

/// Files listed by git in "... would be overwritten by checkout:" errors.
pub fn parse_overwritten_files(output: &str) -> Vec<String> {
    let mut files = Vec::new();
    let mut in_list = false;
    for line in output.lines() {
        if line.contains("would be overwritten by") {
            in_list = true;
            continue;
        }
        if in_list {
            if let Some(f) = line.strip_prefix('\t') {
                files.push(f.trim().to_string());
            } else {
                in_list = false;
            }
        }
    }
    files
}

impl Repo {
    /// Local branches (alphabetical), then remote branches (alphabetical).
    pub fn branches(&self) -> Result<Vec<Branch>, GitError> {
        let repo = self.git();
        let map = |e: git2::Error| GitError::from_git2(&e);
        let mut out = Vec::new();
        for item in repo.branches(None).map_err(map)? {
            let (b, kind) = item.map_err(map)?;
            let Ok(Some(name)) = b.name().map(|n| n.map(str::to_string)) else {
                continue;
            };
            let remote = kind == git2::BranchType::Remote;
            if remote && name.ends_with("/HEAD") {
                continue;
            }
            let (mut upstream, mut ahead, mut behind) = (None, 0, 0);
            if !remote && let Ok(up) = b.upstream() {
                upstream = up.name().ok().flatten().map(str::to_string);
                if let (Some(l), Some(u)) = (b.get().target(), up.get().target()) {
                    (ahead, behind) = repo.graph_ahead_behind(l, u).unwrap_or((0, 0));
                }
            }
            out.push(Branch {
                is_head: b.is_head(),
                name,
                remote,
                upstream,
                ahead,
                behind,
            });
        }
        out.sort_by(|a, b| {
            (a.remote, a.name.to_lowercase()).cmp(&(b.remote, b.name.to_lowercase()))
        });
        Ok(out)
    }

    /// Current local branch, if HEAD is not detached.
    pub fn current_branch(&self) -> Option<Branch> {
        self.branches()
            .ok()?
            .into_iter()
            .find(|b| b.is_head && !b.remote)
    }

    /// `Err(reason)` if `name` is not a valid new branch name.
    pub fn validate_branch_name(&self, name: &str) -> Result<(), String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("Enter a branch name.".into());
        }
        if self
            .branches()
            .unwrap_or_default()
            .iter()
            .any(|b| !b.remote && b.name == name)
        {
            return Err(format!("A branch named '{name}' already exists."));
        }
        match self.run_git(&["check-ref-format", "--branch", name]) {
            Ok(out) if out.success => Ok(()),
            Ok(_) => Err(format!("'{name}' is not a valid branch name.")),
            Err(_) => {
                let bad = name.contains(' ')
                    || name.contains("..")
                    || name.starts_with('-')
                    || name.ends_with('/');
                if bad {
                    Err(format!("'{name}' is not a valid branch name."))
                } else {
                    Ok(())
                }
            }
        }
    }

    pub fn create_branch(&self, name: &str, switch: bool) -> Result<(), GitError> {
        let args: &[&str] = if switch {
            &["switch", "-c", name]
        } else {
            &["branch", name]
        };
        self.git_ok(args).map(|_| ())
    }

    /// `git switch <name>`; `WouldOverwrite` if local changes are in the way.
    pub fn switch_branch(&self, name: &str) -> Result<(), GitError> {
        let out = self.run_git(&["switch", name])?;
        if out.success {
            return Ok(());
        }
        let files = parse_overwritten_files(&out.text);
        if files.is_empty() {
            Err(GitError::Other(out.text))
        } else {
            Err(GitError::WouldOverwrite { files })
        }
    }

    /// Create the local branch tracking `remote_name` (e.g. `origin/feature`) and switch to it.
    pub fn checkout_remote_branch(&self, remote_name: &str) -> Result<(), GitError> {
        let out = self.run_git(&["switch", "--track", remote_name])?;
        if out.success {
            return Ok(());
        }
        let files = parse_overwritten_files(&out.text);
        if files.is_empty() {
            Err(GitError::Other(out.text))
        } else {
            Err(GitError::WouldOverwrite { files })
        }
    }

    pub fn rename_branch(&self, old: &str, new: &str) -> Result<(), GitError> {
        self.git_ok(&["branch", "-m", old, new]).map(|_| ())
    }

    /// Delete a local branch; `NotMerged` unless `force` when it has unmerged commits.
    pub fn delete_branch(&self, name: &str, force: bool) -> Result<(), GitError> {
        let out = self.run_git(&["branch", if force { "-D" } else { "-d" }, name])?;
        if out.success {
            Ok(())
        } else if out.text.contains("not fully merged") {
            Err(GitError::NotMerged(name.to_string()))
        } else {
            Err(GitError::Other(out.text))
        }
    }
}
```

- [ ] **Step 5: Implement stash**

`crates/gitcore/src/stash.rs`:

```rust
use crate::{GitError, Repo};

impl Repo {
    /// Put local changes (untracked files included) aside. `Ok(false)` if there was nothing.
    pub fn stash_push(&self, message: &str) -> Result<bool, GitError> {
        let out = self.git_ok(&["stash", "push", "--include-untracked", "-m", message])?;
        Ok(!out.text.contains("No local changes to save"))
    }

    /// Re-apply the last stash. On conflicts the stash is kept and `StashConflict` returned.
    pub fn stash_pop(&self) -> Result<(), GitError> {
        let out = self.run_git(&["stash", "pop"])?;
        if out.success {
            Ok(())
        } else if out.text.contains("CONFLICT") || out.text.contains("conflict") {
            Err(GitError::StashConflict)
        } else {
            Err(GitError::Other(out.text))
        }
    }
}
```

- [ ] **Step 6: Implement merge/rebase state**

`crates/gitcore/src/ops.rs`:

```rust
use crate::{GitError, Repo};

/// A multi-step operation left in progress (conflicts to resolve).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operation {
    Merge,
    Rebase,
}

impl Repo {
    pub fn operation_in_progress(&self) -> Option<Operation> {
        use git2::RepositoryState as S;
        match self.git().state() {
            S::Merge => Some(Operation::Merge),
            S::Rebase | S::RebaseInteractive | S::RebaseMerge => Some(Operation::Rebase),
            _ => None,
        }
    }

    pub fn abort_operation(&self) -> Result<(), GitError> {
        match self.operation_in_progress() {
            Some(Operation::Merge) => self.git_ok(&["merge", "--abort"]).map(|_| ()),
            Some(Operation::Rebase) => self.git_ok(&["rebase", "--abort"]).map(|_| ()),
            None => Ok(()),
        }
    }

    /// Continue a rebase once conflicts are resolved and staged.
    pub fn continue_rebase(&self) -> Result<(), GitError> {
        let out = self.run_git(&["-c", "core.editor=true", "rebase", "--continue"])?;
        if out.success {
            Ok(())
        } else {
            Err(crate::classify_commit_failure(&out.text))
        }
    }
}
```

- [ ] **Step 7: Implement the signing configuration**

`crates/gitcore/src/signing.rs`:

```rust
use crate::{GitError, Repo};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SigningFormat {
    Gpg,
    Ssh,
    X509,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SigningConfig {
    /// `commit.gpgsign`: commits made with the git CLI will be signed.
    pub enabled: bool,
    pub format: SigningFormat,
    /// `user.signingkey`, if set.
    pub key: Option<String>,
}

impl Repo {
    /// Effective signing configuration (system, global, `includeIf` and repository config).
    pub fn signing_config(&self) -> Result<SigningConfig, GitError> {
        let cfg = self
            .git()
            .config()
            .and_then(|mut c| c.snapshot())
            .map_err(|e| GitError::from_git2(&e))?;
        let format = match cfg.get_string("gpg.format").ok().as_deref() {
            Some("ssh") => SigningFormat::Ssh,
            Some("x509") => SigningFormat::X509,
            _ => SigningFormat::Gpg,
        };
        Ok(SigningConfig {
            enabled: cfg.get_bool("commit.gpgsign").unwrap_or(false),
            format,
            key: cfg
                .get_string("user.signingkey")
                .ok()
                .filter(|k| !k.is_empty()),
        })
    }
}
```

- [ ] **Step 8: Refuse unsigned libgit2 commits when signing is on**

Replace the whole file with the version below (it keeps everything from sub-projects 1 and 2). Only change: the `else if self.signing_config().is_ok_and(|s| s.enabled)` branch in `commit`.

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

pub(crate) fn git_command() -> Command {
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
        } else if self.signing_config().is_ok_and(|s| s.enabled) {
            // libgit2 cannot sign: never create an unsigned commit when signing is required.
            return Err(GitError::SigningRequiresGit);
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
            .args(["commit", "--cleanup=whitespace", "-F", "-"]);
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

- [ ] **Step 9: Run the tests to see them pass**

Run: `cargo test -p gitcore --test branches`

Expected: `4 passed`.

- [ ] **Step 10: Lint**

Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings`

Expected: no output from fmt; clippy finishes without warnings.

- [ ] **Step 11: Full suite**

Run: `cargo test --workspace`

Expected: all tests pass (0 failed).

- [ ] **Step 12: Commit**

```bash
git add crates/gitcore
git commit -m "feat(gitcore): branches, stash, merge/rebase state and signing policy"
```


### Task 3: `gitcore` network: askpass environment, fetch, pull, push

**Files:**
- Create: `crates/gitcore/src/net.rs`, `crates/gitcore/src/remote.rs`
- Modify: `crates/gitcore/src/lib.rs`
- Test: `crates/gitcore/tests/sync.rs` (+ unit tests in `net.rs`, `remote.rs`)

**Interfaces:**
- Consumes: `current_branch`, `operation_in_progress`, `run_git`, `git_ok`, `git_command` (Tasks 1–2).
- Produces: `NetAuth { github_token: Option<String> }` (Debug hides the token); `set_askpass_program(PathBuf)`; `ASKPASS_TOKEN_VAR = "RETROGIT_ASKPASS_TOKEN"`; `askpass_answer(prompt, token) -> String`; `net_settings(url, &NetAuth, askpass: Option<&Path>, ssh_configured: bool) -> NetSettings { pre_args, env }`; `NetProgress { phase, percent: Option<u8> }`, `parse_progress(&str)`; `classify_net_failure(&str) -> GitError`; `PullMode::{FastForwardOnly, Merge, Rebase}`, `PullOutcome::{UpToDate, FastForwarded, Merged, Rebased, Conflicts}`, `PushMode::{Normal, SetUpstream, ForceWithLease}`; `Repo::fetch(&NetAuth, progress, &AtomicBool)`, `Repo::pull(&NetAuth, PullMode, progress, &AtomicBool) -> Result<PullOutcome, _>`, `Repo::push(&NetAuth, PushMode, progress, &AtomicBool)`.

- [ ] **Step 1: Write the failing integration tests**

Everything runs against a local bare `origin.git`; a second clone pushes "remote" commits.

`crates/gitcore/tests/sync.rs`:

```rust
#![allow(clippy::unwrap_used)]
//! fetch / pull / push against a local bare remote (needs `git`).
mod common;

use std::sync::atomic::AtomicBool;

use common::remote::{Env, configure, git, no_cancel};
use gitcore::{
    CommitBackend, GitError, NetAuth, Operation, PullMode, PullOutcome, PushMode, Selection,
};

#[test]
fn checkout_of_a_remote_branch_creates_a_tracking_branch() {
    let Some(env) = Env::new() else { return };
    let other = env.root.join("pusher");
    git(
        &env.root,
        &["clone", "-q", "origin.git", other.to_str().unwrap()],
    );
    configure(&other);
    git(&other, &["switch", "-q", "-c", "topic"]);
    git(&other, &["push", "-q", "-u", "origin", "topic"]);
    let r = env.repo();
    r.fetch(&NetAuth::default(), |_| {}, &no_cancel()).unwrap();
    r.checkout_remote_branch("origin/topic").unwrap();
    let cur = r.current_branch().unwrap();
    assert_eq!(
        (cur.name.as_str(), cur.upstream.as_deref()),
        ("topic", Some("origin/topic"))
    );
}

#[test]
fn pull_fast_forward_divergence_merge_and_rebase() {
    let Some(env) = Env::new() else { return };
    let r = env.repo();
    let auth = NetAuth::default();
    assert_eq!(
        r.pull(&auth, PullMode::FastForwardOnly, |_| {}, &no_cancel())
            .unwrap(),
        PullOutcome::UpToDate
    );
    env.remote_commit("r1.txt", "r1\n");
    assert_eq!(
        r.pull(&auth, PullMode::FastForwardOnly, |_| {}, &no_cancel())
            .unwrap(),
        PullOutcome::FastForwarded
    );
    assert!(env.work.join("r1.txt").exists());

    env.remote_commit("r2.txt", "r2\n");
    env.local_commit("l1.txt", "l1\n");
    assert_eq!(
        r.pull(&auth, PullMode::FastForwardOnly, |_| {}, &no_cancel()),
        Err(GitError::Diverged {
            ahead: 1,
            behind: 1
        })
    );
    assert_eq!(
        r.pull(&auth, PullMode::Rebase, |_| {}, &no_cancel())
            .unwrap(),
        PullOutcome::Rebased
    );
    let b = r.current_branch().unwrap();
    assert_eq!((b.ahead, b.behind), (1, 0));

    env.remote_commit("r3.txt", "r3\n");
    env.local_commit("l2.txt", "l2\n");
    assert_eq!(
        r.pull(&auth, PullMode::Merge, |_| {}, &no_cancel())
            .unwrap(),
        PullOutcome::Merged
    );
    assert_eq!(r.log(0, 1).unwrap()[0].parents.len(), 2);
}

#[test]
fn pull_conflicts_leave_an_operation_that_can_be_aborted() {
    let Some(env) = Env::new() else { return };
    let r = env.repo();
    env.remote_commit("file0.txt", "theirs\n");
    env.local_commit("file0.txt", "ours\n");
    assert_eq!(
        r.pull(&NetAuth::default(), PullMode::Merge, |_| {}, &no_cancel())
            .unwrap(),
        PullOutcome::Conflicts
    );
    assert_eq!(r.operation_in_progress(), Some(Operation::Merge));
    assert!(
        r.status()
            .unwrap()
            .iter()
            .any(|f| f.unstaged == Some(gitcore::Change::Conflicted))
    );
    r.abort_operation().unwrap();
    assert_eq!(r.operation_in_progress(), None);

    assert_eq!(
        r.pull(&NetAuth::default(), PullMode::Rebase, |_| {}, &no_cancel())
            .unwrap(),
        PullOutcome::Conflicts
    );
    assert_eq!(r.operation_in_progress(), Some(Operation::Rebase));
    r.abort_operation().unwrap();
    assert_eq!(r.operation_in_progress(), None);
    assert_eq!(
        std::fs::read_to_string(env.work.join("file0.txt")).unwrap(),
        "ours\n"
    );
}

#[test]
fn rebase_can_continue_after_resolving() {
    let Some(env) = Env::new() else { return };
    let r = env.repo();
    env.remote_commit("file0.txt", "theirs\n");
    env.local_commit("file0.txt", "ours\n");
    assert_eq!(
        r.pull(&NetAuth::default(), PullMode::Rebase, |_| {}, &no_cancel())
            .unwrap(),
        PullOutcome::Conflicts
    );
    std::fs::write(env.work.join("file0.txt"), "resolved\n").unwrap();
    r.stage("file0.txt", &Selection::All, None).unwrap();
    r.continue_rebase().unwrap();
    assert_eq!(r.operation_in_progress(), None);
    assert_eq!(r.log(0, 1).unwrap()[0].summary, "local file0.txt");
}

#[test]
fn push_normal_rejected_publish_and_force_with_lease() {
    let Some(env) = Env::new() else { return };
    let r = env.repo();
    let auth = NetAuth::default();
    let mut phases = Vec::new();
    env.local_commit("p.txt", "p\n");
    r.push(&auth, PushMode::Normal, |p| phases.push(p), &no_cancel())
        .unwrap();
    assert_eq!(r.current_branch().unwrap().ahead, 0);

    env.remote_commit("q.txt", "q\n");
    env.local_commit("mine.txt", "m\n");
    assert_eq!(
        r.push(&auth, PushMode::Normal, |_| {}, &no_cancel()),
        Err(GitError::PushRejected)
    );

    r.create_branch("new-topic", true).unwrap();
    r.push(&auth, PushMode::SetUpstream, |_| {}, &no_cancel())
        .unwrap();
    assert_eq!(
        r.current_branch().unwrap().upstream.as_deref(),
        Some("origin/new-topic")
    );

    std::fs::write(env.work.join("mine.txt"), "amended\n").unwrap();
    r.stage("mine.txt", &Selection::All, None).unwrap();
    r.commit("amended", true, CommitBackend::PreferCli).unwrap();
    assert_eq!(
        r.push(&auth, PushMode::Normal, |_| {}, &no_cancel()),
        Err(GitError::PushRejected)
    );
    r.push(&auth, PushMode::ForceWithLease, |_| {}, &no_cancel())
        .unwrap();
}

#[test]
fn cancelled_fetch_returns_cancelled() {
    let Some(env) = Env::new() else { return };
    let cancel = AtomicBool::new(true);
    assert_eq!(
        env.repo().fetch(&NetAuth::default(), |_| {}, &cancel),
        Err(GitError::Cancelled)
    );
}
```

- [ ] **Step 2: Write the failing tests in `crates/gitcore/src/net.rs`**

Create `crates/gitcore/src/net.rs` containing only this test module for now (the implementation goes above it in a later step).

`crates/gitcore/src/net.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn token() -> NetAuth {
        NetAuth {
            github_token: Some("gho_x".into()),
        }
    }

    #[test]
    fn github_https_gets_the_token_through_askpass() {
        let s = net_settings(
            "https://github.com/o/r.git",
            &token(),
            Some(Path::new("/app/retrogit")),
            false,
        );
        assert_eq!(s.pre_args, vec!["-c", "credential.helper="]);
        assert!(
            s.env
                .contains(&("GIT_ASKPASS".into(), "/app/retrogit".into()))
        );
        assert!(s.env.contains(&(ASKPASS_TOKEN_VAR.into(), "gho_x".into())));
        assert!(s.env.contains(&("GIT_TERMINAL_PROMPT".into(), "0".into())));
    }

    #[test]
    fn ssh_and_other_hosts_never_see_the_token() {
        for url in [
            "git@github.com:o/r.git",
            "https://gitlab.com/o/r.git",
            "https://github.company.com/o/r",
        ] {
            let s = net_settings(url, &token(), Some(Path::new("/app/retrogit")), false);
            assert!(s.pre_args.is_empty(), "{url}");
            assert!(
                s.env
                    .iter()
                    .all(|(k, _)| k != ASKPASS_TOKEN_VAR && k != "GIT_ASKPASS"),
                "{url}"
            );
            assert!(
                s.env
                    .contains(&("GIT_SSH_COMMAND".into(), "ssh -o BatchMode=yes".into()))
            );
        }
    }

    #[test]
    fn a_custom_ssh_command_is_left_alone() {
        let s = net_settings("git@github.com:o/r.git", &NetAuth::default(), None, true);
        assert!(s.env.iter().all(|(k, _)| k != "GIT_SSH_COMMAND"));
    }

    #[test]
    fn askpass_answers_username_then_token() {
        assert_eq!(
            askpass_answer("Username for 'https://github.com': ", "t"),
            "x-access-token"
        );
        assert_eq!(
            askpass_answer("Password for 'https://x-access-token@github.com': ", "t"),
            "t"
        );
    }

    #[test]
    fn debug_hides_the_token() {
        assert!(!format!("{:?}", token()).contains("gho_x"));
    }
}
```

- [ ] **Step 3: Write the failing tests in `crates/gitcore/src/remote.rs`**

Create `crates/gitcore/src/remote.rs` containing only this test module for now (the implementation goes above it in a later step).

`crates/gitcore/src/remote.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_progress_lines() {
        assert_eq!(
            parse_progress("Receiving objects:  45% (450/1000), 1.2 MiB | 2 MiB/s"),
            Some(NetProgress {
                phase: "Receiving objects".into(),
                percent: Some(45)
            })
        );
        assert_eq!(
            parse_progress("remote: Counting objects: 100% (12/12), done."),
            Some(NetProgress {
                phase: "Counting objects".into(),
                percent: Some(100)
            })
        );
        assert_eq!(parse_progress("From github.com:o/r"), None);
        assert_eq!(parse_progress("To github.com:o/r.git"), None);
        assert_eq!(parse_progress(""), None);
    }

    #[test]
    fn classifies_network_failures() {
        let rejected =
            " ! [rejected]        main -> main (fetch first)\nerror: failed to push some refs";
        assert_eq!(classify_net_failure(rejected), GitError::PushRejected);
        assert!(matches!(
            classify_net_failure("fatal: Authentication failed for 'https://github.com/o/r'"),
            GitError::Auth(_)
        ));
        assert!(matches!(
            classify_net_failure("git@github.com: Permission denied (publickey)."),
            GitError::Auth(_)
        ));
        assert!(matches!(
            classify_net_failure(
                "fatal: could not read Username for 'https://x': terminal prompts disabled"
            ),
            GitError::Auth(_)
        ));
        assert!(matches!(
            classify_net_failure("ssh: Could not resolve host github.com"),
            GitError::Network(_)
        ));
        assert!(matches!(
            classify_net_failure("remote: error: GH006: Protected branch update failed"),
            GitError::Other(_)
        ));
    }
}
```

- [ ] **Step 4: Declare the modules (final)**

Replace the whole file with the version below (it keeps everything from sub-projects 1 and 2).

`crates/gitcore/src/lib.rs`:

```rust
//! RetroGit's internal Git API. No `git2` type is ever exposed publicly.

mod branch;
mod cli;
mod clone;
mod commit;
mod commit_detail;
mod diff;
mod discard;
mod error;
mod graph;
mod ignore;
mod log;
mod net;
mod ops;
mod remote;
mod repo;
mod signing;
mod stage;
mod stash;
mod status;

pub use branch::{Branch, parse_overwritten_files};
pub use commit_detail::{ChangedFile, CommitDetail, SignatureStatus, parse_signature_status};
pub use graph::{Edge, GraphRow, GraphState, layout};
pub use log::{LogEntry, RefKind, RefLabel};
pub use net::{
    ASKPASS_TOKEN_VAR, NetAuth, NetSettings, askpass_answer, net_settings, set_askpass_program,
};
pub use ops::Operation;
pub use remote::{
    NetProgress, PullMode, PullOutcome, PushMode, classify_net_failure, parse_progress,
};
pub use signing::{SigningConfig, SigningFormat};

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

- [ ] **Step 5: Run the tests to see them fail**

Run: `cargo test -p gitcore`

Expected: compile errors: `net_settings`, `parse_progress`, `NetAuth`, `PullMode`… not found.

- [ ] **Step 6: Implement the network environment**

Insert this **above** the existing `#[cfg(test)]` module in `crates/gitcore/src/net.rs`.

`-c credential.helper=` empties the helper list for this call only, so a stale Keychain password cannot win over the RetroGit token.

`crates/gitcore/src/net.rs`:

```rust
//! Environment for network git commands: never prompt, and hand the RetroGit token to git
//! for github.com HTTPS remotes through GIT_ASKPASS.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Credentials RetroGit can offer to `git`.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct NetAuth {
    pub github_token: Option<String>,
}

impl std::fmt::Debug for NetAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NetAuth")
            .field("github_token", &self.github_token.as_ref().map(|_| "***"))
            .finish()
    }
}

/// Environment variable carrying the token to the askpass helper (child process only).
pub const ASKPASS_TOKEN_VAR: &str = "RETROGIT_ASKPASS_TOKEN";

static ASKPASS_PROGRAM: OnceLock<PathBuf> = OnceLock::new();

/// Program `git` runs to ask for credentials: the RetroGit executable (`--askpass` mode).
pub fn set_askpass_program(path: PathBuf) {
    let _ = ASKPASS_PROGRAM.set(path);
}

/// What the askpass helper prints for git's `prompt`.
pub fn askpass_answer(prompt: &str, token: &str) -> String {
    if prompt.to_ascii_lowercase().contains("username") {
        "x-access-token".to_string()
    } else {
        token.to_string()
    }
}

/// Extra arguments (before the subcommand) and environment for a network command.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NetSettings {
    pub pre_args: Vec<String>,
    pub env: Vec<(String, String)>,
}

/// Pure: settings for a remote `url`. The token is only used for `https://github.com/`.
pub fn net_settings(
    url: &str,
    auth: &NetAuth,
    askpass: Option<&Path>,
    ssh_configured: bool,
) -> NetSettings {
    let mut s = NetSettings::default();
    s.env.push(("GIT_TERMINAL_PROMPT".into(), "0".into()));
    if !ssh_configured {
        // Fail fast instead of waiting for a passphrase nobody can type.
        s.env
            .push(("GIT_SSH_COMMAND".into(), "ssh -o BatchMode=yes".into()));
    }
    if let (true, Some(token), Some(program)) = (
        url.starts_with("https://github.com/"),
        &auth.github_token,
        askpass,
    ) {
        // Ignore stale credentials from helpers (Keychain...) for this call only.
        s.pre_args
            .extend(["-c".to_string(), "credential.helper=".to_string()]);
        s.env
            .push(("GIT_ASKPASS".into(), program.display().to_string()));
        s.env.push((ASKPASS_TOKEN_VAR.into(), token.clone()));
    }
    s
}

pub(crate) fn askpass_program() -> Option<&'static Path> {
    ASKPASS_PROGRAM.get().map(PathBuf::as_path)
}
```

- [ ] **Step 7: Implement fetch / pull / push**

Insert this **above** the existing `#[cfg(test)]` module in `crates/gitcore/src/remote.rs`.

`run_net` reads stderr byte by byte (progress lines end with `\r`), polls the child every 50 ms and kills it when `cancel` is set. Pull fetches first, then fast-forwards, reports `Diverged`, or merges/rebases on request; a merge/rebase that stops on conflicts returns `Ok(Conflicts)`.

`crates/gitcore/src/remote.rs`:

```rust
//! fetch / pull / push through the git command line, with progress and cancellation.

use std::io::Read;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::channel;
use std::time::Duration;

use crate::net::{NetAuth, askpass_program, net_settings};
use crate::{GitError, Repo};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetProgress {
    /// e.g. "Receiving objects".
    pub phase: String,
    pub percent: Option<u8>,
}

/// Parse one line of `git --progress` output.
pub fn parse_progress(line: &str) -> Option<NetProgress> {
    let line = line.trim().trim_start_matches("remote:").trim();
    let (phase, rest) = line.split_once(':')?;
    let pct = rest.trim_start().split('%').next()?.trim();
    let percent = pct.parse::<u8>().ok().filter(|p| *p <= 100);
    let phase = phase.trim();
    (percent.is_some() && !phase.is_empty()).then(|| NetProgress {
        phase: phase.to_string(),
        percent,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PullMode {
    /// Fast-forward only; `Diverged` otherwise.
    FastForwardOnly,
    Merge,
    Rebase,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PullOutcome {
    UpToDate,
    FastForwarded,
    Merged,
    Rebased,
    /// Merge or rebase stopped on conflicts (see `operation_in_progress`).
    Conflicts,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PushMode {
    Normal,
    /// First push of a branch: `push -u origin <branch>`.
    SetUpstream,
    /// After rewriting pushed commits (amend): `--force-with-lease`.
    ForceWithLease,
}

/// Map a failed network command's output to an error.
pub fn classify_net_failure(output: &str) -> GitError {
    let o = output.to_ascii_lowercase();
    if o.contains("[rejected]") && (o.contains("non-fast-forward") || o.contains("fetch first")) {
        GitError::PushRejected
    } else if o.contains("authentication failed")
        || o.contains("could not read username")
        || o.contains("terminal prompts disabled")
        || o.contains("permission denied (publickey")
        || o.contains("returned error: 403")
        || o.contains("returned error: 401")
    {
        GitError::Auth(output.trim().to_string())
    } else if o.contains("could not resolve host")
        || o.contains("connection timed out")
        || o.contains("failed to connect")
        || o.contains("network is unreachable")
    {
        GitError::Network(output.trim().to_string())
    } else {
        GitError::Other(output.trim().to_string())
    }
}

impl Repo {
    fn origin_url(&self) -> Option<String> {
        self.git()
            .find_remote("origin")
            .ok()
            .and_then(|r| r.url().ok().map(str::to_owned))
    }

    /// Run a network command; progress lines are reported; the process is killed on cancel.
    fn run_net(
        &self,
        auth: &NetAuth,
        args: &[&str],
        mut progress: impl FnMut(NetProgress),
        cancel: &AtomicBool,
    ) -> Result<String, GitError> {
        if !crate::git_available() {
            return Err(GitError::GitMissing);
        }
        let ssh_configured = std::env::var_os("GIT_SSH_COMMAND").is_some()
            || self
                .git()
                .config()
                .ok()
                .and_then(|c| c.get_string("core.sshCommand").ok())
                .is_some();
        let settings = net_settings(
            &self.origin_url().unwrap_or_default(),
            auth,
            askpass_program(),
            ssh_configured,
        );
        let mut cmd = crate::commit::git_command();
        cmd.args(&settings.pre_args)
            .arg("-C")
            .arg(self.workdir()?)
            .args(args);
        for (k, v) in &settings.env {
            cmd.env(k, v);
        }
        let mut child = cmd
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| GitError::Other(format!("cannot run git: {e}")))?;
        let mut stderr = child
            .stderr
            .take()
            .ok_or_else(|| GitError::Other("no stderr".into()))?;
        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| GitError::Other("no stdout".into()))?;
        let (tx, rx) = channel::<String>();
        let reader = std::thread::spawn(move || {
            // Progress lines end with '\r' (updates) or '\n'.
            let mut all = String::new();
            let mut buf = [0u8; 4096];
            let mut line = Vec::new();
            while let Ok(n) = stderr.read(&mut buf) {
                if n == 0 {
                    break;
                }
                for &b in &buf[..n] {
                    if b == b'\r' || b == b'\n' {
                        let s = String::from_utf8_lossy(&line).into_owned();
                        all.push_str(&s);
                        all.push('\n');
                        let _ = tx.send(s);
                        line.clear();
                    } else {
                        line.push(b);
                    }
                }
            }
            all.push_str(&String::from_utf8_lossy(&line));
            all
        });
        let status = loop {
            while let Ok(l) = rx.try_recv() {
                if let Some(p) = parse_progress(&l) {
                    progress(p);
                }
            }
            if cancel.load(Ordering::Relaxed) {
                let _ = child.kill();
                let _ = child.wait();
                let _ = reader.join();
                return Err(GitError::Cancelled);
            }
            match child.try_wait() {
                Ok(Some(s)) => break s,
                Ok(None) => std::thread::sleep(Duration::from_millis(50)),
                Err(e) => return Err(GitError::Other(format!("git failed: {e}"))),
            }
        };
        let mut out = String::new();
        let _ = stdout.read_to_string(&mut out);
        let err = reader.join().unwrap_or_default();
        let text = format!("{out}{err}");
        if status.success() {
            Ok(text)
        } else {
            Err(classify_net_failure(&text))
        }
    }

    pub fn fetch(
        &self,
        auth: &NetAuth,
        progress: impl FnMut(NetProgress),
        cancel: &AtomicBool,
    ) -> Result<(), GitError> {
        self.run_net(
            auth,
            &["fetch", "--prune", "--progress", "origin"],
            progress,
            cancel,
        )
        .map(|_| ())
    }

    /// Fetch, then integrate the upstream according to `mode`.
    pub fn pull(
        &self,
        auth: &NetAuth,
        mode: PullMode,
        progress: impl FnMut(NetProgress),
        cancel: &AtomicBool,
    ) -> Result<PullOutcome, GitError> {
        let branch = self
            .current_branch()
            .ok_or_else(|| GitError::Unsupported("HEAD is detached".into()))?;
        if branch.upstream.is_none() {
            return Err(GitError::Unsupported(
                "this branch has no upstream: publish it first".into(),
            ));
        }
        self.fetch(auth, progress, cancel)?;
        let b = self
            .current_branch()
            .ok_or_else(|| GitError::Unsupported("HEAD is detached".into()))?;
        if b.behind == 0 {
            return Ok(PullOutcome::UpToDate);
        }
        if b.ahead == 0 {
            self.git_ok(&["merge", "--ff-only", "@{u}"])?;
            return Ok(PullOutcome::FastForwarded);
        }
        let (args, done): (&[&str], PullOutcome) = match mode {
            PullMode::FastForwardOnly => {
                return Err(GitError::Diverged {
                    ahead: b.ahead,
                    behind: b.behind,
                });
            }
            PullMode::Merge => (&["merge", "--no-edit", "@{u}"], PullOutcome::Merged),
            PullMode::Rebase => (&["rebase", "@{u}"], PullOutcome::Rebased),
        };
        let out = self.run_git(args)?;
        if out.success {
            Ok(done)
        } else if self.operation_in_progress().is_some() {
            Ok(PullOutcome::Conflicts)
        } else {
            Err(crate::classify_commit_failure(&out.text))
        }
    }

    pub fn push(
        &self,
        auth: &NetAuth,
        mode: PushMode,
        progress: impl FnMut(NetProgress),
        cancel: &AtomicBool,
    ) -> Result<(), GitError> {
        let branch = self
            .current_branch()
            .ok_or_else(|| GitError::Unsupported("HEAD is detached".into()))?;
        let args: Vec<&str> = match mode {
            PushMode::Normal => vec!["push", "--progress"],
            PushMode::SetUpstream => {
                vec!["push", "--progress", "-u", "origin", branch.name.as_str()]
            }
            PushMode::ForceWithLease => vec!["push", "--progress", "--force-with-lease"],
        };
        self.run_net(auth, &args, progress, cancel).map(|_| ())
    }
}
```

- [ ] **Step 8: Run the tests to see them pass**

Run: `cargo test -p gitcore`

Expected: unit tests (graph, net, remote) pass; `tests/sync.rs` `6 passed`.

- [ ] **Step 9: Lint**

Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings`

Expected: no output from fmt; clippy finishes without warnings.

- [ ] **Step 10: Full suite**

Run: `cargo test --workspace`

Expected: all tests pass (0 failed).

- [ ] **Step 11: Commit**

```bash
git add crates/gitcore
git commit -m "feat(gitcore): fetch, pull and push via git with askpass token and progress"
```


### Task 4: `win95` combo box and splitter

**Files:**
- Create: `crates/win95/src/combo_box.rs`, `crates/win95/src/splitter.rs`
- Modify: `crates/win95/src/lib.rs`

**Interfaces:**
- Produces: `win95::combo_box(ui, id, selected_text: &str, width: f32, add_items: impl FnOnce(&mut Ui)) -> Response` (accessible as `Role::ComboBox`; popup closes on click); `win95::splitter<S>(ui, id, fraction: &mut f32, state: &mut S, add_top: impl FnOnce(&mut Ui, &mut S), add_bottom: impl FnOnce(&mut Ui, &mut S))` (`fraction` clamped to 0.1..=0.9; `state` lets both halves share `&mut` data).

- [ ] **Step 1: Write the failing tests in `crates/win95/src/combo_box.rs`**

Create `crates/win95/src/combo_box.rs` containing only this test module for now (the implementation goes above it in a later step).

`crates/win95/src/combo_box.rs`:

```rust
#[cfg(test)]
mod tests {
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    #[test]
    fn opening_and_picking_an_item() {
        let mut h = Harness::new_ui_state(
            |ui, picked: &mut String| {
                super::combo_box(ui, "branches", "main", 180.0, |ui| {
                    for name in ["main", "feature"] {
                        if ui.button(name).clicked() {
                            *picked = name.to_string();
                        }
                    }
                });
            },
            String::new(),
        );
        h.run();
        assert!(h.query_by_label("feature").is_none(), "closed at first");
        h.get_by_role(egui::accesskit::Role::ComboBox).click();
        h.run();
        h.get_by_label("feature").click();
        h.run();
        assert_eq!(h.state(), "feature");
    }
}
```

- [ ] **Step 2: Write the failing tests in `crates/win95/src/splitter.rs`**

Create `crates/win95/src/splitter.rs` containing only this test module for now (the implementation goes above it in a later step).

`crates/win95/src/splitter.rs`:

```rust
#[cfg(test)]
mod tests {
    use egui_kittest::Harness;

    #[test]
    fn dragging_the_bar_changes_the_split() {
        let mut h = Harness::builder()
            .with_size(egui::vec2(300.0, 406.0))
            .build_ui_state(
                |ui, f: &mut f32| {
                    super::splitter(
                        ui,
                        "split",
                        f,
                        &mut (),
                        |ui, _| {
                            ui.label("top");
                        },
                        |ui, _| {
                            ui.label("bottom");
                        },
                    );
                },
                0.5f32,
            );
        h.run();
        // Bar sits at y = 200..206 (half of 400 + margin); drag it down by 100 px.
        let start = egui::pos2(150.0, 208.0);
        h.drag_at(start);
        h.run();
        h.hover_at(start + egui::vec2(0.0, 100.0));
        h.run();
        h.drop_at(start + egui::vec2(0.0, 100.0));
        h.run();
        assert!(*h.state() > 0.6, "fraction {}", h.state());
        assert!(*h.state() <= 0.9);
    }
}
```

- [ ] **Step 3: Export the widgets**

Replace the whole file with the version below (it keeps everything from sub-projects 1 and 2).

`crates/win95/src/lib.rs`:

```rust
//! Windows 95 look-and-feel for egui. No business logic lives here.

pub mod bevel;
pub mod button;
pub mod checkbox;
pub mod combo_box;
pub mod dialog;
pub mod icon;
pub mod list_view;
pub mod panel;
pub mod progress;
pub mod splitter;
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
pub use combo_box::combo_box;
pub use dialog::{Dialog, DialogResponse};
pub use icon::Icon;
pub use list_view::{Cell, Column, ListResponse, ListView};
pub use panel::bevel_frame;
pub use progress::ProgressBar95;
pub use splitter::splitter;
pub use status_bar::status_bar;
pub use tabs::tabs;
pub use text_area::text_area;
pub use text_field::text_field;
pub use title_bar::{TitleAction, TitleBar};
pub use window_frame::{above_dialogs, resize_edges};
```

- [ ] **Step 4: Run the tests to see them fail**

Run: `cargo test -p win95 --lib`

Expected: compile errors: `combo_box`, `splitter` not found.

- [ ] **Step 5: Implement `crates/win95/src/combo_box.rs`**

Insert this **above** the existing `#[cfg(test)]` module in `crates/win95/src/combo_box.rs`.

`crates/win95/src/combo_box.rs`:

```rust
use egui::{
    Align2, Id, Popup, Rect, Response, Sense, Stroke, Ui, WidgetInfo, WidgetType, pos2, vec2,
};

use crate::bevel::{self, Bevel};
use crate::theme::{self, BLACK, SILVER, WHITE};

/// Win95 drop-down list: white sunken field with the current value and an arrow button.
/// `add_items` fills the popup; clicking an item closes it.
pub fn combo_box(
    ui: &mut Ui,
    id: impl egui::AsId,
    selected_text: &str,
    width: f32,
    add_items: impl FnOnce(&mut Ui),
) -> Response {
    let id = Id::new(id);
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 21.0), Sense::click());
    let owned = selected_text.to_string();
    resp.widget_info(|| WidgetInfo::labeled(WidgetType::ComboBox, true, &owned));
    let p = ui.painter();
    p.rect_filled(rect, 0.0, WHITE);
    bevel::paint(p, rect, Bevel::Field);
    let arrow = Rect::from_min_max(
        pos2(rect.right() - 18.0, rect.top() + 2.0),
        rect.max - vec2(2.0, 2.0),
    );
    p.rect_filled(arrow, 0.0, SILVER);
    bevel::paint(p, arrow, Bevel::Raised);
    let c = arrow.center();
    p.add(egui::Shape::convex_polygon(
        vec![
            c + vec2(-4.0, -2.0),
            c + vec2(4.0, -2.0),
            c + vec2(0.0, 2.0),
        ],
        BLACK,
        Stroke::NONE,
    ));
    p.with_clip_rect(Rect::from_min_max(
        rect.min,
        pos2(arrow.left() - 2.0, rect.bottom()),
    ))
    .text(
        rect.left_center() + vec2(5.0, 0.0),
        Align2::LEFT_CENTER,
        selected_text,
        theme::font(theme::FONT_SIZE),
        BLACK,
    );
    Popup::from_toggle_button_response(&resp)
        .id(id.with("popup"))
        .width(width)
        .show(|ui| {
            ui.set_min_width(width - 8.0);
            add_items(ui);
        });
    resp
}
```

- [ ] **Step 6: Implement `crates/win95/src/splitter.rs`**

Insert this **above** the existing `#[cfg(test)]` module in `crates/win95/src/splitter.rs`.

`crates/win95/src/splitter.rs`:

```rust
use egui::{CursorIcon, Id, Rect, Sense, Ui, UiBuilder, pos2, vec2};

use crate::bevel::{self, Bevel};
use crate::theme::SILVER;

/// Horizontal split: `add_top` above, `add_bottom` below, a draggable 6 px bar in between.
/// `fraction` (0.1..=0.9) is the share of the height given to the top part. `state` is
/// handed to both halves in turn (so they can share mutable state).
pub fn splitter<S>(
    ui: &mut Ui,
    id: impl egui::AsId,
    fraction: &mut f32,
    state: &mut S,
    add_top: impl FnOnce(&mut Ui, &mut S),
    add_bottom: impl FnOnce(&mut Ui, &mut S),
) {
    let id = Id::new(id);
    let rect = ui.available_rect_before_wrap();
    let bar_h = 6.0;
    *fraction = fraction.clamp(0.1, 0.9);
    let top_h = ((rect.height() - bar_h) * *fraction).max(0.0);
    let top = Rect::from_min_size(rect.min, vec2(rect.width(), top_h));
    let bar = Rect::from_min_size(pos2(rect.left(), top.bottom()), vec2(rect.width(), bar_h));
    let bottom = Rect::from_min_max(pos2(rect.left(), bar.bottom()), rect.max);

    let resp = ui
        .interact(bar, id.with("bar"), Sense::drag())
        .on_hover_cursor(CursorIcon::ResizeVertical);
    if resp.dragged() && rect.height() > bar_h {
        *fraction = ((top_h + resp.drag_delta().y) / (rect.height() - bar_h)).clamp(0.1, 0.9);
    }
    ui.painter().rect_filled(bar, 0.0, SILVER);
    bevel::paint(ui.painter(), bar.shrink2(vec2(0.0, 1.0)), Bevel::Raised);

    ui.scope_builder(
        UiBuilder::new().max_rect(top).id_salt(id.with("top")),
        |ui| {
            ui.set_clip_rect(top);
            add_top(ui, state);
        },
    );
    ui.scope_builder(
        UiBuilder::new().max_rect(bottom).id_salt(id.with("bottom")),
        |ui| {
            ui.set_clip_rect(bottom);
            add_bottom(ui, state);
        },
    );
    ui.allocate_rect(rect, Sense::hover());
}
```

- [ ] **Step 7: Run the tests to see them pass**

Run: `cargo test -p win95 --lib`

Expected: `18 passed`.

- [ ] **Step 8: Lint**

Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings`

Expected: no output from fmt; clippy finishes without warnings.

- [ ] **Step 9: Commit**

```bash
git add crates/win95
git commit -m "feat(win95): combo box and splitter"
```


### Task 5: App state, worker commands, askpass mode, panic hook

**Files:**
- Create: `crates/app/src/state/sync.rs`, `crates/app/src/worker/sync.rs`, `crates/app/tests/askpass.rs`
- Modify: `crates/app/src/protocol.rs`, `crates/app/src/state.rs`, `crates/app/src/worker.rs`, `crates/app/src/worker/changes.rs`, `crates/app/src/main.rs`, `crates/app/src/logging.rs`
- Test: `crates/app/tests/state.rs`, `crates/app/tests/worker.rs`

**Interfaces:**
- Consumes: everything from Tasks 1–3.
- Produces:
  - `Command` new variants: `LoadLog { skip }`, `LoadCommit(String)`, `LoadCommitFileDiff { id, path }`, `LoadBranches`, `CreateBranch { name, switch }`, `SwitchBranch { name, stash }` (`origin/x` = check out remote), `RenameBranch { old, new }`, `DeleteBranch { name, force }`, `Fetch { background }`, `Pull(PullMode)`, `Push(PushMode)`, `AbortOperation`, `ContinueRebase`; `SyncOp::{Fetch, Pull, Push}`; `Op::{History, Sync}`.
  - `Event` new variants: `LogLoaded { skip, entries }`, `CommitLoaded(CommitDetail)`, `SignatureLoaded { id, status }`, `CommitFileDiffLoaded { id, diff }`, `BranchesLoaded(Vec<Branch>)`, `OperationChanged(Option<Operation>)`, `SigningLoaded(Option<SigningConfig>)`, `SyncStarted { op, background }`, `SyncProgress(NetProgress)`, `SyncFinished { op, ok }`, `Pulled(PullOutcome)`, `Diverged { ahead, behind }`, `PushRejected`, `WouldOverwrite { branch, files }`, `NotMerged(String)`.
  - `state::{Tab::{Changes, History}, HistoryView, SyncView, PendingDialog, LOG_PAGE = 500, branch_name_error(&str, &[Branch]) -> Option<String>}`; `AppState` fields `tab, history, branches, sync, dialog, amended_pushed, operation, signing`; `AppState::{select_commit, wants_more_history}`.
  - `WorkerHandle::cancel_network()`; opening a repo sends signing, branches, first history page, then a background fetch.
  - `retrogit --askpass <prompt>` prints `askpass_answer(prompt, $RETROGIT_ASKPASS_TOKEN)`; `logging::{install_panic_hook, panic_text}`.

- [ ] **Step 1: Write the failing reducer tests**

Replace the whole file with the version below (it keeps everything from sub-projects 1 and 2). Adds the `sync` test module.

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
    fn an_identical_diff_from_a_background_refresh_keeps_the_selection() {
        let mut s = AppState::new(Config::default());
        s.changes.shown = Some(("a.txt".into(), Side::Unstaged));
        s.apply(Event::DiffLoaded(diff("a.txt", Side::Unstaged)));
        s.changes.selected_lines.insert((0, 1));
        s.changes.show_large = true;
        s.apply(Event::DiffLoaded(diff("a.txt", Side::Unstaged)));
        assert_eq!(s.changes.selected_lines.len(), 1);
        assert!(s.changes.show_large);
        let mut changed = diff("a.txt", Side::Unstaged);
        changed.binary = true;
        s.apply(Event::DiffLoaded(changed));
        assert!(
            s.changes.selected_lines.is_empty(),
            "a different diff invalidates the selection"
        );
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

mod discard {
    use super::*;
    use retrogit::protocol::Command;

    #[test]
    fn discard_is_sent_only_after_confirmation() {
        let mut s = AppState::new(Config::default());
        let cmd = Command::DiscardFiles(vec!["a.txt".into()]);
        s.changes
            .request_discard(cmd.clone(), "Discard a.txt?".into());
        assert_eq!(
            s.changes
                .pending_discard
                .as_ref()
                .map(|p| p.question.as_str()),
            Some("Discard a.txt?")
        );
        s.changes.cancel_discard();
        assert!(s.changes.pending_discard.is_none());
        s.changes
            .request_discard(cmd.clone(), "Discard a.txt?".into());
        assert_eq!(s.changes.confirm_discard(), Some(cmd));
        assert_eq!(
            s.changes.confirm_discard(),
            None,
            "a confirmation is used once"
        );
    }
}

mod recents {
    use super::*;

    #[test]
    fn recents_are_listed_alphabetically_and_opening_does_not_reorder() {
        let mut s = AppState::new(Config::default());
        for p in ["/w/zeta", "/w/Alpha", "/w/beta"] {
            s.apply(Event::RepoOpened(RepoSummary {
                name: p.rsplit('/').next().unwrap().into(),
                ..summary(p)
            }));
        }
        let names = |s: &AppState| {
            s.recents_sorted()
                .into_iter()
                .map(|r| r.name)
                .collect::<Vec<_>>()
        };
        assert_eq!(names(&s), ["Alpha", "beta", "zeta"]);
        s.apply(Event::RepoOpened(RepoSummary {
            name: "zeta".into(),
            ..summary("/w/zeta")
        }));
        assert_eq!(names(&s), ["Alpha", "beta", "zeta"]);
        assert_eq!(
            s.selected_recent(),
            Some(2),
            "the open repo is the highlighted row"
        );
    }

    #[test]
    fn same_name_is_ordered_by_path_and_no_repo_means_no_highlight() {
        let mut s = AppState::new(Config::default());
        s.config.add_recent("app", std::path::Path::new("/b/app"));
        s.config.add_recent("app", std::path::Path::new("/a/app"));
        let paths: Vec<_> = s.recents_sorted().into_iter().map(|r| r.path).collect();
        assert_eq!(paths, [PathBuf::from("/a/app"), PathBuf::from("/b/app")]);
        assert_eq!(s.selected_recent(), None);
    }
}

mod sync {
    use super::*;
    use gitcore::{Branch, LogEntry, PullOutcome};
    use retrogit::protocol::SyncOp;
    use retrogit::state::{LOG_PAGE, PendingDialog, Tab, branch_name_error};

    fn entry(id: &str, parents: &[&str]) -> LogEntry {
        LogEntry {
            id: id.into(),
            short_id: id.into(),
            parents: parents.iter().map(|p| p.to_string()).collect(),
            author: "Ada".into(),
            email: String::new(),
            time: 0,
            summary: id.into(),
            refs: vec![],
        }
    }

    #[test]
    fn history_pages_append_and_stale_pages_are_ignored() {
        let mut s = AppState::new(Config::default());
        let page1: Vec<_> = (0..LOG_PAGE)
            .map(|i| entry(&format!("c{i}"), &[&format!("c{}", i + 1)]))
            .collect();
        s.apply(Event::LogLoaded {
            skip: 0,
            entries: page1,
        });
        assert_eq!(s.history.entries.len(), LOG_PAGE);
        assert_eq!(s.history.graph.len(), LOG_PAGE);
        assert!(!s.history.end_reached && s.wants_more_history());
        s.apply(Event::LogLoaded {
            skip: 7,
            entries: vec![entry("x", &[])],
        });
        assert_eq!(
            s.history.entries.len(),
            LOG_PAGE,
            "a page for another offset is ignored"
        );
        s.apply(Event::LogLoaded {
            skip: LOG_PAGE,
            entries: vec![entry(&format!("c{LOG_PAGE}"), &[])],
        });
        assert_eq!(s.history.entries.len(), LOG_PAGE + 1);
        assert!(s.history.end_reached && !s.wants_more_history());
        s.apply(Event::LogLoaded {
            skip: 0,
            entries: vec![entry("new", &[])],
        });
        assert_eq!(s.history.entries.len(), 1, "a reload starts over");
    }

    #[test]
    fn commit_detail_is_kept_only_for_the_selected_commit() {
        let mut s = AppState::new(Config::default());
        s.select_commit("aaa");
        let detail = |id: &str| gitcore::CommitDetail {
            id: id.into(),
            short_id: id.into(),
            parents: vec![],
            author: String::new(),
            email: String::new(),
            time: 0,
            committer: String::new(),
            message: String::new(),
            files: vec![],
        };
        s.apply(Event::CommitLoaded(detail("bbb")));
        assert!(s.history.detail.is_none());
        s.apply(Event::CommitLoaded(detail("aaa")));
        assert!(s.history.detail.is_some());
        s.apply(Event::SignatureLoaded {
            id: "aaa".into(),
            status: gitcore::SignatureStatus::Unsigned,
        });
        assert_eq!(
            s.history.signature,
            Some(gitcore::SignatureStatus::Unsigned)
        );
        s.select_commit("ccc");
        assert!(s.history.detail.is_none() && s.history.signature.is_none());
    }

    #[test]
    fn sync_lifecycle_and_dialogs() {
        let mut s = AppState::new(Config::default());
        s.apply(Event::SyncStarted {
            op: SyncOp::Push,
            background: false,
        });
        assert_eq!(s.sync.running, Some(SyncOp::Push));
        s.apply(Event::SyncProgress(gitcore::NetProgress {
            phase: "Writing objects".into(),
            percent: Some(40),
        }));
        assert!(s.sync.progress.is_some());
        s.apply(Event::SyncFinished {
            op: SyncOp::Push,
            ok: false,
        });
        s.apply(Event::PushRejected);
        assert_eq!(s.sync.running, None);
        assert_eq!(
            s.dialog,
            Some(PendingDialog::PushRejected { can_force: false })
        );

        s.changes.amend = true;
        s.changes.head_pushed = true;
        s.apply(Event::Committed(gitcore::CommitOutcome {
            commit: gitcore::CommitInfo {
                short_id: "a".into(),
                summary: "s".into(),
                author: "A".into(),
                time: 0,
            },
            used_cli: true,
        }));
        s.apply(Event::PushRejected);
        assert_eq!(
            s.dialog,
            Some(PendingDialog::PushRejected { can_force: true }),
            "force offered after amending a pushed commit"
        );
        s.apply(Event::SyncFinished {
            op: SyncOp::Push,
            ok: true,
        });
        assert!(!s.amended_pushed);
        assert_eq!(s.sync.note.as_deref(), Some("Pushed"));

        s.apply(Event::Diverged {
            ahead: 2,
            behind: 3,
        });
        assert_eq!(
            s.dialog,
            Some(PendingDialog::Diverged {
                ahead: 2,
                behind: 3
            })
        );
        s.apply(Event::WouldOverwrite {
            branch: "b".into(),
            files: vec!["f".into()],
        });
        assert!(matches!(
            s.dialog,
            Some(PendingDialog::WouldOverwrite { .. })
        ));
        s.apply(Event::NotMerged("old".into()));
        assert_eq!(
            s.dialog,
            Some(PendingDialog::DeleteNotMerged { name: "old".into() })
        );
    }

    #[test]
    fn conflicts_after_pull_switch_to_changes_with_a_message() {
        let mut s = AppState::new(Config::default());
        s.tab = Tab::History;
        s.apply(Event::Pulled(PullOutcome::Conflicts));
        assert_eq!(s.tab, Tab::Changes);
        assert_eq!(s.messages.len(), 1);
        s.apply(Event::Pulled(PullOutcome::UpToDate));
        assert_eq!(s.sync.note.as_deref(), Some("Already up to date"));
    }

    #[test]
    fn a_sync_error_stops_the_progress() {
        let mut s = AppState::new(Config::default());
        s.apply(Event::SyncStarted {
            op: SyncOp::Fetch,
            background: true,
        });
        s.apply(Event::Error {
            during: Op::Sync,
            error: err(),
        });
        assert_eq!(s.sync.running, None);
    }

    #[test]
    fn opening_another_repo_resets_history_branches_and_dialogs() {
        let mut s = AppState::new(Config::default());
        s.apply(Event::RepoOpened(summary("/tmp/a")));
        s.apply(Event::LogLoaded {
            skip: 0,
            entries: vec![entry("x", &[])],
        });
        s.apply(Event::BranchesLoaded(vec![Branch {
            name: "main".into(),
            remote: false,
            is_head: true,
            upstream: None,
            ahead: 0,
            behind: 0,
        }]));
        s.dialog = Some(PendingDialog::ConfirmForcePush);
        s.apply(Event::RepoOpened(summary("/tmp/b")));
        assert!(s.history.entries.is_empty() && s.branches.is_empty() && s.dialog.is_none());
    }

    #[test]
    fn branch_names_follow_git_rules() {
        let existing = vec![Branch {
            name: "main".into(),
            remote: false,
            is_head: true,
            upstream: None,
            ahead: 0,
            behind: 0,
        }];
        for bad in [
            "", " ", "a b", "-x", "x/", "x.lock", "a..b", "a~1", "a^", "a:b", "a?", "a*", "a[b",
            "a\\b", "@", "x@{y", ".hidden", "a/.b", "a//b", "end.",
        ] {
            assert!(
                branch_name_error(bad, &existing).is_some(),
                "{bad:?} should be refused"
            );
        }
        assert!(
            branch_name_error("main", &existing)
                .unwrap()
                .contains("already exists")
        );
        for good in ["feature/login", "fix-42", "v1.2", "user/ada/test"] {
            assert_eq!(branch_name_error(good, &existing), None, "{good:?}");
        }
    }
}
```

- [ ] **Step 2: Write the failing worker tests**

Replace the whole file with the version below (it keeps everything from sub-projects 1 and 2). Adds the `sync` test module (bare local remote).

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

#[test]
fn stage_all_is_one_operation_with_one_refresh() {
    let server = mockito::Server::new();
    let d = repo_for_changes();
    for i in 0..20 {
        std::fs::write(d.path().join(format!("f{i}.txt")), "x\n").unwrap();
    }
    let w = start(&server, Arc::new(MemoryStore::default()), "");
    w.send(Command::OpenRepo(d.path().to_path_buf()));
    until(&w, |e| matches!(e, Event::StatusLoaded(_)));
    let paths: Vec<String> = (0..20).map(|i| format!("f{i}.txt")).collect();
    w.send(Command::StageFiles(paths.clone()));
    w.send(Command::ValidateToken); // marker
    let evs = until(&w, |e| matches!(e, Event::SignedOut));
    let statuses: Vec<_> = evs
        .iter()
        .filter_map(|e| match e {
            Event::StatusLoaded(f) => Some(f),
            _ => None,
        })
        .collect();
    assert_eq!(statuses.len(), 1);
    assert_eq!(
        statuses[0].iter().filter(|f| f.staged.is_some()).count(),
        20
    );
    w.send(Command::UnstageFiles(paths));
    let evs = until(&w, |e| matches!(e, Event::StatusLoaded(_)));
    let Some(Event::StatusLoaded(files)) = evs.last() else {
        unreachable!()
    };
    assert!(files.iter().all(|f| f.staged.is_none()));
}

#[test]
fn discard_commands_revert_the_working_tree_and_refresh() {
    let server = mockito::Server::new();
    let d = repo_for_changes();
    std::fs::write(d.path().join("README.md"), "hello\nnoise\n").unwrap();
    let w = start(&server, Arc::new(MemoryStore::default()), "");
    w.send(Command::OpenRepo(d.path().to_path_buf()));
    until(&w, |e| matches!(e, Event::StatusLoaded(_)));
    w.send(Command::Discard {
        path: "README.md".into(),
        selection: gitcore::Selection::Hunks(vec![0]),
        shown: None,
    });
    let evs = until(&w, |e| matches!(e, Event::StatusLoaded(_)));
    let Some(Event::StatusLoaded(files)) = evs.last() else {
        unreachable!()
    };
    assert!(files.is_empty(), "{files:?}");
    assert_eq!(
        std::fs::read_to_string(d.path().join("README.md")).unwrap(),
        "hello\n"
    );
    std::fs::write(d.path().join("README.md"), "changed\n").unwrap();
    w.send(Command::DiscardFiles(vec!["README.md".into()]));
    until(&w, |e| matches!(e, Event::StatusLoaded(f) if f.is_empty()));
    assert_eq!(
        std::fs::read_to_string(d.path().join("README.md")).unwrap(),
        "hello\n"
    );
}

mod sync {
    use super::*;
    use retrogit::protocol::SyncOp;
    use std::process::Command as Cmd;

    fn git(dir: &Path, args: &[&str]) {
        let out = Cmd::new("git")
            .current_dir(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn configure(dir: &Path) {
        for (k, v) in [
            ("user.name", "Ada"),
            ("user.email", "ada@example.com"),
            ("commit.gpgsign", "false"),
            ("core.hooksPath", ".git/hooks"),
        ] {
            git(dir, &["config", k, v]);
        }
    }

    /// Bare remote + a clone tracking it; `None` when git is not installed.
    fn remote_env() -> Option<(tempfile::TempDir, std::path::PathBuf)> {
        if !gitcore::git_available() {
            return None;
        }
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let seed = root.join("seed");
        make_source_repo(&seed);
        git(
            &root,
            &[
                "clone",
                "-q",
                "--bare",
                seed.to_str().unwrap(),
                "origin.git",
            ],
        );
        git(&root, &["clone", "-q", "origin.git", "work"]);
        let work = root.join("work");
        configure(&work);
        Some((tmp, work))
    }

    fn push_from_other(work: &Path, file: &str) {
        let root = work.parent().unwrap();
        let other = root.join(format!("other-{file}"));
        git(
            root,
            &["clone", "-q", "origin.git", other.to_str().unwrap()],
        );
        configure(&other);
        std::fs::write(other.join(file), "remote\n").unwrap();
        git(&other, &["add", file]);
        git(&other, &["commit", "-q", "-m", "remote change"]);
        git(&other, &["push", "-q"]);
    }

    #[test]
    fn opening_a_repo_sends_branches_history_and_signing() {
        let Some((_tmp, work)) = remote_env() else {
            return;
        };
        let server = mockito::Server::new();
        let w = start(&server, Arc::new(MemoryStore::default()), "");
        w.send(Command::OpenRepo(work.clone()));
        let evs = until(&w, |e| matches!(e, Event::LogLoaded { .. }));
        assert!(
            evs.iter()
                .any(|e| matches!(e, Event::SigningLoaded(Some(_))))
        );
        assert!(evs.iter().any(|e| matches!(e, Event::BranchesLoaded(b) if b.iter().any(|b| b.name == "main" && b.is_head))));
        assert!(
            evs.iter()
                .any(|e| matches!(e, Event::OperationChanged(None)))
        );
        let Some(Event::LogLoaded { skip: 0, entries }) = evs.last() else {
            unreachable!()
        };
        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn diverged_pull_asks_then_rebase_succeeds() {
        let Some((_tmp, work)) = remote_env() else {
            return;
        };
        push_from_other(&work, "theirs.txt");
        std::fs::write(work.join("mine.txt"), "mine\n").unwrap();
        git(&work, &["add", "mine.txt"]);
        git(&work, &["commit", "-q", "-m", "mine"]);
        let server = mockito::Server::new();
        let w = start(&server, Arc::new(MemoryStore::default()), "");
        w.send(Command::OpenRepo(work.clone()));
        until(&w, |e| {
            matches!(
                e,
                Event::SyncFinished {
                    op: SyncOp::Fetch,
                    ..
                }
            )
        });
        w.send(Command::Pull(gitcore::PullMode::FastForwardOnly));
        until(&w, |e| {
            matches!(
                e,
                Event::Diverged {
                    ahead: 1,
                    behind: 1
                }
            )
        });
        w.send(Command::Pull(gitcore::PullMode::Rebase));
        let evs = until(&w, |e| matches!(e, Event::Pulled(_)));
        assert!(matches!(
            evs.last(),
            Some(Event::Pulled(gitcore::PullOutcome::Rebased))
        ));
        assert!(work.join("theirs.txt").exists());
    }

    #[test]
    fn rejected_push_and_switch_with_stash() {
        let Some((_tmp, work)) = remote_env() else {
            return;
        };
        let server = mockito::Server::new();
        let w = start(&server, Arc::new(MemoryStore::default()), "");
        w.send(Command::OpenRepo(work.clone()));
        until(&w, |e| {
            matches!(
                e,
                Event::SyncFinished {
                    op: SyncOp::Fetch,
                    ..
                }
            )
        });
        push_from_other(&work, "theirs.txt");
        std::fs::write(work.join("mine.txt"), "mine\n").unwrap();
        git(&work, &["add", "mine.txt"]);
        git(&work, &["commit", "-q", "-m", "mine"]);
        w.send(Command::Push(gitcore::PushMode::Normal));
        until(&w, |e| matches!(e, Event::PushRejected));

        w.send(Command::CreateBranch {
            name: "side".into(),
            switch: false,
        });
        until(
            &w,
            |e| matches!(e, Event::BranchesLoaded(b) if b.iter().any(|b| b.name == "side")),
        );
        git(&work, &["switch", "-q", "side"]);
        std::fs::write(work.join("README.md"), "on side\n").unwrap();
        git(&work, &["commit", "-q", "-am", "side edit"]);
        git(&work, &["switch", "-q", "main"]);
        std::fs::write(work.join("README.md"), "local edit\n").unwrap();
        w.send(Command::SwitchBranch {
            name: "side".into(),
            stash: false,
        });
        until(
            &w,
            |e| matches!(e, Event::WouldOverwrite { files, .. } if files == &vec!["README.md".to_string()]),
        );
        std::fs::write(work.join("new-untracked.txt"), "keep me\n").unwrap();
        std::fs::write(work.join("README.md"), "hello\n").unwrap(); // non-conflicting now
        w.send(Command::SwitchBranch {
            name: "side".into(),
            stash: true,
        });
        until(
            &w,
            |e| matches!(e, Event::RepoOpened(s) if s.head == gitcore::Head::Branch("side".into())),
        );
        assert_eq!(
            std::fs::read_to_string(work.join("new-untracked.txt")).unwrap(),
            "keep me\n"
        );
    }
}
```

- [ ] **Step 3: Write the failing askpass test**

Runs the real binary (`CARGO_BIN_EXE_retrogit`) the way git would.

`crates/app/tests/askpass.rs`:

```rust
//! The binary answers git's credential prompts when started with `--askpass`.

use std::process::Command;

fn ask(prompt: &str) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_retrogit"))
        .args(["--askpass", prompt])
        .env(gitcore::ASKPASS_TOKEN_VAR, "gho_test_token")
        .output()
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(out.status.success());
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[test]
fn askpass_mode_answers_username_then_token() {
    assert_eq!(ask("Username for 'https://github.com': "), "x-access-token");
    assert_eq!(
        ask("Password for 'https://x-access-token@github.com': "),
        "gho_test_token"
    );
}
```

- [ ] **Step 4: Run the tests to see them fail**

Run: `cargo test -p retrogit --tests`

Expected: compile errors: unknown `Event::LogLoaded`, `Command::Pull`, `state::Tab`, `branch_name_error`, etc.

- [ ] **Step 5: Extend the protocol**

Replace the whole file with the version below (it keeps everything from sub-projects 1 and 2).

`crates/app/src/protocol.rs`:

```rust
//! Messages between the UI thread and the worker thread.

use std::path::PathBuf;

use gitcore::{
    Branch, CloneProgress, CommitDetail, CommitOutcome, FileDiff, FileStatus, GitError, LogEntry,
    NetProgress, Operation, PullMode, PullOutcome, PushMode, RepoSummary, Selection, Side,
    SignatureStatus, SigningConfig,
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
    /// Revert unstaged working-tree changes (needs a confirmation in the UI).
    Discard {
        path: String,
        selection: Selection,
        shown: Option<FileDiff>,
    },
    /// Whole files back to their index version; untracked files go to the trash.
    DiscardFiles(Vec<String>),
    /// Whole files, one index operation and one refresh (Stage all / Unstage all).
    StageFiles(Vec<String>),
    UnstageFiles(Vec<String>),
    Commit {
        message: String,
        amend: bool,
    },
    AddToGitignore(String),
    /// Reply: `Event::AmendInfo`.
    LoadAmendInfo,
    // --- Sub-project 3 ---
    /// Next page of history (`skip` = number of entries already loaded).
    LoadLog {
        skip: usize,
    },
    LoadCommit(String),
    LoadCommitFileDiff {
        id: String,
        path: String,
    },
    LoadBranches,
    CreateBranch {
        name: String,
        switch: bool,
    },
    /// Local branch, or `origin/x` (creates the tracking branch). `stash`: put local changes
    /// aside, switch, then re-apply them.
    SwitchBranch {
        name: String,
        stash: bool,
    },
    RenameBranch {
        old: String,
        new: String,
    },
    DeleteBranch {
        name: String,
        force: bool,
    },
    /// `background`: automatic fetch at open (errors are only logged).
    Fetch {
        background: bool,
    },
    Pull(PullMode),
    Push(PushMode),
    AbortOperation,
    ContinueRebase,
}

/// Network operation shown in the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncOp {
    Fetch,
    Pull,
    Push,
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
    /// History, branches.
    History,
    Sync,
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
    /// A page of history; `skip` tells where it goes.
    LogLoaded {
        skip: usize,
        entries: Vec<LogEntry>,
    },
    CommitLoaded(CommitDetail),
    SignatureLoaded {
        id: String,
        status: SignatureStatus,
    },
    CommitFileDiffLoaded {
        id: String,
        diff: FileDiff,
    },
    BranchesLoaded(Vec<Branch>),
    /// Merge/rebase in progress in the open repository.
    OperationChanged(Option<Operation>),
    SigningLoaded(Option<SigningConfig>),
    SyncStarted {
        op: SyncOp,
        background: bool,
    },
    SyncProgress(NetProgress),
    /// Network operation finished (successfully, or with an `Error` sent just before).
    SyncFinished {
        op: SyncOp,
        ok: bool,
    },
    Pulled(PullOutcome),
    /// Pull needs a decision: Merge or Rebase.
    Diverged {
        ahead: usize,
        behind: usize,
    },
    PushRejected,
    /// Switching is blocked by local changes to these files.
    WouldOverwrite {
        branch: String,
        files: Vec<String>,
    },
    NotMerged(String),
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
            GitError::WouldOverwrite { files } => {
                AppError::new(Severity::Warning, s::ERR_WOULD_OVERWRITE)
                    .with_detail(files.join("\n"))
            }
            GitError::NotMerged(b) => AppError::new(
                Severity::Warning,
                &format!("Branch '{b}' is not fully merged."),
            ),
            GitError::Diverged { ahead, behind } => AppError::new(
                Severity::Info,
                &format!("Your branch and its upstream have diverged (↑{ahead} ↓{behind})."),
            ),
            GitError::PushRejected => AppError::new(Severity::Warning, s::ERR_PUSH_REJECTED),
            GitError::StashConflict => AppError::new(Severity::Warning, s::ERR_STASH_CONFLICT),
            GitError::SigningRequiresGit => {
                AppError::new(Severity::Error, s::ERR_SIGNING_REQUIRES_GIT)
            }
            GitError::GitMissing => AppError::new(Severity::Warning, s::ERR_GIT_MISSING),
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

- [ ] **Step 6: Route the new events in the reducer**

Replace the whole file with the version below (it keeps everything from sub-projects 1 and 2). New events go to `apply_sync` (in `state/sync.rs`); `switch_repo` also resets the S3 views.

`crates/app/src/state.rs`:

```rust
//! All UI state, updated by the pure `apply` function.

mod sync;

pub use sync::{HistoryView, LOG_PAGE, PendingDialog, SyncView, Tab, branch_name_error};

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
    /// Discard waiting for the user's confirmation.
    pub pending_discard: Option<PendingDiscard>,
}

/// A destructive command and the question shown before running it.
#[derive(Debug, Clone, PartialEq)]
pub struct PendingDiscard {
    pub question: String,
    pub command: crate::protocol::Command,
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

    pub fn request_discard(&mut self, command: crate::protocol::Command, question: String) {
        self.pending_discard = Some(PendingDiscard { question, command });
    }

    pub fn cancel_discard(&mut self) {
        self.pending_discard = None;
    }

    /// The confirmed command, once.
    pub fn confirm_discard(&mut self) -> Option<crate::protocol::Command> {
        self.pending_discard.take().map(|p| p.command)
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
    // --- Sub-project 3 ---
    pub tab: Tab,
    pub history: HistoryView,
    pub branches: Vec<gitcore::Branch>,
    pub sync: SyncView,
    /// At most one sub-project 3 dialog at a time.
    pub dialog: Option<PendingDialog>,
    /// HEAD was amended after being pushed: a rejected push may offer force-with-lease.
    pub amended_pushed: bool,
    pub operation: Option<gitcore::Operation>,
    pub signing: Option<gitcore::SigningConfig>,
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
            tab: Tab::default(),
            history: HistoryView::default(),
            branches: Vec::new(),
            sync: SyncView::default(),
            dialog: None,
            amended_pushed: false,
            operation: None,
            signing: None,
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
                    // Background refreshes re-send the same diff: keep the user's selection.
                    if c.diff.as_ref() != Some(&diff) {
                        c.diff = Some(diff);
                        c.selected_lines.clear();
                        c.show_large = false;
                    }
                }
            }
            Event::Committed(outcome) => {
                if self.changes.amend && self.changes.head_pushed {
                    self.amended_pushed = true;
                }
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
            ev @ (Event::LogLoaded { .. }
            | Event::CommitLoaded(_)
            | Event::SignatureLoaded { .. }
            | Event::CommitFileDiffLoaded { .. }
            | Event::BranchesLoaded(_)
            | Event::OperationChanged(_)
            | Event::SigningLoaded(_)
            | Event::SyncStarted { .. }
            | Event::SyncProgress(_)
            | Event::SyncFinished { .. }
            | Event::Pulled(_)
            | Event::Diverged { .. }
            | Event::PushRejected
            | Event::WouldOverwrite { .. }
            | Event::NotMerged(_)) => self.apply_sync(ev),
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
            Op::History => self.history.loading = false,
            Op::Sync => {
                self.sync.running = None;
                self.sync.progress = None;
            }
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
            self.history = HistoryView::default();
            self.branches.clear();
            self.sync = SyncView::default();
            self.dialog = None;
            self.amended_pushed = false;
            self.operation = None;
            self.signing = None;
        }
        self.current = Some(summary);
    }

    fn remember(&mut self, summary: &RepoSummary) {
        self.config.add_recent(&summary.name, &summary.path);
        self.missing.remove(&summary.path);
        self.config_dirty = true;
    }

    /// Recent repositories as shown in the side list: alphabetical by name
    /// (case-insensitive), then by path. Opening a repo never reorders it.
    pub fn recents_sorted(&self) -> Vec<crate::config::RecentRepo> {
        let mut list = self.config.recent.clone();
        list.sort_by(|a, b| {
            a.name
                .to_lowercase()
                .cmp(&b.name.to_lowercase())
                .then_with(|| a.path.cmp(&b.path))
        });
        list
    }

    /// Row of `recents_sorted()` holding the open repository.
    pub fn selected_recent(&self) -> Option<usize> {
        let current = self.current.as_ref()?;
        self.recents_sorted()
            .iter()
            .position(|r| r.path == current.path)
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

- [ ] **Step 7: Add the S3 state**

`crates/app/src/state/sync.rs`:

```rust
//! State for sub-project 3: history, branches, network operations and their dialogs.

use gitcore::{
    CommitDetail, FileDiff, GraphRow, GraphState, LogEntry, NetProgress, PullOutcome,
    SignatureStatus,
};

use super::AppState;
use crate::protocol::{AppError, Event, Severity, SyncOp};
use crate::strings as s;

/// History page size.
pub const LOG_PAGE: usize = 500;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Tab {
    #[default]
    Changes,
    History,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HistoryView {
    pub entries: Vec<LogEntry>,
    pub graph: Vec<GraphRow>,
    pub graph_state: GraphState,
    /// A page request is in flight.
    pub loading: bool,
    /// The last page was shorter than `LOG_PAGE`.
    pub end_reached: bool,
    pub selected: Option<String>,
    pub detail: Option<CommitDetail>,
    pub signature: Option<SignatureStatus>,
    pub detail_file: Option<String>,
    pub detail_diff: Option<FileDiff>,
    /// Share of the height given to the commit list.
    pub split: f32,
}

impl Default for HistoryView {
    fn default() -> Self {
        HistoryView {
            entries: Vec::new(),
            graph: Vec::new(),
            graph_state: GraphState::default(),
            loading: false,
            end_reached: false,
            selected: None,
            detail: None,
            signature: None,
            detail_file: None,
            detail_diff: None,
            split: 0.55,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SyncView {
    pub running: Option<SyncOp>,
    /// Automatic fetch: no modal progress, errors only logged.
    pub background: bool,
    pub progress: Option<NetProgress>,
    /// Status bar note after the last operation, e.g. "Pushed".
    pub note: Option<String>,
}

/// Dialogs of sub-project 3 (the UI shows at most one).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PendingDialog {
    NewBranch { name: String, switch: bool },
    RenameBranch { old: String, name: String },
    DeleteBranch { name: String },
    DeleteNotMerged { name: String },
    Diverged { ahead: usize, behind: usize },
    PushRejected { can_force: bool },
    ConfirmForcePush,
    WouldOverwrite { branch: String, files: Vec<String> },
}

impl AppState {
    pub(super) fn apply_sync(&mut self, event: Event) {
        match event {
            Event::LogLoaded { skip, entries } => {
                let h = &mut self.history;
                h.loading = false;
                if skip == 0 {
                    h.entries.clear();
                    h.graph.clear();
                    h.graph_state = GraphState::default();
                } else if skip != h.entries.len() {
                    return; // stale page (history was reloaded meanwhile)
                }
                h.end_reached = entries.len() < LOG_PAGE;
                h.graph.extend(h.graph_state.layout_more(&entries));
                h.entries.extend(entries);
                if h.selected
                    .as_ref()
                    .is_some_and(|id| !h.entries.iter().any(|e| &e.id == id))
                    && h.end_reached
                {
                    h.selected = None;
                    h.detail = None;
                }
            }
            Event::CommitLoaded(detail) => {
                let h = &mut self.history;
                if h.selected.as_deref() == Some(detail.id.as_str()) {
                    h.detail = Some(detail);
                    h.signature = None;
                    h.detail_file = None;
                    h.detail_diff = None;
                }
            }
            Event::SignatureLoaded { id, status } => {
                if self.history.selected.as_deref() == Some(id.as_str()) {
                    self.history.signature = Some(status);
                }
            }
            Event::CommitFileDiffLoaded { id, diff } => {
                let h = &mut self.history;
                if h.selected.as_deref() == Some(id.as_str())
                    && h.detail_file.as_deref() == Some(diff.path.as_str())
                {
                    h.detail_diff = Some(diff);
                }
            }
            Event::BranchesLoaded(branches) => self.branches = branches,
            Event::OperationChanged(op) => self.operation = op,
            Event::SigningLoaded(cfg) => self.signing = cfg,
            Event::SyncStarted { op, background } => {
                self.sync.running = Some(op);
                self.sync.background = background;
                self.sync.progress = None;
            }
            Event::SyncProgress(p) => self.sync.progress = Some(p),
            Event::SyncFinished { op, ok } => {
                self.sync.running = None;
                self.sync.progress = None;
                if ok {
                    self.sync.note = Some(
                        match op {
                            SyncOp::Fetch => s::FETCHED,
                            SyncOp::Pull => s::PULLED,
                            SyncOp::Push => s::PUSHED,
                        }
                        .to_string(),
                    );
                    if op == SyncOp::Push {
                        self.amended_pushed = false;
                    }
                }
            }
            Event::Pulled(outcome) => {
                self.sync.note = Some(
                    match outcome {
                        PullOutcome::UpToDate => s::UP_TO_DATE,
                        _ => s::PULLED,
                    }
                    .to_string(),
                );
                if outcome == PullOutcome::Conflicts {
                    self.tab = Tab::Changes;
                    self.messages
                        .push_back(AppError::new(Severity::Info, s::INFO_CONFLICTS));
                }
            }
            Event::Diverged { ahead, behind } => {
                self.dialog = Some(PendingDialog::Diverged { ahead, behind })
            }
            Event::PushRejected => {
                self.dialog = Some(PendingDialog::PushRejected {
                    can_force: self.amended_pushed,
                });
            }
            Event::WouldOverwrite { branch, files } => {
                self.dialog = Some(PendingDialog::WouldOverwrite { branch, files });
            }
            Event::NotMerged(name) => self.dialog = Some(PendingDialog::DeleteNotMerged { name }),
            _ => {}
        }
    }

    /// Select a commit in the history (the UI then asks the worker for its detail).
    pub fn select_commit(&mut self, id: &str) {
        let h = &mut self.history;
        if h.selected.as_deref() != Some(id) {
            h.selected = Some(id.to_string());
            h.detail = None;
            h.signature = None;
            h.detail_file = None;
            h.detail_diff = None;
        }
    }

    /// Whether the history list should ask for its next page.
    pub fn wants_more_history(&self) -> bool {
        !self.history.loading && !self.history.end_reached && !self.history.entries.is_empty()
    }
}

/// Pure check of a new branch name against Git's rules (`git check-ref-format --branch`)
/// and the existing local branches.
pub fn branch_name_error(name: &str, existing: &[gitcore::Branch]) -> Option<String> {
    let n = name.trim();
    if n.is_empty() {
        return Some("Enter a branch name.".into());
    }
    let bad_char = n
        .chars()
        .any(|c| c.is_whitespace() || c.is_control() || "~^:?*[\\".contains(c));
    let bad = bad_char
        || n.starts_with('-')
        || n.starts_with('/')
        || n.ends_with('/')
        || n.ends_with('.')
        || n.ends_with(".lock")
        || n.contains("..")
        || n.contains("//")
        || n.contains("@{")
        || n == "@"
        || n.split('/').any(|part| part.starts_with('.'));
    if bad {
        return Some(format!("'{n}' is not a valid branch name."));
    }
    if existing.iter().any(|b| !b.remote && b.name == n) {
        return Some(format!("A branch named '{n}' already exists."));
    }
    None
}
```

- [ ] **Step 8: Route the new commands in the worker**

Replace the whole file with the version below (it keeps everything from sub-projects 1 and 2).

`crates/app/src/worker.rs`:

```rust
//! The single background thread doing all network and Git work.

mod changes;
mod sync;

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
    cancel_net: Arc<AtomicBool>,
}

impl WorkerHandle {
    pub fn send(&self, cmd: Command) {
        match cmd {
            Command::StartDeviceFlow => self.cancel_flow.store(false, Ordering::SeqCst),
            Command::Clone { .. } => self.cancel_clone.store(false, Ordering::SeqCst),
            Command::Fetch { .. } | Command::Pull(_) | Command::Push(_) => {
                self.cancel_net.store(false, Ordering::SeqCst)
            }
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

    /// Stop the running fetch / pull / push (kills the git process).
    pub fn cancel_network(&self) {
        self.cancel_net.store(true, Ordering::SeqCst);
    }

    /// Cancel whatever is running and wait (up to `timeout`) for the worker to be idle,
    /// so a clone interrupted by quitting still removes its partial folder.
    /// Returns `true` if the worker became idle in time.
    pub fn shutdown(&self, timeout: Duration) -> bool {
        self.cancel_device_flow();
        self.cancel_clone();
        self.cancel_network();
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
    let cancel_net = Arc::new(AtomicBool::new(false));
    let mut worker = Worker {
        cancel_net: cancel_net.clone(),
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
        cancel_net,
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
    cancel_net: Arc<AtomicBool>,
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
            Command::Discard {
                path,
                selection,
                shown,
            } => self.discard(&path, &selection, shown.as_ref()),
            Command::DiscardFiles(paths) => self.discard_files(&paths),
            Command::StageFiles(paths) => self.stage_files(&paths, true),
            Command::UnstageFiles(paths) => self.stage_files(&paths, false),
            Command::Commit { message, amend } => self.commit(&message, amend),
            Command::AddToGitignore(pattern) => self.add_to_gitignore(&pattern),
            Command::LoadAmendInfo => self.amend_info(),
            Command::LoadLog { skip } => self.load_log(skip),
            Command::LoadCommit(id) => self.load_commit(&id),
            Command::LoadCommitFileDiff { id, path } => self.load_commit_file_diff(&id, &path),
            Command::LoadBranches => self.load_branches(),
            Command::CreateBranch { name, switch } => self.create_branch(&name, switch),
            Command::SwitchBranch { name, stash } => self.switch_branch(&name, stash),
            Command::RenameBranch { old, new } => self.rename_branch(&old, &new),
            Command::DeleteBranch { name, force } => self.delete_branch(&name, force),
            Command::Fetch { background } => self.fetch(background),
            Command::Pull(mode) => self.pull(mode),
            Command::Push(mode) => self.push(mode),
            Command::AbortOperation => self.abort_operation(),
            Command::ContinueRebase => self.continue_rebase(),
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

- [ ] **Step 9: Send S3 data when a repo opens; resync after commits**

Replace the whole file with the version below (it keeps everything from sub-projects 1 and 2). `opened` now also loads signing/branches/history and starts the background fetch for a newly opened repo; `refresh` emits `OperationChanged`; `commit` calls `after_ref_change`.

`crates/app/src/worker/changes.rs`:

```rust
//! Worker side of sub-project 2: status, diff, staging, commit, .gitignore.

use gitcore::{FileDiff, Repo, RepoSummary, Selection, Side};

use super::Worker;
use crate::protocol::{AppError, Event, Op};

impl Worker {
    /// A repository was opened or cloned: it becomes the target of later commands.
    pub(super) fn opened(&mut self, summary: RepoSummary, cloned: bool) {
        let new_repo = self.repo.as_ref() != Some(&summary.path);
        if new_repo {
            self.shown = None;
        }
        self.repo = Some(summary.path.clone());
        self.emit(if cloned {
            Event::CloneDone(summary)
        } else {
            Event::RepoOpened(summary)
        });
        self.refresh();
        let Some(repo) = self.open_current(Op::History) else {
            return;
        };
        self.load_repo_extras(&repo);
        if new_repo && !cloned {
            self.auto_fetch(&repo);
        }
    }

    pub(super) fn open_current(&self, during: Op) -> Option<Repo> {
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
        self.emit(Event::OperationChanged(repo.operation_in_progress()));
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

    pub(super) fn discard(&mut self, path: &str, selection: &Selection, shown: Option<&FileDiff>) {
        let Some(repo) = self.open_current(Op::Changes) else {
            return;
        };
        if let Err(e) = repo.discard(path, selection, shown) {
            self.fail(Op::Changes, AppError::from_git(&e));
        }
        self.refresh();
    }

    pub(super) fn discard_files(&mut self, paths: &[String]) {
        let Some(repo) = self.open_current(Op::Changes) else {
            return;
        };
        let refs: Vec<&str> = paths.iter().map(String::as_str).collect();
        if let Err(e) = repo.discard_files(&refs) {
            self.fail(Op::Changes, AppError::from_git(&e));
        }
        self.refresh();
    }

    pub(super) fn stage_files(&mut self, paths: &[String], stage: bool) {
        let Some(repo) = self.open_current(Op::Changes) else {
            return;
        };
        let refs: Vec<&str> = paths.iter().map(String::as_str).collect();
        let result = if stage {
            repo.stage_files(&refs)
        } else {
            repo.unstage_files(&refs)
        };
        if let Err(e) = result {
            self.fail(Op::Changes, AppError::from_git(&e));
        }
        self.refresh();
    }

    pub(super) fn commit(&mut self, message: &str, amend: bool) {
        let Some(repo) = self.open_current(Op::Commit) else {
            return;
        };
        match repo.commit(message, amend, self.deps.commit_backend) {
            Ok(outcome) => self.emit(Event::Committed(outcome)),
            Err(e) => self.fail(Op::Commit, AppError::from_git(&e)),
        }
        self.after_ref_change(&repo);
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

- [ ] **Step 10: Implement the worker side of history, branches and sync**

`crates/app/src/worker/sync.rs`:

```rust
//! Worker side of sub-project 3: history, branches, fetch / pull / push.

use std::time::{Duration, Instant};

use gitcore::{GitError, NetAuth, PullMode, PushMode, Repo};

use super::{Throttle, Worker};
use crate::protocol::{AppError, Event, Op, SyncOp};
use crate::state::LOG_PAGE;
use crate::strings as s;

impl Worker {
    fn net_auth(&self) -> NetAuth {
        NetAuth {
            github_token: self.token.clone(),
        }
    }

    /// HEAD or refs changed: resend everything that depends on them.
    pub(super) fn after_ref_change(&mut self, repo: &Repo) {
        if let Ok(summary) = repo.summary() {
            self.emit(Event::RepoOpened(summary));
        }
        self.load_branches();
        self.load_log(0);
        self.refresh();
    }

    /// Everything the History tab and toolbar need for a freshly opened repo.
    pub(super) fn load_repo_extras(&mut self, repo: &Repo) {
        self.emit(Event::SigningLoaded(repo.signing_config().ok()));
        self.load_branches();
        self.load_log(0);
    }

    pub(super) fn load_log(&mut self, skip: usize) {
        let Some(repo) = self.open_current(Op::History) else {
            return;
        };
        match repo.log(skip, LOG_PAGE) {
            Ok(entries) => self.emit(Event::LogLoaded { skip, entries }),
            Err(e) => self.fail(Op::History, AppError::from_git(&e)),
        }
    }

    pub(super) fn load_commit(&mut self, id: &str) {
        let Some(repo) = self.open_current(Op::History) else {
            return;
        };
        match repo.commit_detail(id) {
            Ok(detail) => self.emit(Event::CommitLoaded(detail)),
            Err(e) => return self.fail(Op::History, AppError::from_git(&e)),
        }
        // Slow (runs gpg): sent separately after the detail.
        let status = repo
            .signature_status(id)
            .unwrap_or(gitcore::SignatureStatus::Unknown);
        self.emit(Event::SignatureLoaded {
            id: id.to_string(),
            status,
        });
    }

    pub(super) fn load_commit_file_diff(&mut self, id: &str, path: &str) {
        let Some(repo) = self.open_current(Op::History) else {
            return;
        };
        match repo.commit_file_diff(id, path) {
            Ok(diff) => self.emit(Event::CommitFileDiffLoaded {
                id: id.to_string(),
                diff,
            }),
            Err(e) => self.fail(Op::History, AppError::from_git(&e)),
        }
    }

    pub(super) fn load_branches(&mut self) {
        let Some(repo) = self.open_current(Op::History) else {
            return;
        };
        match repo.branches() {
            Ok(b) => self.emit(Event::BranchesLoaded(b)),
            Err(e) => self.fail(Op::History, AppError::from_git(&e)),
        }
    }

    /// Run a branch command, then resync; `WouldOverwrite` / `NotMerged` become dialogs.
    fn branch_op(&mut self, run: impl FnOnce(&Repo) -> Result<(), GitError>, branch: &str) {
        let Some(repo) = self.open_current(Op::History) else {
            return;
        };
        match run(&repo) {
            Ok(()) => {}
            Err(GitError::WouldOverwrite { files }) => {
                self.emit(Event::WouldOverwrite {
                    branch: branch.to_string(),
                    files,
                });
            }
            Err(GitError::NotMerged(name)) => self.emit(Event::NotMerged(name)),
            Err(e) => self.fail(Op::History, AppError::from_git(&e)),
        }
        self.after_ref_change(&repo);
    }

    pub(super) fn create_branch(&mut self, name: &str, switch: bool) {
        let name = name.trim().to_string();
        self.branch_op(|r| r.create_branch(&name, switch), &name.clone());
    }

    pub(super) fn switch_branch(&mut self, name: &str, stash: bool) {
        let remote = name.starts_with("origin/");
        let target = name.to_string();
        self.branch_op(
            |r| {
                let switch = |r: &Repo| {
                    if remote {
                        r.checkout_remote_branch(&target)
                    } else {
                        r.switch_branch(&target)
                    }
                };
                if !stash {
                    return switch(r);
                }
                let stashed = r.stash_push(&format!("RetroGit: switch to {target}"))?;
                if let Err(e) = switch(r) {
                    if stashed {
                        let _ = r.stash_pop(); // put the changes back where they were
                    }
                    return Err(e);
                }
                if stashed { r.stash_pop() } else { Ok(()) }
            },
            name,
        );
    }

    pub(super) fn rename_branch(&mut self, old: &str, new: &str) {
        let new = new.trim().to_string();
        self.branch_op(|r| r.rename_branch(old, &new), old);
    }

    pub(super) fn delete_branch(&mut self, name: &str, force: bool) {
        self.branch_op(|r| r.delete_branch(name, force), name);
    }

    pub(super) fn abort_operation(&mut self) {
        self.branch_op(|r| r.abort_operation(), "");
    }

    pub(super) fn continue_rebase(&mut self) {
        let Some(repo) = self.open_current(Op::Commit) else {
            return;
        };
        if let Err(e) = repo.continue_rebase() {
            self.fail(Op::Commit, AppError::from_git(&e));
        }
        self.after_ref_change(&repo);
    }

    /// Common shape of fetch / pull / push: start event, throttled progress, finish event.
    fn network<T>(
        &mut self,
        op: SyncOp,
        background: bool,
        run: impl FnOnce(
            &Repo,
            &NetAuth,
            &mut dyn FnMut(gitcore::NetProgress),
            &std::sync::atomic::AtomicBool,
        ) -> Result<T, GitError>,
    ) -> Option<(Repo, Result<T, GitError>)> {
        let repo = self.open_current(Op::Sync)?;
        self.emit(Event::SyncStarted { op, background });
        let auth = self.net_auth();
        let mut throttle = Throttle::new(Duration::from_millis(100));
        let emit = &self.emit;
        let mut on_progress = |p: gitcore::NetProgress| {
            if throttle.ready(Instant::now()) {
                emit(Event::SyncProgress(p));
            }
        };
        let result = run(&repo, &auth, &mut on_progress, &self.cancel_net);
        Some((repo, result))
    }

    /// Report a network failure (background fetches only log it).
    fn net_failed(&mut self, op: SyncOp, background: bool, e: &GitError) {
        if background {
            log::info!("background fetch failed: {e}");
        } else {
            let mut error = match e {
                GitError::Cancelled => AppError::new(crate::protocol::Severity::Info, s::CANCELLED),
                GitError::Auth(detail) if detail.contains("401") => {
                    AppError::from_github(&github::GithubError::Unauthorized)
                }
                _ => AppError::from_git(e),
            };
            if matches!(e, GitError::Auth(_)) {
                error.message = format!("{}\n\n{}", s::ERR_NET_AUTH_HELP, error.message);
            }
            self.fail(Op::Sync, error);
        }
        self.emit(Event::SyncFinished { op, ok: false });
    }

    pub(super) fn fetch(&mut self, background: bool) {
        let Some((repo, result)) =
            self.network(SyncOp::Fetch, background, |r, a, p, c| r.fetch(a, p, c))
        else {
            return;
        };
        match result {
            Ok(()) => self.emit(Event::SyncFinished {
                op: SyncOp::Fetch,
                ok: true,
            }),
            Err(e) => self.net_failed(SyncOp::Fetch, background, &e),
        }
        self.load_branches();
        self.load_log(0);
        drop(repo);
    }

    pub(super) fn pull(&mut self, mode: PullMode) {
        let Some((repo, result)) =
            self.network(SyncOp::Pull, false, |r, a, p, c| r.pull(a, mode, p, c))
        else {
            return;
        };
        match result {
            Ok(outcome) => {
                self.emit(Event::Pulled(outcome));
                self.emit(Event::SyncFinished {
                    op: SyncOp::Pull,
                    ok: true,
                });
            }
            Err(GitError::Diverged { ahead, behind }) => {
                self.emit(Event::SyncFinished {
                    op: SyncOp::Pull,
                    ok: false,
                });
                self.emit(Event::Diverged { ahead, behind });
            }
            Err(e) => self.net_failed(SyncOp::Pull, false, &e),
        }
        self.after_ref_change(&repo);
    }

    pub(super) fn push(&mut self, mode: PushMode) {
        let Some((repo, result)) =
            self.network(SyncOp::Push, false, |r, a, p, c| r.push(a, mode, p, c))
        else {
            return;
        };
        match result {
            Ok(()) => self.emit(Event::SyncFinished {
                op: SyncOp::Push,
                ok: true,
            }),
            Err(GitError::PushRejected) => {
                self.emit(Event::SyncFinished {
                    op: SyncOp::Push,
                    ok: false,
                });
                self.emit(Event::PushRejected);
            }
            Err(e) => self.net_failed(SyncOp::Push, false, &e),
        }
        self.after_ref_change(&repo);
    }

    /// Fetch once when a repo is opened, unless it would need credentials we don't have.
    pub(super) fn auto_fetch(&mut self, repo: &Repo) {
        let url = repo
            .summary()
            .ok()
            .and_then(|s| s.origin_url)
            .unwrap_or_default();
        if url.is_empty() || (url.starts_with("https://github.com/") && self.token.is_none()) {
            return;
        }
        self.fetch(true);
    }
}
```

- [ ] **Step 11: Log panics**

Replace the whole file with the version below (it keeps everything from sub-projects 1 and 2).

`crates/app/src/logging.rs`:

```rust
//! Tiny file logger that never writes a token in clear.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

pub const MAX_LOG_BYTES: u64 = 5 * 1024 * 1024;

const TOKEN_PREFIXES: &[&str] = &["github_pat_", "ghp_", "gho_", "ghu_", "ghs_", "ghr_"];

/// Mask known secrets and anything that looks like a GitHub token.
pub fn redact(text: &str, secrets: &[String]) -> String {
    let mut out = text.to_string();
    for s in secrets.iter().filter(|s| !s.is_empty()) {
        out = out.replace(s.as_str(), "***");
    }
    for prefix in TOKEN_PREFIXES {
        let mut result = String::with_capacity(out.len());
        let mut rest = out.as_str();
        while let Some(i) = rest.find(prefix) {
            result.push_str(&rest[..i]);
            let after = &rest[i + prefix.len()..];
            let len = after
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .unwrap_or(after.len());
            if len == 0 {
                result.push_str(prefix);
            } else {
                result.push_str("***");
            }
            rest = &after[len..];
        }
        result.push_str(rest);
        out = result;
    }
    out
}

/// Open the log for appending, truncating it first if it grew past `max_bytes`.
pub fn open_log_file(path: &Path, max_bytes: u64) -> std::io::Result<File> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let too_big = std::fs::metadata(path)
        .map(|m| m.len() > max_bytes)
        .unwrap_or(false);
    OpenOptions::new()
        .create(true)
        .append(!too_big)
        .write(true)
        .truncate(too_big)
        .open(path)
}

struct FileLogger {
    file: Mutex<File>,
}

static SECRETS: Mutex<Vec<String>> = Mutex::new(Vec::new());
static LOGGER: OnceLock<FileLogger> = OnceLock::new();

/// Register a secret that must never reach the log (e.g. the current token).
pub fn add_secret(secret: &str) {
    if let Ok(mut s) = SECRETS.lock()
        && !s.iter().any(|x| x == secret)
    {
        s.push(secret.to_string());
    }
}

impl log::Log for FileLogger {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.level() <= log::Level::Info
    }

    fn log(&self, record: &log::Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let secrets = SECRETS.lock().map(|s| s.clone()).unwrap_or_default();
        let line = redact(&format!("{}", record.args()), &secrets);
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        if let Ok(mut f) = self.file.lock() {
            let _ = writeln!(
                f,
                "{} {:<5} {}",
                crate::format::format_epoch(secs),
                record.level(),
                line
            );
        }
    }

    fn flush(&self) {
        if let Ok(mut f) = self.file.lock() {
            let _ = f.flush();
        }
    }
}

/// Log every panic (UI or worker thread) with its location, token-shaped text masked.
pub fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let secrets = SECRETS.lock().map(|s| s.clone()).unwrap_or_default();
        log::error!("panic: {}", redact(&panic_text(info), &secrets));
        previous(info);
    }));
}

/// `message at file:line` for a panic.
pub fn panic_text(info: &std::panic::PanicHookInfo<'_>) -> String {
    let payload = info.payload();
    let message = payload
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "(non-text panic)".into());
    match info.location() {
        Some(l) => format!("{message} at {}:{}", l.file(), l.line()),
        None => message,
    }
}

/// Install the file logger. Failing to open the log is not fatal.
pub fn init(path: &Path) {
    let Ok(file) = open_log_file(path, MAX_LOG_BYTES) else {
        return;
    };
    let logger = LOGGER.get_or_init(|| FileLogger {
        file: Mutex::new(file),
    });
    if log::set_logger(logger).is_ok() {
        log::set_max_level(log::LevelFilter::Info);
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn redacts_registered_secrets_and_token_shapes() {
        let secrets = vec!["s3cr3t".to_string()];
        assert_eq!(redact("pw=s3cr3t!", &secrets), "pw=***!");
        assert_eq!(redact("token gho_AbC123 end", &[]), "token *** end");
        assert_eq!(redact("github_pat_11AA_bb", &[]), "***");
        assert_eq!(redact("ghp_", &[]), "ghp_");
        assert_eq!(redact("a ghs_x b ghu_y", &[]), "a *** b ***");
        assert_eq!(redact("nothing here", &[]), "nothing here");
    }

    #[test]
    fn log_file_is_truncated_when_too_big() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("logs").join("retrogit.log");
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, vec![b'x'; 100]).unwrap();
        drop(open_log_file(&p, 1000).unwrap());
        assert_eq!(std::fs::metadata(&p).unwrap().len(), 100);
        drop(open_log_file(&p, 10).unwrap());
        assert_eq!(std::fs::metadata(&p).unwrap().len(), 0);
    }
}
```

- [ ] **Step 12: Askpass mode and panic hook in main**

Replace the whole file with the version below (it keeps everything from sub-projects 1 and 2). `--askpass` is handled before any logging or GUI setup, so git gets its answer immediately.

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
    // `git` runs us as GIT_ASKPASS: answer and exit before any GUI setup.
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--askpass") {
        let prompt = args.get(2).map(String::as_str).unwrap_or("");
        let token = std::env::var(gitcore::ASKPASS_TOKEN_VAR).unwrap_or_default();
        println!("{}", gitcore::askpass_answer(prompt, &token));
        return Ok(());
    }
    if let Some(dir) = dirs::data_local_dir() {
        logging::init(&dir.join("RetroGit").join("retrogit.log"));
    }
    logging::install_panic_hook();
    log::info!("RetroGit {} starting", env!("CARGO_PKG_VERSION"));
    if let Ok(exe) = std::env::current_exe() {
        gitcore::set_askpass_program(exe);
    }
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

- [ ] **Step 13: Run the tests to see them pass**

Run: `cargo test -p retrogit --tests`

Expected: `tests/state.rs` `30 passed`; `tests/worker.rs` `26 passed`; `tests/askpass.rs` `1 passed`.

- [ ] **Step 14: Lint**

Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings`

Expected: no output from fmt; clippy finishes without warnings.

- [ ] **Step 15: Full suite**

Run: `cargo test --workspace`

Expected: all tests pass (0 failed).

- [ ] **Step 16: Commit**

```bash
git add crates/app
git commit -m "feat(app): history, branches and sync in the reducer and worker; askpass mode; panic log"
```


### Task 6: History tab, toolbar, dialogs, merge banner, signing indicator

**Files:**
- Create: `crates/app/src/ui/history.rs`, `crates/app/src/ui/sync_toolbar.rs`, `crates/app/src/ui/sync_dialogs.rs`
- Modify: `crates/app/src/ui/mod.rs`, `crates/app/src/ui/main_window.rs`, `crates/app/src/ui/changes.rs`, `crates/app/src/app.rs`

**Interfaces:**
- Consumes: Task 5 state/commands; `win95::{combo_box, splitter, tabs}`.
- Produces: `ui::history::{show, relative_time, lane_x, lane_color, ROW_HEIGHT, LANE_WIDTH}`; `ui::sync_toolbar::{toolbar, menu_entries, sync_buttons(&AppState) -> SyncButtons, current_branch, remote_only}`; `ui::sync_dialogs::show`; `ui::changes::signing_label(Option<&SigningConfig>) -> (String, Color32)`.

- [ ] **Step 1: Write the failing tests in `crates/app/src/ui/history.rs`**

Create `crates/app/src/ui/history.rs` containing only this test module for now (the implementation goes above it in a later step).

`crates/app/src/ui/history.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_times() {
        assert_eq!(relative_time(1000, 990), "just now");
        assert_eq!(relative_time(10_000, 10_000 - 300), "5m ago");
        assert_eq!(relative_time(100_000, 100_000 - 7_200), "2h ago");
        assert_eq!(relative_time(1_000_000, 1_000_000 - 172_800), "2d ago");
        assert_eq!(relative_time(1_700_000_000, 0), "1970-01-01");
    }

    #[test]
    fn lanes_are_evenly_spaced_and_colors_cycle() {
        assert_eq!(lane_x(0.0, 0), 7.0);
        assert_eq!(lane_x(0.0, 2), 35.0);
        assert_eq!(lane_color(0), lane_color(8));
    }
}
```

- [ ] **Step 2: Write the failing tests in `crates/app/src/ui/sync_toolbar.rs`**

Create `crates/app/src/ui/sync_toolbar.rs` containing only this test module for now (the implementation goes above it in a later step).

`crates/app/src/ui/sync_toolbar.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use gitcore::{Head, RepoSummary};

    fn branch(
        name: &str,
        remote: bool,
        head: bool,
        upstream: Option<&str>,
        ahead: usize,
        behind: usize,
    ) -> Branch {
        Branch {
            name: name.into(),
            remote,
            is_head: head,
            upstream: upstream.map(Into::into),
            ahead,
            behind,
        }
    }

    fn state_with(branches: Vec<Branch>) -> AppState {
        let mut s = AppState::new(Config::default());
        s.current = Some(RepoSummary {
            name: "r".into(),
            path: "/r".into(),
            head: Head::Branch("main".into()),
            origin_url: Some("https://github.com/o/r.git".into()),
            last_commit: None,
        });
        s.branches = branches;
        s
    }

    #[test]
    fn buttons_follow_ahead_behind_and_upstream() {
        let s = state_with(vec![
            branch("main", false, true, Some("origin/main"), 2, 3),
            branch("origin/main", true, false, None, 0, 0),
        ]);
        let b = sync_buttons(&s);
        assert!(b.fetch && b.pull && b.push && !b.publish);
        assert_eq!(
            (b.pull_label.as_str(), b.push_label.as_str()),
            ("Pull ↓3", "Push ↑2")
        );

        let s = state_with(vec![branch("main", false, true, Some("origin/main"), 0, 0)]);
        assert!(!sync_buttons(&s).push, "nothing to push");

        let s = state_with(vec![branch("topic", false, true, None, 0, 0)]);
        let b = sync_buttons(&s);
        assert!(b.publish && b.push && !b.pull);
        assert_eq!(b.push_label, "Publish");
    }

    #[test]
    fn nothing_while_busy_or_detached_or_merging() {
        let mut s = state_with(vec![branch("main", false, true, Some("origin/main"), 1, 1)]);
        s.sync.running = Some(crate::protocol::SyncOp::Fetch);
        let b = sync_buttons(&s);
        assert!(!b.fetch && !b.pull && !b.push);
        s.sync.running = None;
        s.operation = Some(gitcore::Operation::Merge);
        assert!(!sync_buttons(&s).pull);
        let s = state_with(vec![]);
        let b = sync_buttons(&s);
        assert!(!b.pull && !b.push, "detached HEAD");
    }

    #[test]
    fn remote_branches_already_checked_out_are_hidden() {
        let b = vec![
            branch("main", false, true, None, 0, 0),
            branch("origin/main", true, false, None, 0, 0),
            branch("origin/topic", true, false, None, 0, 0),
        ];
        let names: Vec<_> = remote_only(&b)
            .into_iter()
            .map(|b| b.name.as_str())
            .collect();
        assert_eq!(names, ["origin/topic"]);
    }
}
```

- [ ] **Step 3: Declare the screens**

Replace the whole file with the version below (it keeps everything from sub-projects 1 and 2).

`crates/app/src/ui/mod.rs`:

```rust
//! Screens. Each function draws from `AppState` and sends `Command`s to the worker.

pub mod about;
pub mod changes;
pub mod clone_dialog;
pub mod diff_view;
pub mod discard;
pub mod history;
pub mod main_window;
pub mod message;
pub mod sign_in;
pub mod sync_dialogs;
pub mod sync_toolbar;

use crate::state::AppState;
use crate::worker::WorkerHandle;

/// Everything a screen needs.
pub struct Ctx<'a> {
    pub state: &'a mut AppState,
    pub worker: &'a WorkerHandle,
}
```

- [ ] **Step 4: Add the signing indicator test**

Replace the whole file with the version below (it keeps everything from sub-projects 1 and 2). This version already contains `operation_banner`, `signing_label` and its test `signing_label_says_whether_commits_are_signed`; run the next step before Step 7 to see the other new tests fail.

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
        .show(ui, |ui| {
            header(ui, cx);
            operation_banner(ui, cx);
        });
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
        ui.label(RichText::new(format!("{} · {branch}{last}", c.name)).color(win95::theme::NAVY));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.add(Button95::new(s::REFRESH)).clicked() {
                cx.worker.send(Command::RefreshStatus);
            }
        });
    });
    ui.add_space(2.0);
}

/// Yellow banner while a merge or rebase waits for conflict resolution.
fn operation_banner(ui: &mut egui::Ui, cx: &mut Ctx<'_>) {
    let Some(op) = cx.state.operation else { return };
    let (text, abort) = match op {
        gitcore::Operation::Merge => (s::MERGE_IN_PROGRESS, s::ABORT_MERGE),
        gitcore::Operation::Rebase => (s::REBASE_IN_PROGRESS, s::ABORT_REBASE),
    };
    egui::Frame::NONE
        .fill(egui::Color32::from_rgb(0xFF, 0xFF, 0xC0))
        .inner_margin(egui::Margin::same(4))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(text).color(win95::theme::BLACK));
                if ui
                    .add(Button95::new(abort).min_size(egui::vec2(100.0, 20.0)))
                    .clicked()
                {
                    cx.worker.send(Command::AbortOperation);
                }
                if op == gitcore::Operation::Rebase
                    && ui
                        .add(Button95::new(s::CONTINUE_REBASE).min_size(egui::vec2(110.0, 20.0)))
                        .clicked()
                {
                    cx.worker.send(Command::ContinueRebase);
                }
            });
        });
    ui.add_space(2.0);
}

/// "Signed with GPG key ABCD" / "Commits will NOT be signed" for the commit form.
pub fn signing_label(cfg: Option<&gitcore::SigningConfig>) -> (String, egui::Color32) {
    match cfg {
        Some(c) if c.enabled => {
            let kind = match c.format {
                gitcore::SigningFormat::Gpg => "GPG",
                gitcore::SigningFormat::Ssh => "SSH",
                gitcore::SigningFormat::X509 => "X.509",
            };
            let key = c
                .key
                .as_deref()
                .map(|k| format!(" key {k}"))
                .unwrap_or_default();
            (
                format!("🔒 {} {kind}{key}", s::SIGNED_WITH),
                egui::Color32::from_rgb(0, 0x60, 0),
            )
        }
        _ => (
            s::NOT_SIGNED.to_string(),
            egui::Color32::from_rgb(0xA0, 0, 0),
        ),
    }
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

/// One batch command for Stage all / Unstage all (conflicts are skipped, renames include
/// both paths).
pub fn all_files_command(files: &[FileStatus], side: Side) -> Command {
    let paths: Vec<String> = files
        .iter()
        .filter(|f| f.unstaged != Some(Change::Conflicted))
        .flat_map(|f| paths_of(f, side))
        .collect();
    match side {
        Side::Unstaged => Command::StageFiles(paths),
        Side::Staged => Command::UnstageFiles(paths),
    }
}

/// Unstaged files whose changes can be thrown away (not conflicts).
fn discardable(files: &[FileStatus]) -> Vec<FileStatus> {
    files
        .iter()
        .filter(|f| f.unstaged != Some(Change::Conflicted))
        .cloned()
        .collect()
}

fn request_discard_files(cx: &mut Ctx<'_>, files: &[FileStatus]) {
    let question = super::discard::files_question(files);
    let paths = files.iter().map(|f| f.path.clone()).collect();
    cx.state
        .changes
        .request_discard(Command::DiscardFiles(paths), question);
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
                cx.worker.send(all_files_command(files, side));
            }
            if side == Side::Unstaged {
                let discardable = discardable(files);
                let b = Button95::new(s::DISCARD_ALL)
                    .min_size(egui::vec2(80.0, 20.0))
                    .enabled(!discardable.is_empty());
                if ui.add(b).clicked() {
                    request_discard_files(cx, &discardable);
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
                if files[row].unstaged != Some(Change::Conflicted)
                    && ui.button(s::DISCARD_MENU).clicked()
                {
                    menu_action = Some((row, MenuAction::Discard));
                }
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
        Some((row, MenuAction::Discard)) => {
            request_discard_files(cx, std::slice::from_ref(&files[row]))
        }
        None => {}
    }
}

enum MenuAction {
    Toggle,
    Discard,
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
    let signing = cx.state.signing.clone();
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
            ui.horizontal(|ui| {
                let (text, color) = signing_label(signing.as_ref());
                ui.label(egui::RichText::new(text).color(color));
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
    fn signing_label_says_whether_commits_are_signed() {
        let cfg = gitcore::SigningConfig {
            enabled: true,
            format: gitcore::SigningFormat::Gpg,
            key: Some("ABCD1234".into()),
        };
        assert_eq!(
            signing_label(Some(&cfg)).0,
            "🔒 Signed with GPG key ABCD1234"
        );
        let off = gitcore::SigningConfig {
            enabled: false,
            ..cfg
        };
        assert_eq!(signing_label(Some(&off)).0, "Commits will NOT be signed");
        assert_eq!(signing_label(None).0, "Commits will NOT be signed");
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

- [ ] **Step 5: Run the tests to see them fail**

Run: `cargo test -p retrogit --lib`

Expected: compile errors: `relative_time`, `lane_x`, `sync_buttons`, `remote_only` not found.

- [ ] **Step 6: Implement the History tab**

Insert this **above** the existing `#[cfg(test)]` module in `crates/app/src/ui/history.rs`.

The list is virtualised (`show_rows`) and asks for the next page 50 rows before the end; the graph is painted per row from `GraphRow.up/down`; ref labels are small colored boxes. The detail pane shows message, signature (checked asynchronously), files and a read-only diff.

`crates/app/src/ui/history.rs`:

```rust
//! History tab: commit graph list on top, commit detail below.

use egui::{Align2, Color32, Pos2, Rect, RichText, ScrollArea, Sense, Stroke, pos2, vec2};
use gitcore::{GraphRow, LineKind, RefKind, SignatureStatus};
use win95::{Bevel, bevel_frame, splitter};

use super::Ctx;
use crate::format::format_epoch;
use crate::protocol::Command;
use crate::strings as s;

pub const ROW_HEIGHT: f32 = 20.0;
pub const LANE_WIDTH: f32 = 14.0;
/// Load the next page when this close to the end of the list.
const PREFETCH_ROWS: usize = 50;

/// Lane colors (Win95-ish palette, readable on white).
pub const PALETTE: [Color32; 8] = [
    Color32::from_rgb(0x00, 0x00, 0x80),
    Color32::from_rgb(0x80, 0x00, 0x00),
    Color32::from_rgb(0x00, 0x80, 0x00),
    Color32::from_rgb(0x80, 0x00, 0x80),
    Color32::from_rgb(0x00, 0x80, 0x80),
    Color32::from_rgb(0x80, 0x80, 0x00),
    Color32::from_rgb(0xC0, 0x40, 0x00),
    Color32::from_rgb(0x40, 0x40, 0x40),
];

pub fn lane_color(i: usize) -> Color32 {
    PALETTE[i % PALETTE.len()]
}

/// Center x of lane `col` in a graph area starting at `left`.
pub fn lane_x(left: f32, col: usize) -> f32 {
    left + LANE_WIDTH / 2.0 + col as f32 * LANE_WIDTH
}

/// "5m ago", "3h ago", "2d ago", or the date for older commits.
pub fn relative_time(now: i64, t: i64) -> String {
    let d = (now - t).max(0);
    match d {
        0..60 => "just now".into(),
        60..3_600 => format!("{}m ago", d / 60),
        3_600..86_400 => format!("{}h ago", d / 3_600),
        86_400..2_592_000 => format!("{}d ago", d / 86_400),
        _ => format_epoch(t)[..10].to_string(),
    }
}

fn paint_graph(painter: &egui::Painter, row: &GraphRow, left: f32, rect: Rect) {
    let (top, mid, bottom) = (rect.top(), rect.center().y, rect.bottom());
    let line =
        |a: Pos2, b: Pos2, c: usize| painter.line_segment([a, b], Stroke::new(2.0, lane_color(c)));
    for e in &row.up {
        line(
            pos2(lane_x(left, e.from), top),
            pos2(lane_x(left, e.to), mid),
            e.color,
        );
    }
    for e in &row.down {
        line(
            pos2(lane_x(left, e.from), mid),
            pos2(lane_x(left, e.to), bottom),
            e.color,
        );
    }
    let c = pos2(lane_x(left, row.column), mid);
    painter.circle_filled(c, 4.5, lane_color(row.color));
    painter.circle_stroke(c, 4.5, Stroke::new(1.0, Color32::WHITE));
}

pub fn show(ui: &mut egui::Ui, cx: &mut Ctx<'_>) {
    let mut split = cx.state.history.split;
    splitter(
        ui,
        "history_split",
        &mut split,
        cx,
        |ui, cx| list(ui, cx),
        |ui, cx| detail(ui, cx),
    );
    cx.state.history.split = split;
}

fn list(ui: &mut egui::Ui, cx: &mut Ctx<'_>) {
    bevel_frame(ui, Bevel::Field, win95::theme::WHITE, 1, |ui| {
        ui.set_min_size(ui.available_size());
        let h = &cx.state.history;
        if h.entries.is_empty() {
            ui.label(if h.loading {
                s::LOADING_HISTORY
            } else {
                s::NO_HISTORY
            });
            return;
        }
        let lanes = h
            .graph
            .iter()
            .map(GraphRow::width)
            .max()
            .unwrap_or(1)
            .min(12);
        let graph_w = lanes as f32 * LANE_WIDTH + 6.0;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let font = win95::theme::font(win95::theme::FONT_SIZE);
        let mut clicked: Option<String> = None;
        let mut last_visible = 0;
        ScrollArea::vertical()
            .auto_shrink([false, false])
            .show_rows(ui, ROW_HEIGHT, h.entries.len(), |ui, range| {
                ui.spacing_mut().item_spacing.y = 0.0;
                for i in range {
                    last_visible = i;
                    let e = &h.entries[i];
                    let (rect, resp) = ui.allocate_exact_size(
                        vec2(ui.available_width(), ROW_HEIGHT),
                        Sense::click(),
                    );
                    let selected = h.selected.as_deref() == Some(e.id.as_str());
                    let p = ui.painter();
                    if selected {
                        p.rect_filled(rect, 0.0, win95::theme::NAVY);
                    }
                    if let Some(row) = h.graph.get(i) {
                        paint_graph(&p.with_clip_rect(rect), row, rect.left(), rect);
                    }
                    let text_color = if selected {
                        win95::theme::WHITE
                    } else {
                        win95::theme::BLACK
                    };
                    let mut x = rect.left() + graph_w;
                    let y = rect.center().y;
                    let mono = egui::FontId::monospace(win95::theme::FONT_SIZE);
                    let id_rect = p.text(
                        pos2(x, y),
                        Align2::LEFT_CENTER,
                        &e.short_id,
                        mono,
                        text_color,
                    );
                    x = id_rect.right() + 8.0;
                    for r in &e.refs {
                        let (bg, fg) = match r.kind {
                            RefKind::Head => {
                                (Color32::from_rgb(0xFF, 0xFF, 0x80), win95::theme::BLACK)
                            }
                            RefKind::LocalBranch => {
                                (Color32::from_rgb(0xC0, 0xFF, 0xC0), win95::theme::BLACK)
                            }
                            RefKind::RemoteBranch => {
                                (Color32::from_rgb(0xC0, 0xD8, 0xFF), win95::theme::BLACK)
                            }
                            RefKind::Tag => {
                                (Color32::from_rgb(0xFF, 0xD8, 0xA0), win95::theme::BLACK)
                            }
                        };
                        let g = p.layout_no_wrap(r.name.clone(), font.clone(), fg);
                        let tag =
                            Rect::from_min_size(pos2(x, y - 8.0), vec2(g.size().x + 8.0, 16.0));
                        p.rect_filled(tag, 0.0, bg);
                        p.rect_stroke(
                            tag,
                            0.0,
                            Stroke::new(1.0, win95::theme::GRAY),
                            egui::StrokeKind::Inside,
                        );
                        p.galley(pos2(x + 4.0, y - g.size().y / 2.0), g, fg);
                        x = tag.right() + 4.0;
                    }
                    let right = format!("{}  {}", e.author, relative_time(now, e.time));
                    let right_rect = p.text(
                        rect.right_center() - vec2(6.0, 0.0),
                        Align2::RIGHT_CENTER,
                        &right,
                        font.clone(),
                        text_color,
                    );
                    p.with_clip_rect(Rect::from_min_max(
                        pos2(x, rect.top()),
                        pos2(right_rect.left() - 8.0, rect.bottom()),
                    ))
                    .text(
                        pos2(x + 2.0, y),
                        Align2::LEFT_CENTER,
                        &e.summary,
                        font.clone(),
                        text_color,
                    );
                    if resp.clicked() {
                        clicked = Some(e.id.clone());
                    }
                }
            });
        let need_more = last_visible + PREFETCH_ROWS >= cx.state.history.entries.len()
            && cx.state.wants_more_history();
        if need_more {
            cx.state.history.loading = true;
            cx.worker.send(Command::LoadLog {
                skip: cx.state.history.entries.len(),
            });
        }
        if let Some(id) = clicked {
            cx.state.select_commit(&id);
            cx.worker.send(Command::LoadCommit(id));
        }
    });
}

fn signature_text(s: Option<&SignatureStatus>) -> (String, Color32) {
    match s {
        None => (s::SIG_CHECKING.into(), win95::theme::GRAY),
        Some(SignatureStatus::Good { signer }) => (
            format!("🔒 {} {signer}", s::SIG_GOOD),
            Color32::from_rgb(0, 0x80, 0),
        ),
        Some(SignatureStatus::Bad) => (s::SIG_BAD.into(), Color32::from_rgb(0xC0, 0, 0)),
        Some(SignatureStatus::Unknown) => (s::SIG_UNKNOWN.into(), Color32::from_rgb(0x80, 0x60, 0)),
        Some(SignatureStatus::Unsigned) => (s::SIG_UNSIGNED.into(), win95::theme::GRAY),
    }
}

fn detail(ui: &mut egui::Ui, cx: &mut Ctx<'_>) {
    let mut open_file: Option<String> = None;
    bevel_frame(ui, Bevel::Field, win95::theme::WHITE, 4, |ui| {
        ui.set_min_size(ui.available_size());
        let h = &cx.state.history;
        let Some(d) = &h.detail else {
            ui.label(s::SELECT_A_COMMIT);
            return;
        };
        let (sig, sig_color) = signature_text(h.signature.as_ref());
        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new(&d.short_id).font(egui::FontId::monospace(win95::theme::FONT_SIZE)),
            );
            ui.label(format!(
                "· {} <{}> · {}",
                d.author,
                d.email,
                format_epoch(d.time)
            ));
            ui.label(RichText::new(sig).color(sig_color));
        });
        ui.separator();
        ScrollArea::vertical()
            .id_salt("commit_detail")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add(
                    egui::Label::new(RichText::new(&d.message).color(win95::theme::BLACK)).wrap(),
                );
                ui.add_space(6.0);
                ui.label(s::FILES);
                for f in &d.files {
                    let label = super::changes::describe(&f.path, &f.change);
                    let selected = h.detail_file.as_deref() == Some(f.path.as_str());
                    if ui.selectable_label(selected, label).clicked() {
                        open_file = Some(f.path.clone());
                    }
                }
                if let Some(diff) = &h.detail_diff {
                    ui.add_space(6.0);
                    if diff.binary {
                        ui.label(s::BINARY_FILE);
                    }
                    let mono = egui::FontId::monospace(win95::theme::FONT_SIZE);
                    for hunk in &diff.hunks {
                        ui.label(
                            RichText::new(&hunk.header)
                                .font(mono.clone())
                                .color(win95::theme::NAVY),
                        );
                        for l in &hunk.lines {
                            let (sign, bg) = match l.kind {
                                LineKind::Added => ("+", Color32::from_rgb(0xE6, 0xFF, 0xE6)),
                                LineKind::Removed => ("-", Color32::from_rgb(0xFF, 0xE6, 0xE6)),
                                LineKind::Context => (" ", win95::theme::WHITE),
                            };
                            let text = format!("{sign} {}", l.text.trim_end_matches(['\n', '\r']));
                            ui.label(
                                RichText::new(text)
                                    .font(mono.clone())
                                    .color(win95::theme::BLACK)
                                    .background_color(bg),
                            );
                        }
                    }
                }
            });
    });
    if let Some(path) = open_file {
        let h = &mut cx.state.history;
        if let Some(id) = h.selected.clone() {
            h.detail_file = Some(path.clone());
            h.detail_diff = None;
            cx.worker.send(Command::LoadCommitFileDiff { id, path });
        }
    }
}
```

- [ ] **Step 7: Implement the toolbar and menu entries**

Insert this **above** the existing `#[cfg(test)]` module in `crates/app/src/ui/sync_toolbar.rs`.

`crates/app/src/ui/sync_toolbar.rs`:

```rust
//! Toolbar and menu entries for branches and fetch / pull / push.

use gitcore::{Branch, PullMode, PushMode};
use win95::{Button95, combo_box};

use super::Ctx;
use crate::protocol::Command;
use crate::state::{AppState, PendingDialog};
use crate::strings as s;

/// What the sync buttons can do right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncButtons {
    pub fetch: bool,
    pub pull: bool,
    pub push: bool,
    /// The current branch has no upstream: Push becomes "Publish".
    pub publish: bool,
    pub pull_label: String,
    pub push_label: String,
}

/// Pure: button states from the app state.
pub fn sync_buttons(state: &AppState) -> SyncButtons {
    let idle = state.current.is_some() && state.sync.running.is_none() && state.operation.is_none();
    let has_origin = state
        .current
        .as_ref()
        .is_some_and(|c| c.origin_url.is_some());
    let head = current_branch(&state.branches);
    let publish = head.is_some_and(|b| b.upstream.is_none());
    let (ahead, behind) = head.map(|b| (b.ahead, b.behind)).unwrap_or((0, 0));
    SyncButtons {
        fetch: idle && has_origin,
        pull: idle && has_origin && head.is_some_and(|b| b.upstream.is_some()),
        push: idle && has_origin && head.is_some() && (publish || ahead > 0),
        publish,
        pull_label: if behind > 0 {
            format!("{} ↓{behind}", s::PULL)
        } else {
            s::PULL.to_string()
        },
        push_label: if publish {
            s::PUBLISH.to_string()
        } else if ahead > 0 {
            format!("{} ↑{ahead}", s::PUSH)
        } else {
            s::PUSH.to_string()
        },
    }
}

pub fn current_branch(branches: &[Branch]) -> Option<&Branch> {
    branches.iter().find(|b| b.is_head && !b.remote)
}

/// Remote branches worth offering (no local branch with the same short name).
pub fn remote_only(branches: &[Branch]) -> Vec<&Branch> {
    branches
        .iter()
        .filter(|b| b.remote)
        .filter(|r| {
            let short = r.name.split_once('/').map(|x| x.1).unwrap_or(&r.name);
            !branches.iter().any(|l| !l.remote && l.name == short)
        })
        .collect()
}

pub fn toolbar(ui: &mut egui::Ui, cx: &mut Ctx<'_>) {
    let b = sync_buttons(cx.state);
    let size = egui::vec2(60.0, 22.0);
    if ui
        .add(Button95::new(s::FETCH).min_size(size).enabled(b.fetch))
        .clicked()
    {
        cx.worker.send(Command::Fetch { background: false });
    }
    if ui
        .add(Button95::new(&b.pull_label).min_size(size).enabled(b.pull))
        .clicked()
    {
        cx.worker.send(Command::Pull(PullMode::FastForwardOnly));
    }
    if ui
        .add(Button95::new(&b.push_label).min_size(size).enabled(b.push))
        .clicked()
    {
        cx.worker.send(Command::Push(if b.publish {
            PushMode::SetUpstream
        } else {
            PushMode::Normal
        }));
    }
    if cx.state.current.is_none() {
        return;
    }
    ui.separator();
    ui.label(s::BRANCH_LABEL);
    let head_name = current_branch(&cx.state.branches)
        .map(|b| b.name.clone())
        .unwrap_or_else(|| s::DETACHED_AT.to_string());
    let mut pick: Option<String> = None;
    let locals: Vec<Branch> = cx
        .state
        .branches
        .iter()
        .filter(|b| !b.remote)
        .cloned()
        .collect();
    let remotes: Vec<Branch> = remote_only(&cx.state.branches)
        .into_iter()
        .cloned()
        .collect();
    combo_box(ui, "branch_picker", &head_name, 200.0, |ui| {
        for l in &locals {
            let label = if l.is_head {
                format!("✓ {}", l.name)
            } else {
                format!("   {}", l.name)
            };
            if ui.button(label).clicked() && !l.is_head {
                pick = Some(l.name.clone());
            }
        }
        if !remotes.is_empty() {
            ui.separator();
            ui.label(egui::RichText::new(s::REMOTE_BRANCHES).color(win95::theme::GRAY));
            for r in &remotes {
                if ui.button(format!("   {}", r.name)).clicked() {
                    pick = Some(r.name.clone());
                }
            }
        }
    });
    if let Some(name) = pick {
        cx.worker.send(Command::SwitchBranch { name, stash: false });
    }
    let idle = cx.state.sync.running.is_none();
    if ui
        .add(
            Button95::new(s::NEW_BRANCH)
                .min_size(egui::vec2(90.0, 22.0))
                .enabled(idle),
        )
        .clicked()
    {
        cx.state.dialog = Some(PendingDialog::NewBranch {
            name: String::new(),
            switch: true,
        });
    }
}

/// Repository menu entries for sub-project 3.
pub fn menu_entries(ui: &mut egui::Ui, cx: &mut Ctx<'_>) {
    let b = sync_buttons(cx.state);
    if ui
        .add_enabled(b.fetch, egui::Button::new(s::FETCH))
        .clicked()
    {
        cx.worker.send(Command::Fetch { background: false });
    }
    if ui
        .add_enabled(b.pull, egui::Button::new(b.pull_label.clone()))
        .clicked()
    {
        cx.worker.send(Command::Pull(PullMode::FastForwardOnly));
    }
    if ui
        .add_enabled(b.push, egui::Button::new(b.push_label.clone()))
        .clicked()
    {
        cx.worker.send(Command::Push(if b.publish {
            PushMode::SetUpstream
        } else {
            PushMode::Normal
        }));
    }
    ui.separator();
    let open = cx.state.current.is_some();
    let head = current_branch(&cx.state.branches).map(|b| b.name.clone());
    if ui
        .add_enabled(open, egui::Button::new(s::NEW_BRANCH_MENU))
        .clicked()
    {
        cx.state.dialog = Some(PendingDialog::NewBranch {
            name: String::new(),
            switch: true,
        });
    }
    if ui
        .add_enabled(head.is_some(), egui::Button::new(s::RENAME_BRANCH_MENU))
        .clicked()
        && let Some(old) = head.clone()
    {
        cx.state.dialog = Some(PendingDialog::RenameBranch {
            name: old.clone(),
            old,
        });
    }
    if ui
        .add_enabled(open, egui::Button::new(s::DELETE_BRANCH_MENU))
        .clicked()
    {
        cx.state.dialog = Some(PendingDialog::DeleteBranch {
            name: String::new(),
        });
    }
    if let Some(op) = cx.state.operation {
        ui.separator();
        let label = match op {
            gitcore::Operation::Merge => s::ABORT_MERGE,
            gitcore::Operation::Rebase => s::ABORT_REBASE,
        };
        if ui.button(label).clicked() {
            cx.worker.send(Command::AbortOperation);
        }
    }
}
```

- [ ] **Step 8: Implement the dialogs and the network progress box**

Each dialog returns an `Outcome` (keep, close, send a command, or replace by another dialog such as the force-push confirmation).

`crates/app/src/ui/sync_dialogs.rs`:

```rust
//! Dialogs of sub-project 3 and the network progress box.

use gitcore::{PullMode, PushMode};
use win95::{Button95, Dialog, Icon, ProgressBar95, checkbox, combo_box, text_field};

use super::Ctx;
use crate::protocol::{Command, SyncOp};
use crate::state::{PendingDialog, branch_name_error};
use crate::strings as s;

/// What the user decided in a dialog this frame.
#[derive(Clone)]
enum Outcome {
    Keep(PendingDialog),
    Close,
    Send(Command),
    /// Replace the dialog by another one (e.g. a second confirmation).
    Next(PendingDialog),
}

pub fn show(egui_ctx: &egui::Context, cx: &mut Ctx<'_>) {
    progress(egui_ctx, cx);
    let Some(dialog) = cx.state.dialog.take() else {
        return;
    };
    let outcome = match dialog.clone() {
        PendingDialog::NewBranch { name, switch } => {
            name_dialog(egui_ctx, cx, s::NEW_BRANCH_TITLE, None, name, switch)
        }
        PendingDialog::RenameBranch { old, name } => {
            name_dialog(egui_ctx, cx, s::RENAME_BRANCH_TITLE, Some(old), name, false)
        }
        PendingDialog::DeleteBranch { name } => delete_dialog(egui_ctx, cx, name),
        PendingDialog::DeleteNotMerged { name } => choice(
            egui_ctx,
            s::DELETE_BRANCH_TITLE,
            s::NOT_MERGED_QUESTION,
            dialog,
            vec![(
                s::DELETE_ANYWAY,
                Outcome::Send(Command::DeleteBranch { name, force: true }),
            )],
        ),
        PendingDialog::Diverged { ahead, behind } => {
            let q = format!(
                "Your branch and its upstream have diverged (↑{ahead} ↓{behind}). How do you want to integrate the remote changes?"
            );
            let buttons = vec![
                (s::MERGE, Outcome::Send(Command::Pull(PullMode::Merge))),
                (s::REBASE, Outcome::Send(Command::Pull(PullMode::Rebase))),
            ];
            choice(egui_ctx, s::DIVERGED_TITLE, &q, dialog, buttons)
        }
        PendingDialog::PushRejected { can_force } => {
            let mut buttons = vec![(
                s::PULL,
                Outcome::Send(Command::Pull(PullMode::FastForwardOnly)),
            )];
            if can_force {
                buttons.push((
                    s::FORCE_PUSH,
                    Outcome::Next(PendingDialog::ConfirmForcePush),
                ));
            }
            choice(
                egui_ctx,
                s::PUSH_REJECTED_TITLE,
                s::ERR_PUSH_REJECTED,
                dialog,
                buttons,
            )
        }
        PendingDialog::ConfirmForcePush => choice(
            egui_ctx,
            s::PUSH_REJECTED_TITLE,
            s::FORCE_PUSH_CONFIRM,
            dialog,
            vec![(
                s::FORCE,
                Outcome::Send(Command::Push(PushMode::ForceWithLease)),
            )],
        ),
        PendingDialog::WouldOverwrite { branch, files } => {
            let q = format!("{}\n\n{}", s::ERR_WOULD_OVERWRITE, files.join("\n"));
            let buttons = vec![(
                s::STASH_SWITCH,
                Outcome::Send(Command::SwitchBranch {
                    name: branch,
                    stash: true,
                }),
            )];
            choice(egui_ctx, s::SWITCH_TITLE, &q, dialog, buttons)
        }
    };
    match outcome {
        Outcome::Keep(d) | Outcome::Next(d) => cx.state.dialog = Some(d),
        Outcome::Close => {}
        Outcome::Send(cmd) => cx.worker.send(cmd),
    }
}

/// Question, one button per choice, and Cancel.
fn choice(
    egui_ctx: &egui::Context,
    title: &str,
    question: &str,
    dialog: PendingDialog,
    buttons: Vec<(&'static str, Outcome)>,
) -> Outcome {
    let mut picked: Option<Outcome> = None;
    let mut cancel = false;
    let r = Dialog::new(("sync_choice", title), title)
        .width(420.0)
        .show(egui_ctx, |ui| {
            ui.horizontal(|ui| {
                win95::icon::icon(ui, Icon::Warning);
                ui.add(egui::Label::new(question).wrap());
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                for (label, outcome) in &buttons {
                    if ui
                        .add(Button95::new(*label).min_size(egui::vec2(90.0, 23.0)))
                        .clicked()
                    {
                        picked = Some(outcome.clone());
                    }
                }
                cancel = ui.add(Button95::new(s::CANCEL)).clicked();
            });
        });
    match picked {
        Some(o) => o,
        None if cancel || r.close_requested => Outcome::Close,
        None => Outcome::Keep(dialog),
    }
}

fn name_dialog(
    egui_ctx: &egui::Context,
    cx: &mut Ctx<'_>,
    title: &str,
    old: Option<String>,
    mut name: String,
    mut switch: bool,
) -> Outcome {
    let error = branch_name_error(&name, &cx.state.branches);
    let mut ok = false;
    let mut cancel = false;
    let r = Dialog::new(("branch_name", title), title)
        .width(360.0)
        .show(egui_ctx, |ui| {
            ui.label(s::BRANCH_NAME);
            let resp = text_field(ui, &mut name, 320.0, false);
            if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                ok = true;
            }
            if !name.is_empty()
                && let Some(e) = &error
            {
                ui.label(egui::RichText::new(e).color(egui::Color32::from_rgb(0x80, 0, 0)));
            }
            if old.is_none() {
                checkbox(ui, &mut switch, s::SWITCH_TO_IT);
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ok |= ui
                    .add(Button95::new(s::OK).enabled(error.is_none()))
                    .clicked();
                cancel = ui.add(Button95::new(s::CANCEL)).clicked();
            });
        });
    if cancel || r.close_requested {
        return Outcome::Close;
    }
    if ok && branch_name_error(&name, &cx.state.branches).is_none() {
        let name = name.trim().to_string();
        return Outcome::Send(match old {
            Some(old) => Command::RenameBranch { old, new: name },
            None => Command::CreateBranch { name, switch },
        });
    }
    Outcome::Keep(match old {
        Some(old) => PendingDialog::RenameBranch { old, name },
        None => PendingDialog::NewBranch { name, switch },
    })
}

fn delete_dialog(egui_ctx: &egui::Context, cx: &mut Ctx<'_>, mut name: String) -> Outcome {
    let candidates: Vec<String> = cx
        .state
        .branches
        .iter()
        .filter(|b| !b.remote && !b.is_head)
        .map(|b| b.name.clone())
        .collect();
    let mut delete = false;
    let mut cancel = false;
    let r = Dialog::new("delete_branch", s::DELETE_BRANCH_TITLE)
        .width(360.0)
        .show(egui_ctx, |ui| {
            if candidates.is_empty() {
                ui.label(s::CANNOT_DELETE_CURRENT);
            } else {
                if !candidates.contains(&name) {
                    name = candidates[0].clone();
                }
                let shown = name.clone();
                combo_box(ui, "delete_branch_pick", &shown, 320.0, |ui| {
                    for c in &candidates {
                        if ui.button(c).clicked() {
                            name = c.clone();
                        }
                    }
                });
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                delete = ui
                    .add(Button95::new(s::DELETE).enabled(!candidates.is_empty()))
                    .clicked();
                cancel = ui.add(Button95::new(s::CANCEL)).clicked();
            });
        });
    if cancel || r.close_requested {
        Outcome::Close
    } else if delete {
        Outcome::Send(Command::DeleteBranch { name, force: false })
    } else {
        Outcome::Keep(PendingDialog::DeleteBranch { name })
    }
}

/// Modal progress for user-started network operations (background fetch: status bar only).
fn progress(egui_ctx: &egui::Context, cx: &mut Ctx<'_>) {
    let sync = &cx.state.sync;
    let Some(op) = sync.running else { return };
    if sync.background {
        return;
    }
    let title = match op {
        SyncOp::Fetch => s::FETCHING,
        SyncOp::Pull => s::PULLING,
        SyncOp::Push => s::PUSHING,
    };
    let (phase, pct) = match &sync.progress {
        Some(p) => (
            format!("{}: {}%", p.phase, p.percent.unwrap_or(0)),
            p.percent.map(|v| v as f32 / 100.0),
        ),
        None => (title.to_string(), None),
    };
    let r = Dialog::new("sync_progress", title)
        .width(340.0)
        .show(egui_ctx, |ui| {
            ui.label(phase);
            ui.add(ProgressBar95::new(pct).width(ui.available_width()));
            ui.add_space(8.0);
            ui.vertical_centered(|ui| ui.add(Button95::new(s::CANCEL)).clicked())
                .inner
        });
    if r.inner || r.close_requested {
        cx.worker.cancel_network();
    }
}
```

- [ ] **Step 9: Tabs, toolbar and menu in the main window**

Replace the whole file with the version below (it keeps everything from sub-projects 1 and 2).

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
                    let mut tab = match cx.state.tab {
                        crate::state::Tab::Changes => 0,
                        crate::state::Tab::History => 1,
                    };
                    win95::tabs(ui, &mut tab, &[s::TAB_CHANGES, s::TAB_HISTORY]);
                    cx.state.tab = if tab == 0 {
                        crate::state::Tab::Changes
                    } else {
                        crate::state::Tab::History
                    };
                    match cx.state.tab {
                        crate::state::Tab::Changes => super::changes::show(ui, cx),
                        crate::state::Tab::History => super::history::show(ui, cx),
                    }
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
                cx.worker.send(super::changes::all_files_command(
                    &files,
                    gitcore::Side::Unstaged,
                ));
            }
            if ui
                .add_enabled(open, egui::Button::new(s::UNSTAGE_ALL))
                .clicked()
            {
                let files: Vec<_> = cx.state.changes.staged().cloned().collect();
                cx.worker.send(super::changes::all_files_command(
                    &files,
                    gitcore::Side::Staged,
                ));
            }
            ui.separator();
            super::sync_toolbar::menu_entries(ui, cx);
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
        super::sync_toolbar::toolbar(ui, cx);
    });
}

const RECENT_COLUMNS: &[Column] = &[Column {
    title: s::REPOSITORIES,
    width: 400.0,
}];

fn recents(ui: &mut egui::Ui, cx: &mut Ctx<'_>) {
    ui.label(s::REPOSITORIES);
    let recent = cx.state.recents_sorted();
    let selected = cx.state.selected_recent();
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
    } else if let Some(op) = cx.state.sync.running {
        match op {
            crate::protocol::SyncOp::Fetch => s::FETCHING,
            crate::protocol::SyncOp::Pull => s::PULLING,
            crate::protocol::SyncOp::Push => s::PUSHING,
        }
    } else if let Some(note) = cx.state.sync.note.as_deref() {
        note
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

- [ ] **Step 10: Show the dialogs**

Replace the whole file with the version below (it keeps everything from sub-projects 1 and 2).

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
        ui::discard::show(&egui_ctx, &mut cx);
        ui::sync_dialogs::show(&egui_ctx, &mut cx);
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

- [ ] **Step 11: Run the tests to see them pass**

Run: `cargo test -p retrogit --lib`

Expected: all lib tests pass, including `relative_times`, `lanes_are_evenly_spaced_and_colors_cycle`, `buttons_follow_ahead_behind_and_upstream`, `nothing_while_busy_or_detached_or_merging`, `remote_branches_already_checked_out_are_hidden`, `signing_label_says_whether_commits_are_signed`.

- [ ] **Step 12: Lint**

Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings`

Expected: no output from fmt; clippy finishes without warnings.

- [ ] **Step 13: Full suite**

Run: `cargo test --workspace`

Expected: 220 passed, 0 failed.

- [ ] **Step 14: Commit**

```bash
git add crates/app
git commit -m "feat(app): History tab with commit graph, branch picker, sync toolbar and dialogs"
```


### Task 7: Release check and manual verification

**Files:**
- No code changes expected (fix and commit anything the checks reveal).

**Interfaces:**
- Consumes: the finished feature.

- [ ] **Step 1: Release build**

```bash
cargo build --release -p retrogit
ls -lh target/release/retrogit
otool -L target/release/retrogit
```

Expected: under 15 MB; only `/System/Library/...` and `/usr/lib/...`.

- [ ] **Step 2: Manual checks (user, launched from the Dock)**

1. Open a ExampleOrg repo: the toolbar shows the branch, Pull ↓n / Push ↑n after the background fetch; History shows the graph with colored lanes and ref labels; scrolling far down keeps loading.
2. Click a commit: message, files, per-file diff, and the signature line (🔒 Good signature … for your signed commits).
3. The commit form shows "🔒 Signed with GPG key ABCD1234…"; commit something: the GPG passphrase prompt (pinentry-mac) appears if needed; `git log --show-signature -1` shows a good signature.
4. New branch → Publish → the branch exists on GitHub; Push after a commit works (github.com HTTPS uses the RetroGit token even if the Keychain has an old password).
5. From another clone, push a commit to the same branch; commit locally; Pull → Merge/Rebase dialog; both choices work; a conflicting change shows the yellow banner with Abort / Continue.
6. Edit a file that differs on another branch and switch to it → "Stash, switch and re-apply" works.
7. Rename and delete a branch from the Repository menu (unmerged → second confirmation).
8. Push rejected → Pull offered; after amending a pushed commit → "Force push (with lease)…" with a second confirmation.
9. An SSH remote without a loaded key fails within seconds with help text (no hang).

- [ ] **Step 3: Windows check (when available)**

`cargo test --workspace`, then checks 1, 4 and 5 on Windows x86_64: no console window flashes on fetch/push, and the askpass handshake works with the GUI-subsystem binary.
