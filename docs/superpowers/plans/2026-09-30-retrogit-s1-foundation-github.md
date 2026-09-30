# RetroGit S1 — Foundation + GitHub Sign-in Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A Windows 95–style desktop Git client (macOS arm64 + Windows x86_64) that signs in to github.com (Device Flow or PAT), lists the user's repositories, clones them with progress/cancel, and opens local repositories.

**Architecture:** Cargo workspace with four crates. `win95` (egui widget kit), `gitcore` (our Git API over git2), and `github` (REST + Device Flow + token storage) are independent libraries; `retrogit` (in `crates/app`) wires them together. The UI thread never blocks: one worker thread receives `Command`s over an mpsc channel and sends back `Event`s, which a pure `AppState::apply` folds into state. egui runs in reactive mode (no repaint when idle).

**Tech Stack:** Rust 1.95+ (verified with 1.98.1), eframe/egui 0.36 (glow backend), egui_kittest 0.36, git2 0.21 (vendored libgit2 + vendored OpenSSL, HTTPS only), ureq 3 (rustls + OS trust store), keyring 4, rfd 0.17, serde/serde_json, thiserror 2, dirs 7, log; tests use mockito 1 and tempfile 3.

**Spec:** `docs/superpowers/specs/2026-09-30-retrogit-s1-foundation-github-design.md`

> Every code block in this plan was compiled, formatted, linted (`clippy -D warnings`) and tested on macOS arm64 with Rust 1.98.1 before the plan was written (80 tests). Copy code verbatim; if a step's expected output differs, stop and investigate instead of improvising.

## Global Constraints

- Project root: `~/perso/retrogit` (already a git repo containing the spec). Never work inside `~/work`.
- Targets: `aarch64-apple-darwin` and `x86_64-pc-windows-msvc`. Nothing platform-specific except `#![cfg_attr(windows, windows_subsystem)]` and what the crates already abstract.
- Rust edition 2024, `rust-version = "1.95"` (required by eframe 0.36 / egui_kittest 0.36).
- Dependency versions (workspace): eframe/egui/egui_kittest `0.36`, git2 `0.21` with `default-features = false, features = ["https", "vendored-libgit2", "vendored-openssl"]`, ureq `3` with `["json", "platform-verifier"]`, keyring `4`, rfd `0.17`, thiserror `2`, serde `1` (derive), serde_json `1`, dirs `7`, log `0.4`, mockito `1`, tempfile `3`.
- No SSH, no async runtime, no webview.
- `win95`, `gitcore`, `github` must not depend on each other. No `git2` type in `gitcore`'s public API.
- The token is never written to disk in clear, never embedded in a clone URL or `.git/config`, never logged (the logger redacts it).
- UI language: English. Every user-visible string lives in `crates/app/src/strings.rs`.
- Font: W95FA (SIL OFL 1.1), shipped with its `OFL.txt`. No Microsoft font.
- Release profile: `opt-level = 3`, `lto = "thin"`, `codegen-units = 1`, `strip = true`, `panic = "unwind"`.
- Keychain entry: service `RetroGit`, account `github.com`. OAuth scopes: `repo read:org`.
- Config: `<OS config dir>/RetroGit/config.json`. Log: `<OS local data dir>/RetroGit/retrogit.log`, truncated above 5 MB.
- Workspace lints: `unsafe_code = "forbid"`, clippy `unwrap_used`/`expect_used` = warn (tests opt out with `#![allow]`). Every task ends with `cargo fmt --all --check` and `cargo clippy --workspace --all-targets -- -D warnings` clean.
- Deviation from spec, agreed: the `tree_view` widget moves to sub-project 2 (the S1 recent-repos list is flat and uses `ListView`).

## Review Focus

1. **Token not SSO-authorized when cloning an org repo** (GitHub answers 401 to git): the clone must fail fast with an auth message (no endless credential retry) and remove the folder it created. Pinned by `rejected_credentials_fail_fast_with_auth_error_and_clean_up` (Task 2).
2. **Network drops while waiting for Device Flow authorization:** a message box appears and the dialog returns to the "Sign in" button instead of spinning forever. Pinned by `network_failure_while_polling_reports_auth_error` (Task 10) and `auth_error_while_waiting_resets_to_signed_out_and_queues_message` (Task 9).
3. **Token revoked while the app is open:** the next GitHub call clears the keychain entry, shows a message and reopens sign-in. Pinned by `token_revoked_while_running_signs_out_on_next_call` (Task 10).
4. **Quitting during a clone or sign-in:** the worker is cancelled and given time to delete the partial folder before the process exits. Pinned by `shutdown_interrupts_a_running_device_flow` (Task 10) plus the cancel-cleanup tests of Task 2; wired in `on_exit` (Task 11).
5. **Corporate TLS-inspection proxy:** HTTPS must trust the OS certificate store (`RootCerts::PlatformVerifier`). No automated test can simulate this; it is a manual check in Task 12 on the corporate network.

---


### Task 1: Workspace scaffold + `gitcore` open/summary

**Files:**
- Create: `Cargo.toml`, `rust-toolchain.toml`, `.gitignore`
- Create: `crates/gitcore/Cargo.toml`, `crates/gitcore/src/lib.rs`, `crates/gitcore/src/error.rs`, `crates/gitcore/src/repo.rs`
- Test: `crates/gitcore/tests/common/mod.rs`, `crates/gitcore/tests/repo.rs`

**Interfaces:**
- Consumes: nothing.
- Produces: `gitcore::Repo::open(&Path) -> Result<Repo, GitError>`, `Repo::path(&self) -> &Path`, `Repo::summary(&self) -> Result<RepoSummary, GitError>`; `RepoSummary { name: String, path: PathBuf, head: Head, origin_url: Option<String>, last_commit: Option<CommitInfo> }`; `Head::{Branch(String), Unborn(String), Detached(String)}`; `CommitInfo { short_id: String /*7 chars*/, summary: String, author: String, time: i64 }`; `GitError::{NotARepository(PathBuf), DestinationNotEmpty(PathBuf), Cancelled, Auth(String), Network(String), Other(String)}` (`Clone + PartialEq + Eq + Display`); crate-private `GitError::from_git2(&git2::Error) -> GitError`.

- [ ] **Step 1: Check the toolchain**

Run: `rustc --version`

Expected: `rustc 1.95.0` or newer. If older, run `rustup update stable`.

- [ ] **Step 2: Create the workspace manifest**

`members = ["crates/*"]` picks up each crate as later tasks create it. Path dependencies to crates that don't exist yet are fine until something uses them.

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
eframe = { version = "0.36", default-features = false, features = ["glow", "accesskit", "default_fonts"] }
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
mockito = "1"
tempfile = "3"

[workspace.lints.rust]
unsafe_code = "forbid"

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

- [ ] **Step 3: Pin the toolchain channel and ignore build output**

`rust-toolchain.toml`:

```toml
[toolchain]
channel = "stable"
components = ["rustfmt", "clippy"]
```

`.gitignore`:
```text
target/
.DS_Store
```

- [ ] **Step 4: Create the `gitcore` manifest**

`crates/gitcore/Cargo.toml`:

```toml
[package]
name = "gitcore"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
publish.workspace = true

[dependencies]
git2.workspace = true
thiserror.workspace = true

[dev-dependencies]
git2.workspace = true
mockito.workspace = true
tempfile.workspace = true

[lints]
workspace = true
```

- [ ] **Step 5: Write the test helpers**

`make_repo` builds a real repository on branch `main` with a fixed author date; `file_url` is used by Task 2.

`crates/gitcore/tests/common/mod.rs`:

```rust
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;

/// Create a non-bare repo with `commits` commits on branch `main`.
pub fn make_repo(dir: &Path, commits: usize) -> git2::Repository {
    let mut opts = git2::RepositoryInitOptions::new();
    opts.initial_head("main");
    let repo = git2::Repository::init_opts(dir, &opts).unwrap();
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

- [ ] **Step 6: Write the failing tests**

`crates/gitcore/tests/repo.rs`:

```rust
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
```

- [ ] **Step 7: Create the crate root (Task 1 version)**

`crates/gitcore/src/lib.rs`:

```rust
//! RetroGit's internal Git API. No `git2` type is ever exposed publicly.

mod error;
mod repo;

pub use error::GitError;
pub use repo::{CommitInfo, Head, Repo, RepoSummary};
```

- [ ] **Step 8: Run the tests to see them fail**

Run: `cargo test -p gitcore`

Expected: compile error `file not found for module error` / `repo`.

- [ ] **Step 9: Implement the error type**

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

- [ ] **Step 10: Implement `Repo`**

`open` uses `NO_SEARCH` so picking a sub-folder of a repo is reported as "not a repository" instead of silently opening the parent.

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

- [ ] **Step 11: Run the tests to see them pass**

Run: `cargo test -p gitcore`

Expected: `test result: ok. 5 passed` for `tests/repo.rs`.

- [ ] **Step 12: Lint**

Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings`

Expected: no output from fmt, clippy finishes without warnings.

- [ ] **Step 13: Commit**

```bash
git add Cargo.toml Cargo.lock rust-toolchain.toml .gitignore crates/gitcore
git commit -m "feat(gitcore): workspace scaffold, open repository and summary"
```


### Task 2: `gitcore` clone with progress, cancel and cleanup

**Files:**
- Create: `crates/gitcore/src/clone.rs`
- Modify: `crates/gitcore/src/lib.rs`, `crates/gitcore/src/repo.rs` (add `from_git2`), `crates/gitcore/Cargo.toml` (mockito dev-dep, already in the file from Task 1)
- Test: `crates/gitcore/tests/clone.rs`

**Interfaces:**
- Consumes: `Repo`, `GitError`, `GitError::from_git2` (Task 1).
- Produces: `gitcore::clone(req: &CloneRequest, progress: impl FnMut(CloneProgress), cancel: &AtomicBool) -> Result<Repo, GitError>`; `CloneRequest { url: String, dest: PathBuf, credentials: Option<Credentials> }`; `Credentials { username: String, password: String }` (Debug hides the password); `CloneProgress { received_objects, total_objects, received_bytes, indexed_deltas, total_deltas: usize }` (`Copy + Default + PartialEq`) with `fraction(&self) -> f32` in 0..=1.

- [ ] **Step 1: Write the failing tests**

Local clones use a `file://` URL so libgit2 goes through the smart transport and fires progress callbacks (a plain path silently does a local copy with no progress). The last test serves a 401 over HTTP with mockito to reproduce an SSO-blocked token.

`crates/gitcore/tests/clone.rs`:

```rust
#![allow(clippy::unwrap_used)]
mod common;

use std::sync::atomic::{AtomicBool, Ordering};

use gitcore::{CloneProgress, CloneRequest, GitError, Head, clone};

fn request(src: &std::path::Path, dest: std::path::PathBuf) -> CloneRequest {
    CloneRequest {
        url: common::file_url(src),
        dest,
        credentials: None,
    }
}

#[test]
fn clone_succeeds_and_reports_progress() {
    let src = tempfile::tempdir().unwrap();
    common::make_repo(src.path(), 3);
    let out = tempfile::tempdir().unwrap();
    let dest = out.path().join("demo");
    let mut last = CloneProgress::default();
    let mut calls = 0;
    let repo = clone(
        &request(src.path(), dest.clone()),
        |p| {
            calls += 1;
            last = p;
        },
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(calls > 0);
    assert!(last.total_objects > 0);
    assert_eq!(last.received_objects, last.total_objects);
    assert!((last.fraction() - 1.0).abs() < f32::EPSILON);
    assert_eq!(repo.summary().unwrap().head, Head::Branch("main".into()));
    assert!(dest.join("file2.txt").exists());
}

#[test]
fn clone_into_existing_empty_dir_is_allowed() {
    let src = tempfile::tempdir().unwrap();
    common::make_repo(src.path(), 1);
    let dest = tempfile::tempdir().unwrap();
    clone(
        &request(src.path(), dest.path().to_path_buf()),
        |_| {},
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(dest.path().join(".git").exists());
}

#[test]
fn clone_refuses_non_empty_destination_and_leaves_it_untouched() {
    let src = tempfile::tempdir().unwrap();
    common::make_repo(src.path(), 1);
    let dest = tempfile::tempdir().unwrap();
    std::fs::write(dest.path().join("keep.txt"), "mine").unwrap();
    let err = clone(
        &request(src.path(), dest.path().to_path_buf()),
        |_| {},
        &AtomicBool::new(false),
    )
    .err()
    .unwrap();
    assert_eq!(
        err,
        GitError::DestinationNotEmpty(dest.path().to_path_buf())
    );
    assert_eq!(
        std::fs::read_to_string(dest.path().join("keep.txt")).unwrap(),
        "mine"
    );
}

#[test]
fn cancelled_clone_removes_the_directory_it_created() {
    let src = tempfile::tempdir().unwrap();
    common::make_repo(src.path(), 3);
    let out = tempfile::tempdir().unwrap();
    let dest = out.path().join("demo");
    let cancel = AtomicBool::new(false);
    let err = clone(
        &request(src.path(), dest.clone()),
        |_| cancel.store(true, Ordering::Relaxed),
        &cancel,
    )
    .err()
    .unwrap();
    assert_eq!(err, GitError::Cancelled);
    assert!(!dest.exists());
}

#[test]
fn cancelled_clone_into_existing_empty_dir_keeps_the_dir_but_empties_it() {
    let src = tempfile::tempdir().unwrap();
    common::make_repo(src.path(), 3);
    let dest = tempfile::tempdir().unwrap();
    let cancel = AtomicBool::new(false);
    let err = clone(
        &request(src.path(), dest.path().to_path_buf()),
        |_| cancel.store(true, Ordering::Relaxed),
        &cancel,
    )
    .err()
    .unwrap();
    assert_eq!(err, GitError::Cancelled);
    assert!(dest.path().exists());
    assert_eq!(std::fs::read_dir(dest.path()).unwrap().count(), 0);
}

#[test]
fn clone_of_missing_source_fails_and_cleans_up() {
    let out = tempfile::tempdir().unwrap();
    let dest = out.path().join("demo");
    let missing = out.path().join("does-not-exist");
    let err = clone(
        &request(&missing, dest.clone()),
        |_| {},
        &AtomicBool::new(false),
    )
    .err()
    .unwrap();
    assert!(
        matches!(err, GitError::Other(_) | GitError::Network(_)),
        "{err:?}"
    );
    assert!(!dest.exists());
}

#[test]
fn credentials_debug_never_shows_password() {
    let c = gitcore::Credentials {
        username: "x-access-token".into(),
        password: "gho_secret".into(),
    };
    assert!(!format!("{c:?}").contains("gho_secret"));
}

#[test]
fn fraction_handles_zero_totals() {
    assert_eq!(CloneProgress::default().fraction(), 0.0);
}

#[test]
fn rejected_credentials_fail_fast_with_auth_error_and_clean_up() {
    // An HTTPS remote that always answers 401, like GitHub for a token not authorized for SSO.
    let mut server = mockito::Server::new();
    let m = server
        .mock("GET", mockito::Matcher::Any)
        .with_status(401)
        .with_header("WWW-Authenticate", "Basic realm=\"GitHub\"")
        .expect_at_most(4)
        .create();
    let out = tempfile::tempdir().unwrap();
    let dest = out.path().join("demo");
    let req = CloneRequest {
        url: format!("{}/org/demo.git", server.url()),
        dest: dest.clone(),
        credentials: Some(gitcore::Credentials {
            username: "x-access-token".into(),
            password: "gho_x".into(),
        }),
    };
    let err = clone(&req, |_| {}, &AtomicBool::new(false)).err().unwrap();
    assert!(matches!(err, GitError::Auth(_)), "{err:?}");
    assert!(!dest.exists());
    m.assert();
}
```

- [ ] **Step 2: Export the new module**

`crates/gitcore/src/lib.rs`:

```rust
//! RetroGit's internal Git API. No `git2` type is ever exposed publicly.

mod clone;
mod error;
mod repo;

pub use clone::{CloneProgress, CloneRequest, Credentials, clone};
pub use error::GitError;
pub use repo::{CommitInfo, Head, Repo, RepoSummary};
```

- [ ] **Step 3: Run the tests to see them fail**

Run: `cargo test -p gitcore --test clone`

Expected: compile error `file not found for module clone`.

- [ ] **Step 4: Add the crate-private constructor to `Repo`**

Insert inside `impl Repo`, right after `open`.

`crates/gitcore/src/repo.rs`:

```rust
    pub(crate) fn from_git2(inner: git2::Repository, path: PathBuf) -> Repo {
        Repo { inner, path }
    }
```

- [ ] **Step 5: Implement clone**

Key points: destination must be missing or empty; we remember whether we created it so cleanup never deletes a folder the user made; the credentials callback refuses to answer twice (libgit2 would otherwise retry forever with a rejected token); returning `false` from the progress callback aborts the transfer.

`crates/gitcore/src/clone.rs`:

```rust
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::{GitError, Repo};

/// HTTPS credentials. `Debug` never prints the password.
#[derive(Clone)]
pub struct Credentials {
    pub username: String,
    pub password: String,
}

impl fmt::Debug for Credentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Credentials")
            .field("username", &self.username)
            .field("password", &"***")
            .finish()
    }
}

#[derive(Debug, Clone)]
pub struct CloneRequest {
    /// Clone URL without any embedded credentials.
    pub url: String,
    pub dest: PathBuf,
    pub credentials: Option<Credentials>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CloneProgress {
    pub received_objects: usize,
    pub total_objects: usize,
    pub received_bytes: usize,
    pub indexed_deltas: usize,
    pub total_deltas: usize,
}

impl CloneProgress {
    /// Overall completion between 0.0 and 1.0 (objects count for 80 %, deltas for 20 %).
    pub fn fraction(&self) -> f32 {
        let objects = ratio(self.received_objects, self.total_objects);
        let deltas = if self.total_deltas == 0 {
            if self.total_objects > 0 && self.received_objects == self.total_objects {
                1.0
            } else {
                0.0
            }
        } else {
            ratio(self.indexed_deltas, self.total_deltas)
        };
        0.8 * objects + 0.2 * deltas
    }
}

fn ratio(a: usize, b: usize) -> f32 {
    if b == 0 {
        0.0
    } else {
        (a as f32 / b as f32).clamp(0.0, 1.0)
    }
}

/// Clone `req.url` into `req.dest`.
///
/// `dest` must not exist or be an empty directory. On failure or cancellation, everything
/// this function created is removed again.
pub fn clone(
    req: &CloneRequest,
    mut progress: impl FnMut(CloneProgress),
    cancel: &AtomicBool,
) -> Result<Repo, GitError> {
    let created_dest = prepare_destination(&req.dest)?;

    let mut callbacks = git2::RemoteCallbacks::new();
    if let Some(creds) = req.credentials.clone() {
        let mut attempts = 0u8;
        callbacks.credentials(move |_url, _user, _allowed| {
            attempts += 1;
            if attempts > 1 {
                // libgit2 retries forever with the same (rejected) credentials otherwise.
                return Err(git2::Error::new(
                    git2::ErrorCode::Auth,
                    git2::ErrorClass::Http,
                    "credentials rejected",
                ));
            }
            git2::Cred::userpass_plaintext(&creds.username, &creds.password)
        });
    }
    callbacks.transfer_progress(|p| {
        progress(CloneProgress {
            received_objects: p.received_objects(),
            total_objects: p.total_objects(),
            received_bytes: p.received_bytes(),
            indexed_deltas: p.indexed_deltas(),
            total_deltas: p.total_deltas(),
        });
        !cancel.load(Ordering::Relaxed)
    });
    let mut fetch = git2::FetchOptions::new();
    fetch.remote_callbacks(callbacks);

    let result = git2::build::RepoBuilder::new()
        .fetch_options(fetch)
        .clone(&req.url, &req.dest);
    match result {
        Ok(inner) if !cancel.load(Ordering::Relaxed) => {
            Ok(Repo::from_git2(inner, req.dest.clone()))
        }
        Ok(_) => {
            cleanup(&req.dest, created_dest);
            Err(GitError::Cancelled)
        }
        Err(e) => {
            cleanup(&req.dest, created_dest);
            if cancel.load(Ordering::Relaxed) {
                Err(GitError::Cancelled)
            } else {
                Err(GitError::from_git2(&e))
            }
        }
    }
}

/// Returns `true` if the directory did not exist (so we are the ones creating it).
fn prepare_destination(dest: &Path) -> Result<bool, GitError> {
    match std::fs::read_dir(dest) {
        Ok(mut entries) => {
            if entries.next().is_some() {
                Err(GitError::DestinationNotEmpty(dest.to_path_buf()))
            } else {
                Ok(false)
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(true),
        Err(e) => Err(GitError::Other(format!(
            "cannot use '{}': {e}",
            dest.display()
        ))),
    }
}

fn cleanup(dest: &Path, created_dest: bool) {
    if created_dest {
        let _ = std::fs::remove_dir_all(dest);
        return;
    }
    // The directory existed and was empty: remove only what the clone wrote into it.
    if let Ok(entries) = std::fs::read_dir(dest) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                let _ = std::fs::remove_dir_all(&p);
            } else {
                let _ = std::fs::remove_file(&p);
            }
        }
    }
}
```

- [ ] **Step 6: Run the tests to see them pass**

Run: `cargo test -p gitcore`

Expected: `tests/clone.rs`: `9 passed`; `tests/repo.rs`: `5 passed`.

- [ ] **Step 7: Lint**

Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings`

Expected: clean.

- [ ] **Step 8: Commit**

```bash
git add crates/gitcore
git commit -m "feat(gitcore): clone with progress, cancellation and cleanup"
```


### Task 3: `github` errors, Link header and Device Flow state machine

**Files:**
- Create: `crates/github/Cargo.toml`, `crates/github/src/lib.rs`, `crates/github/src/error.rs`, `crates/github/src/link.rs`, `crates/github/src/device_flow.rs`

**Interfaces:**
- Consumes: nothing from other crates.
- Produces: `GithubError::{Unauthorized, SsoRequired { url: String }, RateLimited, Network(String), Http(u16), Decode(String)}` (+ `From<ureq::Error>`); `github::next_link(&str) -> Option<String>`; `DeviceCode { device_code, user_code, verification_uri: String, expires_in: u64, interval: u64 }` (serde `Deserialize`); `PollResponse::{Token(String), Pending, SlowDown, Expired, Denied, Other(String)}` with `PollResponse::from_json(&serde_json::Value)`; `DeviceFlow::new(&DeviceCode)`, `first_wait(&self) -> Duration`, `on_response(&mut self, PollResponse, elapsed: Duration) -> Step`; `Step::{Wait(Duration), Done(String), Failed(DeviceFlowFailure)}`; `DeviceFlowFailure::{Expired, Denied, Other(String)}`; `github::SCOPES: &[&str] = ["repo", "read:org"]`.

- [ ] **Step 1: Create the manifest**

`crates/github/Cargo.toml`:

```toml
[package]
name = "github"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
publish.workspace = true

[dependencies]
ureq.workspace = true
keyring.workspace = true
serde.workspace = true
serde_json.workspace = true
thiserror.workspace = true

[dev-dependencies]
mockito.workspace = true

[lints]
workspace = true
```

- [ ] **Step 2: Create the crate root (Task 3 version)**

`crates/github/src/lib.rs`:

```rust
//! GitHub access for RetroGit: OAuth Device Flow, REST API, token storage.

mod device_flow;
mod error;
mod link;

pub use device_flow::{DeviceCode, DeviceFlow, DeviceFlowFailure, PollResponse, Step};
pub use error::GithubError;
pub use link::next_link;

/// OAuth scopes requested by RetroGit.
pub const SCOPES: &[&str] = &["repo", "read:org"];
```

- [ ] **Step 3: Implement the error type**

`crates/github/src/error.rs`:

```rust
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum GithubError {
    #[error("GitHub rejected the token (401)")]
    Unauthorized,
    #[error("this organization requires SSO authorization for the token: {url}")]
    SsoRequired { url: String },
    #[error("GitHub API rate limit exceeded")]
    RateLimited,
    #[error("network error: {0}")]
    Network(String),
    #[error("unexpected HTTP status {0}")]
    Http(u16),
    #[error("could not decode GitHub response: {0}")]
    Decode(String),
}

impl From<ureq::Error> for GithubError {
    fn from(e: ureq::Error) -> Self {
        match e {
            ureq::Error::StatusCode(s) => GithubError::Http(s),
            ureq::Error::Json(e) => GithubError::Decode(e.to_string()),
            other => GithubError::Network(other.to_string()),
        }
    }
}
```

- [ ] **Step 4: Write the failing tests in `crates/github/src/link.rs`**

Create `crates/github/src/link.rs` containing only this test module for now (the implementation goes above it in a later step).

`crates/github/src/link.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::next_link;

    #[test]
    fn finds_next_among_several_rels() {
        let h = r#"<https://api.github.com/user/repos?page=2>; rel="next", <https://api.github.com/user/repos?page=5>; rel="last""#;
        assert_eq!(
            next_link(h).as_deref(),
            Some("https://api.github.com/user/repos?page=2")
        );
    }

    #[test]
    fn next_can_be_last_in_the_list() {
        let h = r#"<https://x/?page=1>; rel="prev", <https://x/?page=3>; rel="next""#;
        assert_eq!(next_link(h).as_deref(), Some("https://x/?page=3"));
    }

    #[test]
    fn none_on_last_page_or_garbage() {
        assert_eq!(next_link(r#"<https://x/?page=1>; rel="prev""#), None);
        assert_eq!(next_link(""), None);
        assert_eq!(next_link("garbage"), None);
    }
}
```

- [ ] **Step 5: Write the failing tests in `crates/github/src/device_flow.rs`**

Create `crates/github/src/device_flow.rs` containing only this test module for now (the implementation goes above it in a later step).

`crates/github/src/device_flow.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn code(interval: u64, expires_in: u64) -> DeviceCode {
        DeviceCode {
            device_code: "dc".into(),
            user_code: "ABCD-1234".into(),
            verification_uri: "https://github.com/login/device".into(),
            expires_in,
            interval,
        }
    }

    const S: fn(u64) -> Duration = Duration::from_secs;

    #[test]
    fn pending_waits_for_interval() {
        let mut f = DeviceFlow::new(&code(5, 900));
        assert_eq!(f.first_wait(), S(5));
        assert_eq!(f.on_response(PollResponse::Pending, S(5)), Step::Wait(S(5)));
    }

    #[test]
    fn slow_down_adds_five_seconds_permanently() {
        let mut f = DeviceFlow::new(&code(5, 900));
        assert_eq!(
            f.on_response(PollResponse::SlowDown, S(5)),
            Step::Wait(S(10))
        );
        assert_eq!(
            f.on_response(PollResponse::Pending, S(15)),
            Step::Wait(S(10))
        );
    }

    #[test]
    fn zero_interval_is_clamped_to_one_second() {
        assert_eq!(DeviceFlow::new(&code(0, 900)).first_wait(), S(1));
    }

    #[test]
    fn token_finishes() {
        let mut f = DeviceFlow::new(&code(5, 900));
        assert_eq!(
            f.on_response(PollResponse::Token("gho_x".into()), S(5)),
            Step::Done("gho_x".into())
        );
    }

    #[test]
    fn terminal_errors_fail() {
        let mut f = DeviceFlow::new(&code(5, 900));
        assert_eq!(
            f.on_response(PollResponse::Expired, S(5)),
            Step::Failed(DeviceFlowFailure::Expired)
        );
        assert_eq!(
            f.on_response(PollResponse::Denied, S(5)),
            Step::Failed(DeviceFlowFailure::Denied)
        );
        assert_eq!(
            f.on_response(
                PollResponse::Other("incorrect_client_credentials".into()),
                S(5)
            ),
            Step::Failed(DeviceFlowFailure::Other(
                "incorrect_client_credentials".into()
            ))
        );
    }

    #[test]
    fn pending_past_expiry_fails_without_polling_again() {
        let mut f = DeviceFlow::new(&code(5, 900));
        assert_eq!(
            f.on_response(PollResponse::Pending, S(896)),
            Step::Failed(DeviceFlowFailure::Expired)
        );
    }

    #[test]
    fn parses_poll_json() {
        assert_eq!(
            PollResponse::from_json(&json!({"access_token":"gho_1","token_type":"bearer"})),
            PollResponse::Token("gho_1".into())
        );
        assert_eq!(
            PollResponse::from_json(&json!({"error":"authorization_pending"})),
            PollResponse::Pending
        );
        assert_eq!(
            PollResponse::from_json(&json!({"error":"slow_down","interval":10})),
            PollResponse::SlowDown
        );
        assert_eq!(
            PollResponse::from_json(&json!({"error":"expired_token"})),
            PollResponse::Expired
        );
        assert_eq!(
            PollResponse::from_json(&json!({"error":"access_denied"})),
            PollResponse::Denied
        );
        assert_eq!(
            PollResponse::from_json(&json!({"error":"device_flow_disabled"})),
            PollResponse::Other("device_flow_disabled".into())
        );
        assert_eq!(
            PollResponse::from_json(&json!({})),
            PollResponse::Other("unexpected response".into())
        );
    }
}
```

- [ ] **Step 6: Run the tests to see them fail**

Run: `cargo test -p github`

Expected: compile errors: `cannot find function next_link`, `cannot find type DeviceFlow`, etc.

- [ ] **Step 7: Implement `crates/github/src/link.rs`**

Insert this **above** the existing `#[cfg(test)]` module in `crates/github/src/link.rs`.

`crates/github/src/link.rs`:

```rust
/// Extract the `rel="next"` URL from a GitHub `Link` header.
pub fn next_link(header: &str) -> Option<String> {
    header.split(',').find_map(|part| {
        let mut pieces = part.split(';');
        let url = pieces.next()?.trim();
        let is_next = pieces.any(|p| p.trim() == r#"rel="next""#);
        if is_next && url.starts_with('<') && url.ends_with('>') {
            Some(url[1..url.len() - 1].to_string())
        } else {
            None
        }
    })
}
```

- [ ] **Step 8: Implement the Device Flow state machine**

Insert this **above** the existing `#[cfg(test)]` module in `crates/github/src/device_flow.rs`.

`crates/github/src/device_flow.rs`:

```rust
//! OAuth Device Flow as a pure state machine (no I/O, no clock).

use std::time::Duration;

use serde::Deserialize;

/// Response of `POST /login/device/code`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct DeviceCode {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    /// Seconds until `device_code` expires.
    pub expires_in: u64,
    /// Minimum seconds between two polls.
    pub interval: u64,
}

/// Interpreted response of `POST /login/oauth/access_token`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PollResponse {
    Token(String),
    Pending,
    SlowDown,
    Expired,
    Denied,
    Other(String),
}

impl PollResponse {
    pub fn from_json(v: &serde_json::Value) -> PollResponse {
        if let Some(t) = v.get("access_token").and_then(|t| t.as_str()) {
            return PollResponse::Token(t.to_string());
        }
        match v.get("error").and_then(|e| e.as_str()) {
            Some("authorization_pending") => PollResponse::Pending,
            Some("slow_down") => PollResponse::SlowDown,
            Some("expired_token") => PollResponse::Expired,
            Some("access_denied") => PollResponse::Denied,
            Some(other) => PollResponse::Other(other.to_string()),
            None => PollResponse::Other("unexpected response".to_string()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceFlowFailure {
    Expired,
    Denied,
    Other(String),
}

/// What the caller must do next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// Sleep this long, then poll again.
    Wait(Duration),
    Done(String),
    Failed(DeviceFlowFailure),
}

#[derive(Debug, Clone)]
pub struct DeviceFlow {
    interval: Duration,
    expires_in: Duration,
}

impl DeviceFlow {
    pub fn new(code: &DeviceCode) -> DeviceFlow {
        DeviceFlow {
            interval: Duration::from_secs(code.interval.max(1)),
            expires_in: Duration::from_secs(code.expires_in),
        }
    }

    /// Delay before the very first poll.
    pub fn first_wait(&self) -> Duration {
        self.interval
    }

    /// Feed the latest poll response. `elapsed` is the time since the device code was issued.
    pub fn on_response(&mut self, response: PollResponse, elapsed: Duration) -> Step {
        match response {
            PollResponse::Token(t) => Step::Done(t),
            PollResponse::Expired => Step::Failed(DeviceFlowFailure::Expired),
            PollResponse::Denied => Step::Failed(DeviceFlowFailure::Denied),
            PollResponse::Other(e) => Step::Failed(DeviceFlowFailure::Other(e)),
            PollResponse::Pending | PollResponse::SlowDown => {
                if matches!(response, PollResponse::SlowDown) {
                    self.interval += Duration::from_secs(5);
                }
                if elapsed + self.interval >= self.expires_in {
                    Step::Failed(DeviceFlowFailure::Expired)
                } else {
                    Step::Wait(self.interval)
                }
            }
        }
    }
}
```

The machine is pure: no clock, no HTTP. The worker (Task 10) feeds it responses and the elapsed time.

- [ ] **Step 9: Run the tests to see them pass**

Run: `cargo test -p github`

Expected: `10 passed` (3 link + 7 device flow).

- [ ] **Step 10: Lint**

Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings`

Expected: clean.

- [ ] **Step 11: Commit**

```bash
git add Cargo.lock crates/github
git commit -m "feat(github): error type, Link parsing and Device Flow state machine"
```


### Task 4: `github` HTTP client and token store

**Files:**
- Create: `crates/github/src/client.rs`, `crates/github/src/token_store.rs`
- Modify: `crates/github/src/lib.rs`
- Test: `crates/github/tests/client.rs`

**Interfaces:**
- Consumes: `GithubError`, `next_link`, `DeviceCode`, `PollResponse` (Task 3).
- Produces: `Client::github_com()`, `Client::with_bases(api_base: &str, web_base: &str)`, `request_device_code(&self, client_id: &str, scopes: &[&str]) -> Result<DeviceCode, GithubError>`, `poll_token(&self, client_id: &str, device_code: &str) -> Result<PollResponse, GithubError>`, `current_user(&self, token: &str) -> Result<User, GithubError>`, `list_repos(&self, token: &str) -> Result<Vec<RepoInfo>, GithubError>`; `User { login: String, name: Option<String> }`; `RepoInfo { full_name, name, owner, clone_url, updated_at: String, private: bool }`; `trait TokenStore: Send + Sync { load(&self) -> Result<Option<String>, TokenStoreError>; save(&self, &str) -> Result<(), TokenStoreError>; clear(&self) -> Result<(), TokenStoreError> }`; `KeyringStore::new(service, account)`; `MemoryStore::default()` / `MemoryStore::with_token(&str)`; `TokenStoreError(pub String)`. `Client` is `Clone` and stateless (token passed per call).

- [ ] **Step 1: Write the failing HTTP tests**

`crates/github/tests/client.rs`:

```rust
#![allow(clippy::unwrap_used)]

use github::{Client, GithubError, PollResponse};
use mockito::Matcher;

fn client(server: &mockito::Server) -> Client {
    Client::with_bases(&server.url(), &server.url())
}

fn repo_json(i: usize) -> serde_json::Value {
    serde_json::json!({
        "full_name": format!("ExampleOrg/repo{i}"),
        "name": format!("repo{i}"),
        "owner": { "login": "ExampleOrg" },
        "private": i.is_multiple_of(2),
        "clone_url": format!("https://github.com/ExampleOrg/repo{i}.git"),
        "updated_at": "2026-09-30T10:00:00Z",
        "extra_field_we_ignore": true
    })
}

#[test]
fn current_user_sends_bearer_token() {
    let mut server = mockito::Server::new();
    let m = server
        .mock("GET", "/user")
        .match_header("authorization", "Bearer gho_abc")
        .match_header("user-agent", Matcher::Regex("^RetroGit/".into()))
        .with_body(r#"{"login":"ada","name":"Ada L","id":1}"#)
        .create();
    let u = client(&server).current_user("gho_abc").unwrap();
    assert_eq!(u.login, "ada");
    assert_eq!(u.name.as_deref(), Some("Ada L"));
    m.assert();
}

#[test]
fn unauthorized_maps_to_error() {
    let mut server = mockito::Server::new();
    server
        .mock("GET", "/user")
        .with_status(401)
        .with_body("{}")
        .create();
    assert_eq!(
        client(&server).current_user("bad").err(),
        Some(GithubError::Unauthorized)
    );
}

#[test]
fn sso_header_maps_to_sso_required() {
    let mut server = mockito::Server::new();
    server
        .mock("GET", "/user")
        .with_status(403)
        .with_header(
            "X-GitHub-SSO",
            "required; url=https://github.com/orgs/ExampleOrg/sso?authorization_request=abc",
        )
        .with_body("{}")
        .create();
    assert_eq!(
        client(&server).current_user("t").err(),
        Some(GithubError::SsoRequired {
            url: "https://github.com/orgs/ExampleOrg/sso?authorization_request=abc".into()
        })
    );
}

#[test]
fn rate_limit_maps_to_error() {
    let mut server = mockito::Server::new();
    server
        .mock("GET", "/user")
        .with_status(403)
        .with_header("x-ratelimit-remaining", "0")
        .with_body("{}")
        .create();
    assert_eq!(
        client(&server).current_user("t").err(),
        Some(GithubError::RateLimited)
    );
}

#[test]
fn other_status_maps_to_http() {
    let mut server = mockito::Server::new();
    server.mock("GET", "/user").with_status(500).create();
    assert_eq!(
        client(&server).current_user("t").err(),
        Some(GithubError::Http(500))
    );
}

#[test]
fn bad_json_maps_to_decode() {
    let mut server = mockito::Server::new();
    server.mock("GET", "/user").with_body("not json").create();
    assert!(matches!(
        client(&server).current_user("t"),
        Err(GithubError::Decode(_))
    ));
}

#[test]
fn unreachable_server_maps_to_network() {
    let c = Client::with_bases("http://127.0.0.1:9", "http://127.0.0.1:9");
    assert!(matches!(c.current_user("t"), Err(GithubError::Network(_))));
}

#[test]
fn list_repos_follows_pagination() {
    let mut server = mockito::Server::new();
    let page1: Vec<_> = (0..100).map(repo_json).collect();
    let page2: Vec<_> = (100..130).map(repo_json).collect();
    let next = format!(
        "<{}/user/repos?page=2>; rel=\"next\", <{}/user/repos?page=2>; rel=\"last\"",
        server.url(),
        server.url()
    );
    let m1 = server
        .mock("GET", "/user/repos")
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded(
                "affiliation".into(),
                "owner,collaborator,organization_member".into(),
            ),
            Matcher::UrlEncoded("sort".into(), "updated".into()),
            Matcher::UrlEncoded("per_page".into(), "100".into()),
        ]))
        .with_header("link", &next)
        .with_body(serde_json::to_string(&page1).unwrap())
        .create();
    let m2 = server
        .mock("GET", "/user/repos")
        .match_query(Matcher::UrlEncoded("page".into(), "2".into()))
        .with_body(serde_json::to_string(&page2).unwrap())
        .create();
    let repos = client(&server).list_repos("t").unwrap();
    assert_eq!(repos.len(), 130);
    assert_eq!(repos[0].owner, "ExampleOrg");
    assert_eq!(repos[0].full_name, "ExampleOrg/repo0");
    assert!(repos[0].private);
    assert_eq!(repos[129].name, "repo129");
    m1.assert();
    m2.assert();
}

#[test]
fn device_code_request_and_poll() {
    let mut server = mockito::Server::new();
    server
        .mock("POST", "/login/device/code")
        .match_header("accept", "application/json")
        .match_body(Matcher::AllOf(vec![
            Matcher::UrlEncoded("client_id".into(), "Iv1.test".into()),
            Matcher::UrlEncoded("scope".into(), "repo read:org".into()),
        ]))
        .with_body(r#"{"device_code":"dc1","user_code":"ABCD-1234","verification_uri":"https://github.com/login/device","expires_in":900,"interval":5}"#)
        .create();
    server
        .mock("POST", "/login/oauth/access_token")
        .match_body(Matcher::AllOf(vec![
            Matcher::UrlEncoded("device_code".into(), "dc1".into()),
            Matcher::UrlEncoded(
                "grant_type".into(),
                "urn:ietf:params:oauth:grant-type:device_code".into(),
            ),
        ]))
        .with_body(r#"{"error":"authorization_pending"}"#)
        .create();
    let c = client(&server);
    let code = c.request_device_code("Iv1.test", github::SCOPES).unwrap();
    assert_eq!(code.user_code, "ABCD-1234");
    assert_eq!(code.interval, 5);
    assert_eq!(
        c.poll_token("Iv1.test", "dc1").unwrap(),
        PollResponse::Pending
    );
}
```

- [ ] **Step 2: Write the failing tests in `crates/github/src/token_store.rs`**

Create `crates/github/src/token_store.rs` containing only this test module for now (the implementation goes above it in a later step).

`crates/github/src/token_store.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_store_roundtrip() {
        let s = MemoryStore::default();
        assert_eq!(s.load(), Ok(None));
        s.save("gho_1").ok();
        assert_eq!(s.load(), Ok(Some("gho_1".into())));
        s.clear().ok();
        s.clear().ok();
        assert_eq!(s.load(), Ok(None));
    }
}
```

- [ ] **Step 3: Export the new modules**

`crates/github/src/lib.rs`:

```rust
//! GitHub access for RetroGit: OAuth Device Flow, REST API, token storage.

mod client;
mod device_flow;
mod error;
mod link;
mod token_store;

pub use client::{Client, RepoInfo, User};
pub use device_flow::{DeviceCode, DeviceFlow, DeviceFlowFailure, PollResponse, Step};
pub use error::GithubError;
pub use link::next_link;
pub use token_store::{KeyringStore, MemoryStore, TokenStore, TokenStoreError};

/// OAuth scopes requested by RetroGit.
pub const SCOPES: &[&str] = &["repo", "read:org"];
```

- [ ] **Step 4: Run the tests to see them fail**

Run: `cargo test -p github`

Expected: compile error `file not found for module client`.

- [ ] **Step 5: Implement the client**

`http_status_as_error(false)` lets us read headers of 4xx responses (needed for `X-GitHub-SSO` and rate limit). `RootCerts::PlatformVerifier` uses the OS trust store, which matters behind a corporate TLS-inspection proxy.

`crates/github/src/client.rs`:

```rust
use std::time::Duration;

use serde::Deserialize;
use ureq::http::Response;
use ureq::tls::{RootCerts, TlsConfig};
use ureq::{Agent, Body};

use crate::device_flow::{DeviceCode, PollResponse};
use crate::{GithubError, next_link};

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct User {
    pub login: String,
    pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoInfo {
    /// `owner/name`
    pub full_name: String,
    pub name: String,
    pub owner: String,
    pub private: bool,
    /// HTTPS clone URL (no credentials).
    pub clone_url: String,
    /// ISO-8601 timestamp, e.g. `2026-09-30T10:00:00Z`.
    pub updated_at: String,
}

#[derive(Deserialize)]
struct RawRepo {
    full_name: String,
    name: String,
    owner: RawOwner,
    private: bool,
    clone_url: String,
    updated_at: String,
}

#[derive(Deserialize)]
struct RawOwner {
    login: String,
}

impl From<RawRepo> for RepoInfo {
    fn from(r: RawRepo) -> Self {
        RepoInfo {
            full_name: r.full_name,
            name: r.name,
            owner: r.owner.login,
            private: r.private,
            clone_url: r.clone_url,
            updated_at: r.updated_at,
        }
    }
}

/// Stateless GitHub client: the token is passed to each call.
#[derive(Clone)]
pub struct Client {
    agent: Agent,
    api_base: String,
    web_base: String,
}

const USER_AGENT: &str = concat!("RetroGit/", env!("CARGO_PKG_VERSION"));
const REPOS_PATH: &str =
    "/user/repos?affiliation=owner,collaborator,organization_member&sort=updated&per_page=100";

impl Client {
    /// Client for github.com.
    pub fn github_com() -> Client {
        Client::with_bases("https://api.github.com", "https://github.com")
    }

    /// Client with custom base URLs (tests use a local mock server).
    pub fn with_bases(api_base: &str, web_base: &str) -> Client {
        let agent: Agent = Agent::config_builder()
            .http_status_as_error(false)
            .user_agent(USER_AGENT)
            .timeout_connect(Some(Duration::from_secs(10)))
            .timeout_global(Some(Duration::from_secs(60)))
            // OS trust store: works behind corporate TLS inspection proxies.
            .tls_config(
                TlsConfig::builder()
                    .root_certs(RootCerts::PlatformVerifier)
                    .build(),
            )
            .build()
            .into();
        Client {
            agent,
            api_base: api_base.trim_end_matches('/').to_string(),
            web_base: web_base.trim_end_matches('/').to_string(),
        }
    }

    pub fn request_device_code(
        &self,
        client_id: &str,
        scopes: &[&str],
    ) -> Result<DeviceCode, GithubError> {
        let scope = scopes.join(" ");
        let resp = self
            .agent
            .post(format!("{}/login/device/code", self.web_base))
            .header("Accept", "application/json")
            .send_form([("client_id", client_id), ("scope", scope.as_str())])?;
        let mut resp = check(resp)?;
        Ok(resp.body_mut().read_json::<DeviceCode>()?)
    }

    pub fn poll_token(
        &self,
        client_id: &str,
        device_code: &str,
    ) -> Result<PollResponse, GithubError> {
        let resp = self
            .agent
            .post(format!("{}/login/oauth/access_token", self.web_base))
            .header("Accept", "application/json")
            .send_form([
                ("client_id", client_id),
                ("device_code", device_code),
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ])?;
        let mut resp = check(resp)?;
        let v: serde_json::Value = resp.body_mut().read_json()?;
        Ok(PollResponse::from_json(&v))
    }

    pub fn current_user(&self, token: &str) -> Result<User, GithubError> {
        let mut resp = self.api_get(&format!("{}/user", self.api_base), token)?;
        Ok(resp.body_mut().read_json::<User>()?)
    }

    /// All repositories the user can access, following pagination.
    pub fn list_repos(&self, token: &str) -> Result<Vec<RepoInfo>, GithubError> {
        let mut url = format!("{}{}", self.api_base, REPOS_PATH);
        let mut out = Vec::new();
        loop {
            let mut resp = self.api_get(&url, token)?;
            let next = resp
                .headers()
                .get("link")
                .and_then(|v| v.to_str().ok())
                .and_then(next_link);
            let page: Vec<RawRepo> = resp.body_mut().read_json()?;
            out.extend(page.into_iter().map(RepoInfo::from));
            match next {
                Some(n) => url = n,
                None => return Ok(out),
            }
        }
    }

    fn api_get(&self, url: &str, token: &str) -> Result<Response<Body>, GithubError> {
        let resp = self
            .agent
            .get(url)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .header("Authorization", format!("Bearer {token}"))
            .call()?;
        check(resp)
    }
}

fn header<'a>(resp: &'a Response<Body>, name: &str) -> Option<&'a str> {
    resp.headers().get(name).and_then(|v| v.to_str().ok())
}

/// Map non-2xx statuses to typed errors.
fn check(resp: Response<Body>) -> Result<Response<Body>, GithubError> {
    let status = resp.status().as_u16();
    if (200..300).contains(&status) {
        return Ok(resp);
    }
    if status == 401 {
        return Err(GithubError::Unauthorized);
    }
    if status == 403
        && let Some(sso) = header(&resp, "x-github-sso")
    {
        let url = sso.split("url=").nth(1).unwrap_or("").trim().to_string();
        return Err(GithubError::SsoRequired { url });
    }
    if (status == 403 || status == 429) && header(&resp, "x-ratelimit-remaining") == Some("0") {
        return Err(GithubError::RateLimited);
    }
    Err(GithubError::Http(status))
}
```

- [ ] **Step 6: Implement `crates/github/src/token_store.rs`**

Insert this **above** the existing `#[cfg(test)]` module in `crates/github/src/token_store.rs`.

`crates/github/src/token_store.rs`:

```rust
use std::sync::Mutex;

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
#[error("credential store error: {0}")]
pub struct TokenStoreError(pub String);

/// Where the GitHub token lives between runs.
pub trait TokenStore: Send + Sync {
    fn load(&self) -> Result<Option<String>, TokenStoreError>;
    fn save(&self, token: &str) -> Result<(), TokenStoreError>;
    /// Removing a token that does not exist is not an error.
    fn clear(&self) -> Result<(), TokenStoreError>;
}

/// macOS Keychain / Windows Credential Manager.
pub struct KeyringStore {
    service: String,
    account: String,
}

impl KeyringStore {
    pub fn new(service: &str, account: &str) -> KeyringStore {
        KeyringStore {
            service: service.to_string(),
            account: account.to_string(),
        }
    }

    fn entry(&self) -> Result<keyring::Entry, TokenStoreError> {
        keyring::Entry::new(&self.service, &self.account)
            .map_err(|e| TokenStoreError(e.to_string()))
    }
}

impl TokenStore for KeyringStore {
    fn load(&self) -> Result<Option<String>, TokenStoreError> {
        match self.entry()?.get_password() {
            Ok(t) => Ok(Some(t)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(TokenStoreError(e.to_string())),
        }
    }

    fn save(&self, token: &str) -> Result<(), TokenStoreError> {
        self.entry()?
            .set_password(token)
            .map_err(|e| TokenStoreError(e.to_string()))
    }

    fn clear(&self) -> Result<(), TokenStoreError> {
        match self.entry()?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(TokenStoreError(e.to_string())),
        }
    }
}

/// In-memory store for tests.
#[derive(Default)]
pub struct MemoryStore(Mutex<Option<String>>);

impl MemoryStore {
    pub fn with_token(token: &str) -> MemoryStore {
        MemoryStore(Mutex::new(Some(token.to_string())))
    }
}

impl TokenStore for MemoryStore {
    fn load(&self) -> Result<Option<String>, TokenStoreError> {
        Ok(self
            .0
            .lock()
            .map_err(|e| TokenStoreError(e.to_string()))?
            .clone())
    }

    fn save(&self, token: &str) -> Result<(), TokenStoreError> {
        *self.0.lock().map_err(|e| TokenStoreError(e.to_string()))? = Some(token.to_string());
        Ok(())
    }

    fn clear(&self) -> Result<(), TokenStoreError> {
        *self.0.lock().map_err(|e| TokenStoreError(e.to_string()))? = None;
        Ok(())
    }
}
```

- [ ] **Step 7: Run the tests to see them pass**

Run: `cargo test -p github`

Expected: unit tests `11 passed`, `tests/client.rs` `9 passed`.

- [ ] **Step 8: Lint**

Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings`

Expected: clean.

- [ ] **Step 9: Commit**

```bash
git add Cargo.lock crates/github
git commit -m "feat(github): REST client with typed errors and keychain token store"
```


### Task 5: `win95` theme, bevels, frames and push button

**Files:**
- Create: `crates/win95/Cargo.toml`, `crates/win95/src/lib.rs`, `crates/win95/src/theme.rs`, `crates/win95/src/bevel.rs`, `crates/win95/src/panel.rs`, `crates/win95/src/button.rs`
- Create: `crates/win95/assets/W95FA.otf`, `crates/win95/assets/OFL.txt` (downloaded)
- Test: `crates/win95/tests/theme.rs`

**Interfaces:**
- Consumes: nothing.
- Produces: `win95::theme::{install(&egui::Context), font(size: f32) -> FontId, FONT_SIZE, SILVER, LIGHT, WHITE, GRAY, BLACK, NAVY, TITLE_END, INACTIVE_TITLE, INACTIVE_TITLE_END}`; `Bevel::{Raised, Pressed, Field, Window, Shallow, Sunken}`, `bevel::paint(&Painter, Rect, Bevel)`, `bevel::rings(Bevel)`, `bevel::thickness(Bevel) -> f32`; `win95::bevel_frame(ui, Bevel, fill: Color32, margin: i8, add) -> InnerResponse<R>`; `Button95::new(text).enabled(bool).min_size(Vec2)` implementing `egui::Widget` (accessible label = text, so kittest can `get_by_label`).

- [ ] **Step 1: Download the font and its licence**

```bash
cd /tmp && curl -sL -o W95FA.zip https://fontsarena.com/wp-content/uploads/2020/01/W95FA.zip
shasum -a 256 W95FA.zip
```

Expected hash: `a78972d3d46cc506f9aef423100b027696fad437b16b078e3bdf396c0bf6d3eb`. If it differs, stop and ask (the upstream file changed).

```bash
unzip -o -q W95FA.zip -d W95FA
mkdir -p ~/perso/retrogit/crates/win95/assets
cp W95FA/W95FA/W95FA.otf W95FA/W95FA/OFL.txt ~/perso/retrogit/crates/win95/assets/
```

- [ ] **Step 2: Create the manifest**

`crates/win95/Cargo.toml`:

```toml
[package]
name = "win95"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
publish.workspace = true
description = "Windows 95 look-and-feel widgets for egui"

[dependencies]
egui.workspace = true

[dev-dependencies]
egui_kittest.workspace = true

[lints]
workspace = true
```

- [ ] **Step 3: Write the failing theme test**

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
```

- [ ] **Step 4: Write the failing tests in `crates/win95/src/bevel.rs`**

Create `crates/win95/src/bevel.rs` containing only this test module for now (the implementation goes above it in a later step).

`crates/win95/src/bevel.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pressed_is_raised_inverted() {
        let raised = rings(Bevel::Raised);
        let pressed = rings(Bevel::Pressed);
        for (r, p) in raised.iter().zip(pressed) {
            assert_eq!((r.1, r.0), (p.0, p.1));
        }
    }

    #[test]
    fn raised_is_lit_from_top_left() {
        assert_eq!(rings(Bevel::Raised)[0].0, WHITE);
        assert_eq!(rings(Bevel::Field)[0].1, WHITE);
        assert_eq!(thickness(Bevel::Shallow), 1.0);
        assert_eq!(thickness(Bevel::Window), 2.0);
    }
}
```

- [ ] **Step 5: Write the failing tests in `crates/win95/src/button.rs`**

Create `crates/win95/src/button.rs` containing only this test module for now (the implementation goes above it in a later step).

`crates/win95/src/button.rs`:

```rust
#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    use super::Button95;

    #[test]
    fn enabled_button_reports_click() {
        let mut h = Harness::new_ui_state(
            |ui, clicks: &mut u32| {
                if ui.add(Button95::new("OK")).clicked() {
                    *clicks += 1;
                }
            },
            0,
        );
        h.get_by_label("OK").click();
        h.run();
        assert_eq!(*h.state(), 1);
    }

    #[test]
    fn disabled_button_ignores_clicks() {
        let mut h = Harness::new_ui_state(
            |ui, clicks: &mut u32| {
                if ui.add(Button95::new("Push").enabled(false)).clicked() {
                    *clicks += 1;
                }
            },
            0,
        );
        h.get_by_label("Push").click();
        h.run();
        assert_eq!(*h.state(), 0);
    }
}
```

- [ ] **Step 6: Create the crate root (Task 5 version)**

`crates/win95/src/lib.rs`:

```rust
//! Windows 95 look-and-feel for egui. No business logic lives here.

pub mod bevel;
pub mod button;
pub mod panel;
pub mod theme;

pub use bevel::Bevel;
pub use button::Button95;
pub use panel::bevel_frame;
```

- [ ] **Step 7: Run the tests to see them fail**

Run: `cargo test -p win95`

Expected: compile errors (missing `theme`, `panel`, `rings`, `Button95`).

- [ ] **Step 8: Implement the theme**

W95FA goes first in both families; egui's default fonts stay as fallback for glyphs W95FA lacks. `animation_time = 0` keeps the UI snappy and avoids idle repaints.

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
    for family in [FontFamily::Proportional, FontFamily::Monospace] {
        // W95FA first, egui's default fonts stay as fallback for missing glyphs.
        fonts
            .families
            .entry(family)
            .or_default()
            .insert(0, FONT_NAME.to_owned());
    }
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
    v.override_text_color = Some(BLACK);
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

- [ ] **Step 9: Implement `crates/win95/src/bevel.rs`**

Insert this **above** the existing `#[cfg(test)]` module in `crates/win95/src/bevel.rs`.

`crates/win95/src/bevel.rs`:

```rust
//! The 3D edges that make Win95 look like Win95.

use egui::{Color32, Painter, Rect, Vec2, pos2};

use crate::theme::{BLACK, GRAY, LIGHT, WHITE};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bevel {
    /// Button at rest.
    Raised,
    /// Button held down.
    Pressed,
    /// Text fields, list views: white well.
    Field,
    /// Dialog / main window border.
    Window,
    /// Thin 1-ring well (status bar cells, progress bar).
    Shallow,
    /// Group or panel slightly sunk.
    Sunken,
}

/// Edge colors as rings from outermost to innermost: `(top_left, bottom_right)`.
pub fn rings(kind: Bevel) -> &'static [(Color32, Color32)] {
    match kind {
        Bevel::Raised => &[(WHITE, BLACK), (LIGHT, GRAY)],
        Bevel::Pressed => &[(BLACK, WHITE), (GRAY, LIGHT)],
        Bevel::Field => &[(GRAY, WHITE), (BLACK, LIGHT)],
        Bevel::Window => &[(LIGHT, BLACK), (WHITE, GRAY)],
        Bevel::Shallow => &[(GRAY, WHITE)],
        Bevel::Sunken => &[(GRAY, WHITE), (BLACK, LIGHT)],
    }
}

/// Width in points of the whole bevel.
pub fn thickness(kind: Bevel) -> f32 {
    rings(kind).len() as f32
}

/// Paint the bevel along the inside of `rect`.
pub fn paint(painter: &Painter, rect: Rect, kind: Bevel) {
    let mut r = rect;
    for &(tl, br) in rings(kind) {
        if r.width() < 2.0 || r.height() < 2.0 {
            return;
        }
        // 1-point filled strips: crisp at any pixels_per_point, no anti-aliased lines.
        painter.rect_filled(
            Rect::from_min_max(r.min, pos2(r.max.x, r.min.y + 1.0)),
            0.0,
            tl,
        );
        painter.rect_filled(
            Rect::from_min_max(r.min, pos2(r.min.x + 1.0, r.max.y)),
            0.0,
            tl,
        );
        painter.rect_filled(
            Rect::from_min_max(pos2(r.min.x, r.max.y - 1.0), r.max),
            0.0,
            br,
        );
        painter.rect_filled(
            Rect::from_min_max(pos2(r.max.x - 1.0, r.min.y), r.max),
            0.0,
            br,
        );
        r = r.shrink2(Vec2::splat(1.0));
    }
}
```

- [ ] **Step 10: Implement the bevelled frame**

`crates/win95/src/panel.rs`:

```rust
use egui::{Color32, InnerResponse, Margin, Ui};

use crate::bevel::{self, Bevel};

/// Lay out `add` inside a filled, bevelled box.
/// `margin` is the space between the bevel and the content (the bevel is added on top).
pub fn bevel_frame<R>(
    ui: &mut Ui,
    kind: Bevel,
    fill: Color32,
    margin: i8,
    add: impl FnOnce(&mut Ui) -> R,
) -> InnerResponse<R> {
    let edge = bevel::thickness(kind) as i8;
    let inner = egui::Frame::NONE
        .fill(fill)
        .inner_margin(Margin::same(margin + edge))
        .show(ui, add);
    bevel::paint(ui.painter(), inner.response.rect, kind);
    inner
}
```

- [ ] **Step 11: Implement `crates/win95/src/button.rs`**

Insert this **above** the existing `#[cfg(test)]` module in `crates/win95/src/button.rs`.

`crates/win95/src/button.rs`:

```rust
use egui::{Align2, Response, Sense, Ui, Vec2, Widget, WidgetInfo, WidgetType, vec2};

use crate::bevel::{self, Bevel};
use crate::theme::{self, BLACK, GRAY, SILVER, WHITE};

/// Classic push button: 75×23 minimum, pressed look while held, embossed text when disabled.
pub struct Button95 {
    text: String,
    enabled: bool,
    min_size: Vec2,
}

impl Button95 {
    pub fn new(text: impl Into<String>) -> Button95 {
        Button95 {
            text: text.into(),
            enabled: true,
            min_size: vec2(75.0, 23.0),
        }
    }

    pub fn enabled(mut self, enabled: bool) -> Button95 {
        self.enabled = enabled;
        self
    }

    pub fn min_size(mut self, size: Vec2) -> Button95 {
        self.min_size = size;
        self
    }
}

impl Widget for Button95 {
    fn ui(self, ui: &mut Ui) -> Response {
        let font = theme::font(theme::FONT_SIZE);
        let galley = ui
            .painter()
            .layout_no_wrap(self.text.clone(), font.clone(), BLACK);
        let size = (galley.size() + vec2(16.0, 8.0)).max(self.min_size);
        let sense = if self.enabled {
            Sense::click()
        } else {
            Sense::hover()
        };
        let (rect, response) = ui.allocate_exact_size(size, sense);
        let (enabled, text) = (self.enabled, self.text.clone());
        response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, enabled, &text));

        if ui.is_rect_visible(rect) {
            let pressed = self.enabled && response.is_pointer_button_down_on();
            let p = ui.painter();
            p.rect_filled(rect, 0.0, SILVER);
            bevel::paint(
                p,
                rect,
                if pressed {
                    Bevel::Pressed
                } else {
                    Bevel::Raised
                },
            );
            let center = rect.center() + if pressed { vec2(1.0, 1.0) } else { Vec2::ZERO };
            if self.enabled {
                p.text(center, Align2::CENTER_CENTER, &self.text, font, BLACK);
            } else {
                p.text(
                    center + vec2(1.0, 1.0),
                    Align2::CENTER_CENTER,
                    &self.text,
                    font.clone(),
                    WHITE,
                );
                p.text(center, Align2::CENTER_CENTER, &self.text, font, GRAY);
            }
        }
        response
    }
}
```

- [ ] **Step 12: Run the tests to see them pass**

Run: `cargo test -p win95`

Expected: unit tests `4 passed`, `tests/theme.rs` `1 passed`.

- [ ] **Step 13: Lint**

Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings`

Expected: clean.

- [ ] **Step 14: Commit**

```bash
git add Cargo.lock crates/win95
git commit -m "feat(win95): theme with W95FA font, bevels, frames and push button"
```


### Task 6: `win95` title bar, dialog, icons, status bar, resize edges

**Files:**
- Create: `crates/win95/src/title_bar.rs`, `crates/win95/src/dialog.rs`, `crates/win95/src/icon.rs`, `crates/win95/src/status_bar.rs`, `crates/win95/src/window_frame.rs`
- Modify: `crates/win95/src/lib.rs`

**Interfaces:**
- Consumes: `bevel`, `bevel_frame`, `theme` (Task 5).
- Produces: `TitleBar::new(&str).active(bool).close_only().show(ui) -> TitleAction`; `TitleAction::{None, Minimize, ToggleMaximize, Close, StartDrag}` (caption buttons are accessible as "Close", "Maximize", "Minimize"); `Dialog::new(id: impl egui::AsId, title: &str).width(f32).show(ctx, add) -> DialogResponse<R>` with `DialogResponse { inner: R, close_requested: bool }` (Close button or Escape); `Icon::{Error, Warning, Info}` + `win95::icon::icon(ui, Icon)`; `win95::status_bar(ui, &[(&str, Option<f32>)])` (`None` = flexible width); `win95::resize_edges(ui, Rect)`.

- [ ] **Step 1: Write the failing tests in `crates/win95/src/title_bar.rs`**

Create `crates/win95/src/title_bar.rs` containing only this test module for now (the implementation goes above it in a later step).

`crates/win95/src/title_bar.rs`:

```rust
#[cfg(test)]
mod tests {
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    use super::{TitleAction, TitleBar};

    fn click(label: &str, close_only: bool) -> TitleAction {
        let mut h = Harness::new_ui_state(
            move |ui, last: &mut TitleAction| {
                let mut bar = TitleBar::new("RetroGit");
                if close_only {
                    bar = bar.close_only();
                }
                let a = bar.show(ui);
                if a != TitleAction::None {
                    *last = a;
                }
            },
            TitleAction::None,
        );
        h.get_by_label(label).click();
        h.run();
        *h.state()
    }

    #[test]
    fn caption_buttons_report_their_action() {
        assert_eq!(click("Close", false), TitleAction::Close);
        assert_eq!(click("Minimize", false), TitleAction::Minimize);
        assert_eq!(click("Maximize", false), TitleAction::ToggleMaximize);
    }

    #[test]
    fn dialogs_only_have_close() {
        let mut h = Harness::new_ui(|ui| {
            TitleBar::new("Dialog").close_only().show(ui);
        });
        h.run();
        assert!(h.query_by_label("Minimize").is_none());
        assert!(h.query_by_label("Close").is_some());
    }
}
```

- [ ] **Step 2: Write the failing tests in `crates/win95/src/dialog.rs`**

Create `crates/win95/src/dialog.rs` containing only this test module for now (the implementation goes above it in a later step).

`crates/win95/src/dialog.rs`:

```rust
#[cfg(test)]
mod tests {
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    use super::Dialog;

    #[test]
    fn close_button_requests_close_and_content_is_shown() {
        let mut h = Harness::new_ui_state(
            |ui, closed: &mut bool| {
                let ctx = ui.ctx().clone();
                let r = Dialog::new("about", "About RetroGit").show(&ctx, |ui| {
                    ui.label("RetroGit 0.1.0");
                });
                if r.close_requested {
                    *closed = true;
                }
            },
            false,
        );
        h.run();
        assert!(h.query_by_label("RetroGit 0.1.0").is_some());
        h.get_by_label("Close").click();
        h.run();
        assert!(*h.state());
    }

    #[test]
    fn escape_requests_close() {
        let mut h = Harness::new_ui_state(
            |ui, closed: &mut bool| {
                let ctx = ui.ctx().clone();
                if Dialog::new("d", "D")
                    .show(&ctx, |ui| ui.label("x"))
                    .close_requested
                {
                    *closed = true;
                }
            },
            false,
        );
        h.run();
        h.key_press(egui::Key::Escape);
        h.run();
        assert!(*h.state());
    }
}
```

- [ ] **Step 3: Create the crate root (Task 6 version)**

`crates/win95/src/lib.rs`:

```rust
//! Windows 95 look-and-feel for egui. No business logic lives here.

pub mod bevel;
pub mod button;
pub mod dialog;
pub mod icon;
pub mod panel;
pub mod status_bar;
pub mod theme;
pub mod title_bar;
pub mod window_frame;

pub use bevel::Bevel;
pub use button::Button95;
pub use dialog::{Dialog, DialogResponse};
pub use icon::Icon;
pub use panel::bevel_frame;
pub use status_bar::status_bar;
pub use title_bar::{TitleAction, TitleBar};
pub use window_frame::resize_edges;
```

- [ ] **Step 4: Run the tests to see them fail**

Run: `cargo test -p win95`

Expected: compile errors (missing modules `icon`, `status_bar`, `window_frame`, missing `TitleBar`, `Dialog`).

- [ ] **Step 5: Implement the title bar**

Insert this **above** the existing `#[cfg(test)]` module in `crates/win95/src/title_bar.rs`.

`crates/win95/src/title_bar.rs`:

```rust
use egui::{Align2, Color32, Mesh, Rect, Sense, Stroke, Ui, WidgetInfo, WidgetType, pos2, vec2};

use crate::bevel::{self, Bevel};
use crate::theme::{
    self, BLACK, INACTIVE_TITLE, INACTIVE_TITLE_END, NAVY, SILVER, TITLE_END, WHITE,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TitleAction {
    None,
    Minimize,
    ToggleMaximize,
    Close,
    /// The user started dragging the title bar.
    StartDrag,
}

/// Blue gradient title bar with caption buttons.
pub struct TitleBar<'a> {
    title: &'a str,
    active: bool,
    close_only: bool,
}

pub const HEIGHT: f32 = 18.0;

impl<'a> TitleBar<'a> {
    pub fn new(title: &'a str) -> TitleBar<'a> {
        TitleBar {
            title,
            active: true,
            close_only: false,
        }
    }

    pub fn active(mut self, active: bool) -> Self {
        self.active = active;
        self
    }

    /// Dialogs only have the close button.
    pub fn close_only(mut self) -> Self {
        self.close_only = true;
        self
    }

    pub fn show(self, ui: &mut Ui) -> TitleAction {
        let width = ui.available_width();
        let (rect, bar) = ui.allocate_exact_size(vec2(width, HEIGHT), Sense::click_and_drag());
        let (start, end) = if self.active {
            (NAVY, TITLE_END)
        } else {
            (INACTIVE_TITLE, INACTIVE_TITLE_END)
        };
        ui.painter().add(gradient(rect, start, end));
        ui.painter().text(
            rect.left_center() + vec2(4.0, 0.0),
            Align2::LEFT_CENTER,
            self.title,
            theme::font(theme::FONT_SIZE),
            if self.active { WHITE } else { SILVER },
        );

        let mut action = TitleAction::None;
        let btn = vec2(16.0, 14.0);
        let mut x = rect.right() - 2.0 - btn.x;
        let y = rect.center().y - btn.y / 2.0;
        let mut buttons: Vec<(Glyph, &str, TitleAction)> =
            vec![(Glyph::Close, "Close", TitleAction::Close)];
        if !self.close_only {
            buttons.push((Glyph::Maximize, "Maximize", TitleAction::ToggleMaximize));
            buttons.push((Glyph::Minimize, "Minimize", TitleAction::Minimize));
        }
        for (i, (glyph, label, a)) in buttons.into_iter().enumerate() {
            let r = Rect::from_min_size(pos2(x, y), btn);
            if caption_button(ui, r, glyph, label) {
                action = a;
            }
            // Win95 leaves a 2px gap between Close and the others.
            x -= btn.x + if i == 0 { 2.0 } else { 0.0 };
        }

        if action == TitleAction::None {
            if bar.double_clicked() && !self.close_only {
                action = TitleAction::ToggleMaximize;
            } else if bar.drag_started() {
                action = TitleAction::StartDrag;
            }
        }
        action
    }
}

#[derive(Clone, Copy)]
enum Glyph {
    Minimize,
    Maximize,
    Close,
}

fn caption_button(ui: &mut Ui, rect: Rect, glyph: Glyph, label: &str) -> bool {
    let id = ui.id().with(("caption", label));
    let resp = ui.interact(rect, id, Sense::click());
    let label_owned = label.to_string();
    resp.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, &label_owned));
    let pressed = resp.is_pointer_button_down_on();
    let p = ui.painter();
    p.rect_filled(rect, 0.0, SILVER);
    bevel::paint(
        p,
        rect,
        if pressed {
            Bevel::Pressed
        } else {
            Bevel::Raised
        },
    );
    let c = rect.center()
        + if pressed {
            vec2(1.0, 1.0)
        } else {
            vec2(0.0, 0.0)
        };
    let s = Stroke::new(1.0, BLACK);
    match glyph {
        Glyph::Minimize => {
            p.rect_filled(
                Rect::from_min_size(c + vec2(-4.0, 2.0), vec2(6.0, 2.0)),
                0.0,
                BLACK,
            );
        }
        Glyph::Maximize => {
            let r = Rect::from_min_size(c + vec2(-4.5, -4.5), vec2(9.0, 8.0));
            p.rect_stroke(r, 0.0, s, egui::StrokeKind::Inside);
            p.rect_filled(Rect::from_min_size(r.min, vec2(9.0, 2.0)), 0.0, BLACK);
        }
        Glyph::Close => {
            let s = Stroke::new(1.5, BLACK);
            p.line_segment([c + vec2(-3.5, -3.0), c + vec2(3.5, 3.0)], s);
            p.line_segment([c + vec2(3.5, -3.0), c + vec2(-3.5, 3.0)], s);
        }
    }
    resp.clicked()
}

fn gradient(rect: Rect, left: Color32, right: Color32) -> Mesh {
    let mut mesh = Mesh::default();
    mesh.colored_vertex(rect.left_top(), left);
    mesh.colored_vertex(rect.right_top(), right);
    mesh.colored_vertex(rect.right_bottom(), right);
    mesh.colored_vertex(rect.left_bottom(), left);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    mesh
}
```

The gradient is a 4-vertex mesh (egui has no gradient fill). Caption glyphs are painted with rects/lines so they never depend on font coverage.

- [ ] **Step 6: Implement the modal dialog**

Insert this **above** the existing `#[cfg(test)]` module in `crates/win95/src/dialog.rs`.

`crates/win95/src/dialog.rs`:

```rust
use egui::{Context, Id, Key, Modal};

use crate::bevel::Bevel;
use crate::panel::bevel_frame;
use crate::theme::SILVER;
use crate::title_bar::{TitleAction, TitleBar};

/// A modal Win95 dialog drawn inside the main window.
pub struct Dialog<'a> {
    id: Id,
    title: &'a str,
    width: f32,
}

pub struct DialogResponse<R> {
    pub inner: R,
    /// Close button or Escape.
    pub close_requested: bool,
}

impl<'a> Dialog<'a> {
    pub fn new(id: impl egui::AsId, title: &'a str) -> Dialog<'a> {
        Dialog {
            id: Id::new(id),
            title,
            width: 360.0,
        }
    }

    pub fn width(mut self, width: f32) -> Self {
        self.width = width;
        self
    }

    pub fn show<R>(self, ctx: &Context, add: impl FnOnce(&mut egui::Ui) -> R) -> DialogResponse<R> {
        let mut close = false;
        let resp = Modal::new(self.id)
            .backdrop_color(egui::Color32::TRANSPARENT)
            .frame(egui::Frame::NONE)
            .show(ctx, |ui| {
                bevel_frame(ui, Bevel::Window, SILVER, 1, |ui| {
                    ui.set_width(self.width);
                    if TitleBar::new(self.title).close_only().show(ui) == TitleAction::Close {
                        close = true;
                    }
                    egui::Frame::NONE
                        .inner_margin(egui::Margin::same(8))
                        .show(ui, add)
                        .inner
                })
                .inner
            });
        if ctx.input(|i| i.key_pressed(Key::Escape)) {
            close = true;
        }
        DialogResponse {
            inner: resp.inner,
            close_requested: close,
        }
    }
}
```

`egui::Modal` blocks input to the window behind; a transparent backdrop keeps the Win95 look (Win95 never dimmed the screen).

- [ ] **Step 7: Implement the message-box icons**

`crates/win95/src/icon.rs`:

```rust
use egui::{Align2, Color32, Pos2, Stroke, Ui, pos2, vec2};

use crate::theme::{self, BLACK, NAVY, WHITE};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Error,
    Warning,
    Info,
}

const RED: Color32 = Color32::from_rgb(0xFF, 0x00, 0x00);
const YELLOW: Color32 = Color32::from_rgb(0xFF, 0xFF, 0x00);

/// Paint a 32×32 message-box icon at the current cursor position.
pub fn icon(ui: &mut Ui, kind: Icon) {
    let (rect, _) = ui.allocate_exact_size(vec2(32.0, 32.0), egui::Sense::hover());
    let p = ui.painter();
    let c = rect.center();
    let bold = theme::font(20.0);
    match kind {
        Icon::Error => {
            p.circle_filled(c, 15.0, RED);
            p.circle_stroke(c, 15.0, Stroke::new(1.0, BLACK));
            let d = 6.0;
            let s = Stroke::new(3.0, WHITE);
            p.line_segment([c + vec2(-d, -d), c + vec2(d, d)], s);
            p.line_segment([c + vec2(d, -d), c + vec2(-d, d)], s);
        }
        Icon::Warning => {
            let pts: Vec<Pos2> = vec![
                pos2(c.x, rect.top() + 1.0),
                rect.right_bottom() - vec2(1.0, 2.0),
                rect.left_bottom() + vec2(1.0, -2.0),
            ];
            p.add(egui::Shape::convex_polygon(
                pts,
                YELLOW,
                Stroke::new(1.0, BLACK),
            ));
            p.text(c + vec2(0.0, 4.0), Align2::CENTER_CENTER, "!", bold, BLACK);
        }
        Icon::Info => {
            p.circle_filled(c, 15.0, WHITE);
            p.circle_stroke(c, 15.0, Stroke::new(1.0, BLACK));
            p.text(c, Align2::CENTER_CENTER, "i", bold, NAVY);
        }
    }
}
```

- [ ] **Step 8: Implement the status bar**

`crates/win95/src/status_bar.rs`:

```rust
use egui::{Align2, Rect, Ui, pos2, vec2};

use crate::bevel::{self, Bevel};
use crate::theme::{self, BLACK};

/// Status bar made of sunken cells. `None` width = take the remaining space.
pub fn status_bar(ui: &mut Ui, cells: &[(&str, Option<f32>)]) {
    let height = 20.0;
    let (rect, _) =
        ui.allocate_exact_size(vec2(ui.available_width(), height), egui::Sense::hover());
    let fixed: f32 = cells.iter().filter_map(|c| c.1).sum();
    let flexible = cells.iter().filter(|c| c.1.is_none()).count().max(1) as f32;
    let gap = 2.0;
    let spare =
        (rect.width() - fixed - gap * (cells.len().saturating_sub(1)) as f32).max(0.0) / flexible;
    let mut x = rect.left();
    for (text, width) in cells {
        let w = width.unwrap_or(spare);
        let cell = Rect::from_min_size(pos2(x, rect.top() + 2.0), vec2(w, height - 2.0));
        bevel::paint(ui.painter(), cell, Bevel::Shallow);
        ui.painter().with_clip_rect(cell.shrink(2.0)).text(
            cell.left_center() + vec2(4.0, 0.0),
            Align2::LEFT_CENTER,
            *text,
            theme::font(theme::FONT_SIZE),
            BLACK,
        );
        x += w + gap;
    }
}
```

- [ ] **Step 9: Implement resize edges for the undecorated window**

`crates/win95/src/window_frame.rs`:

```rust
use egui::{CursorIcon, Rect, ResizeDirection, Sense, Ui, ViewportCommand, pos2};

/// Invisible resize handles along the edges of `rect` for an undecorated window.
pub fn resize_edges(ui: &mut Ui, rect: Rect) {
    let t = 4.0;
    let (l, r, top, b) = (rect.left(), rect.right(), rect.top(), rect.bottom());
    let zones = [
        (
            Rect::from_min_max(pos2(l, top), pos2(l + t, top + t)),
            ResizeDirection::NorthWest,
            CursorIcon::ResizeNorthWest,
        ),
        (
            Rect::from_min_max(pos2(r - t, top), pos2(r, top + t)),
            ResizeDirection::NorthEast,
            CursorIcon::ResizeNorthEast,
        ),
        (
            Rect::from_min_max(pos2(l, b - t), pos2(l + t, b)),
            ResizeDirection::SouthWest,
            CursorIcon::ResizeSouthWest,
        ),
        (
            Rect::from_min_max(pos2(r - t, b - t), pos2(r, b)),
            ResizeDirection::SouthEast,
            CursorIcon::ResizeSouthEast,
        ),
        (
            Rect::from_min_max(pos2(l + t, top), pos2(r - t, top + t)),
            ResizeDirection::North,
            CursorIcon::ResizeNorth,
        ),
        (
            Rect::from_min_max(pos2(l + t, b - t), pos2(r - t, b)),
            ResizeDirection::South,
            CursorIcon::ResizeSouth,
        ),
        (
            Rect::from_min_max(pos2(l, top + t), pos2(l + t, b - t)),
            ResizeDirection::West,
            CursorIcon::ResizeWest,
        ),
        (
            Rect::from_min_max(pos2(r - t, top + t), pos2(r, b - t)),
            ResizeDirection::East,
            CursorIcon::ResizeEast,
        ),
    ];
    for (i, (zone, dir, cursor)) in zones.into_iter().enumerate() {
        let resp = ui
            .interact(zone, ui.id().with(("resize", i)), Sense::drag())
            .on_hover_cursor(cursor);
        if resp.drag_started() {
            ui.ctx()
                .send_viewport_cmd(ViewportCommand::BeginResize(dir));
        }
    }
}
```

- [ ] **Step 10: Run the tests to see them pass**

Run: `cargo test -p win95`

Expected: unit tests `8 passed`.

- [ ] **Step 11: Lint**

Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings`

Expected: clean.

- [ ] **Step 12: Commit**

```bash
git add crates/win95
git commit -m "feat(win95): title bar, modal dialog, icons, status bar and resize edges"
```


### Task 7: `win95` list view, text field, tabs, progress bar

**Files:**
- Create: `crates/win95/src/list_view.rs`, `crates/win95/src/text_field.rs`, `crates/win95/src/tabs.rs`, `crates/win95/src/progress.rs`
- Modify: `crates/win95/src/lib.rs`

**Interfaces:**
- Consumes: `bevel`, `bevel_frame`, `theme` (Task 5).
- Produces: `Column { title: &'static str, width: f32 }`; `Cell { text: String, dimmed: bool }` (+ `From<String>`, `From<&str>`); `ListView::new(id: impl egui::AsId, &[Column], row_count).header(bool).height(f32).context_menu(impl FnMut(usize, &mut Ui)).show(ui, selected: Option<usize>, cell: impl FnMut(row, col) -> Cell) -> ListResponse`; `ListResponse { clicked, double_clicked, secondary_clicked: Option<usize> }` (rows accessible by their first-column text); `win95::text_field(ui, &mut String, width: f32, password: bool) -> Response`; `win95::tabs(ui, &mut usize, &[&str]) -> bool`; `ProgressBar95::new(Option<f32>).width(f32)` (`None` = animated marquee) and `progress::block_count(fraction, inner_width) -> usize`.

- [ ] **Step 1: Write the failing tests in `crates/win95/src/list_view.rs`**

Create `crates/win95/src/list_view.rs` containing only this test module for now (the implementation goes above it in a later step).

`crates/win95/src/list_view.rs`:

```rust
#[cfg(test)]
mod tests {
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    use super::{Column, ListView};

    const COLS: &[Column] = &[
        Column {
            title: "Name",
            width: 120.0,
        },
        Column {
            title: "Owner",
            width: 120.0,
        },
    ];

    #[test]
    fn click_selects_row_and_only_visible_rows_are_built() {
        let mut h = Harness::new_ui_state(
            |ui, sel: &mut Option<usize>| {
                let r = ListView::new("repos", COLS, 10_000).height(200.0).show(
                    ui,
                    *sel,
                    |row, col| {
                        if col == 0 {
                            format!("repo{row}").into()
                        } else {
                            "ada".into()
                        }
                    },
                );
                if let Some(i) = r.clicked {
                    *sel = Some(i);
                }
            },
            None,
        );
        h.run();
        h.get_by_label("repo3").click();
        h.run();
        assert_eq!(*h.state(), Some(3));
        // Virtualisation: row 5000 is not laid out.
        assert!(h.query_by_label("repo5000").is_none());
    }

    #[test]
    fn context_menu_runs_for_the_right_clicked_row() {
        let mut h = Harness::new_ui_state(
            |ui, removed: &mut Option<usize>| {
                ListView::new("recents", COLS, 3)
                    .context_menu(|row, ui| {
                        if ui.button("Remove from list").clicked() {
                            *removed = Some(row);
                        }
                    })
                    .show(ui, None, |row, _| format!("repo{row}").into());
            },
            None,
        );
        h.run();
        h.get_by_label("repo1").click_secondary();
        h.run();
        h.get_by_label("Remove from list").click();
        h.run();
        assert_eq!(*h.state(), Some(1));
    }
}
```

- [ ] **Step 2: Write the failing tests in `crates/win95/src/text_field.rs`**

Create `crates/win95/src/text_field.rs` containing only this test module for now (the implementation goes above it in a later step).

`crates/win95/src/text_field.rs`:

```rust
#[cfg(test)]
mod tests {
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    #[test]
    fn typing_updates_the_string() {
        let mut h = Harness::new_ui_state(
            |ui, s: &mut String| {
                super::text_field(ui, s, 200.0, false);
            },
            String::new(),
        );
        h.get_by_role(egui::accesskit::Role::TextInput).focus();
        h.run();
        h.get_by_role(egui::accesskit::Role::TextInput)
            .type_text("octocat");
        h.run();
        assert_eq!(h.state(), "octocat");
    }
}
```

- [ ] **Step 3: Write the failing tests in `crates/win95/src/tabs.rs`**

Create `crates/win95/src/tabs.rs` containing only this test module for now (the implementation goes above it in a later step).

`crates/win95/src/tabs.rs`:

```rust
#[cfg(test)]
mod tests {
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    #[test]
    fn clicking_a_tab_selects_it() {
        let mut h = Harness::new_ui_state(
            |ui, sel: &mut usize| {
                super::tabs(ui, sel, &["Standard", "Advanced"]);
            },
            0usize,
        );
        h.get_by_label("Advanced").click();
        h.run();
        assert_eq!(*h.state(), 1);
    }
}
```

- [ ] **Step 4: Write the failing tests in `crates/win95/src/progress.rs`**

Create `crates/win95/src/progress.rs` containing only this test module for now (the implementation goes above it in a later step).

`crates/win95/src/progress.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::block_count;

    #[test]
    fn block_count_scales_and_clamps() {
        // 98 px inner width => (98 + 2) / 10 = 10 blocks.
        assert_eq!(block_count(0.0, 98.0), 0);
        assert_eq!(block_count(0.5, 98.0), 5);
        assert_eq!(block_count(1.0, 98.0), 10);
        assert_eq!(block_count(1.7, 98.0), 10);
        assert_eq!(block_count(-1.0, 98.0), 0);
        assert_eq!(block_count(1.0, 0.0), 0);
    }
}
```

- [ ] **Step 5: Create the final crate root**

`crates/win95/src/lib.rs`:

```rust
//! Windows 95 look-and-feel for egui. No business logic lives here.

pub mod bevel;
pub mod button;
pub mod dialog;
pub mod icon;
pub mod list_view;
pub mod panel;
pub mod progress;
pub mod status_bar;
pub mod tabs;
pub mod text_field;
pub mod theme;
pub mod title_bar;
pub mod window_frame;

pub use bevel::Bevel;
pub use button::Button95;
pub use dialog::{Dialog, DialogResponse};
pub use icon::Icon;
pub use list_view::{Cell, Column, ListResponse, ListView};
pub use panel::bevel_frame;
pub use progress::ProgressBar95;
pub use status_bar::status_bar;
pub use tabs::tabs;
pub use text_field::text_field;
pub use title_bar::{TitleAction, TitleBar};
pub use window_frame::resize_edges;
```

- [ ] **Step 6: Run the tests to see them fail**

Run: `cargo test -p win95`

Expected: compile errors for the four new modules.

- [ ] **Step 7: Implement the virtualised list view**

Insert this **above** the existing `#[cfg(test)]` module in `crates/win95/src/list_view.rs`.

`crates/win95/src/list_view.rs`:

```rust
use egui::{Align2, Id, Rect, ScrollArea, Sense, Ui, WidgetInfo, WidgetType, pos2, vec2};

use crate::bevel::{self, Bevel};
use crate::theme::{self, BLACK, GRAY, NAVY, SILVER, WHITE};

pub struct Column {
    pub title: &'static str,
    pub width: f32,
}

pub struct Cell {
    pub text: String,
    /// Greyed out (e.g. a missing folder).
    pub dimmed: bool,
}

impl From<String> for Cell {
    fn from(text: String) -> Cell {
        Cell {
            text,
            dimmed: false,
        }
    }
}

impl From<&str> for Cell {
    fn from(text: &str) -> Cell {
        Cell {
            text: text.to_string(),
            dimmed: false,
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ListResponse {
    pub clicked: Option<usize>,
    pub double_clicked: Option<usize>,
    pub secondary_clicked: Option<usize>,
}

/// Virtualised multi-column list in a white sunken well. Only visible rows are laid out,
/// so thousands of rows cost the same as twenty.
pub struct ListView<'a> {
    id: Id,
    columns: &'a [Column],
    row_count: usize,
    show_header: bool,
    height: f32,
    context_menu: Option<ContextMenuFn<'a>>,
}

type ContextMenuFn<'a> = Box<dyn FnMut(usize, &mut Ui) + 'a>;

pub const ROW_HEIGHT: f32 = 18.0;

impl<'a> ListView<'a> {
    pub fn new(id: impl egui::AsId, columns: &'a [Column], row_count: usize) -> ListView<'a> {
        ListView {
            id: Id::new(id),
            columns,
            row_count,
            show_header: true,
            height: 200.0,
            context_menu: None,
        }
    }

    pub fn header(mut self, show: bool) -> Self {
        self.show_header = show;
        self
    }

    pub fn height(mut self, height: f32) -> Self {
        self.height = height;
        self
    }

    /// Right-click menu for a row.
    pub fn context_menu(mut self, menu: impl FnMut(usize, &mut Ui) + 'a) -> Self {
        self.context_menu = Some(Box::new(menu));
        self
    }

    pub fn show(
        mut self,
        ui: &mut Ui,
        selected: Option<usize>,
        mut cell: impl FnMut(usize, usize) -> Cell,
    ) -> ListResponse {
        let mut out = ListResponse::default();
        let width = ui.available_width();
        let font = theme::font(theme::FONT_SIZE);
        let (outer, _) = ui.allocate_exact_size(vec2(width, self.height), Sense::hover());
        ui.painter().rect_filled(outer, 0.0, WHITE);
        bevel::paint(ui.painter(), outer, Bevel::Field);
        let inner = outer.shrink(2.0);
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(inner).id_salt(self.id));
        child.set_clip_rect(inner);
        let ui = &mut child;
        ui.spacing_mut().item_spacing = vec2(0.0, 0.0);

        if self.show_header {
            let (hdr, _) =
                ui.allocate_exact_size(vec2(inner.width(), ROW_HEIGHT + 2.0), Sense::hover());
            let mut x = hdr.left();
            for col in self.columns {
                let r = Rect::from_min_size(pos2(x, hdr.top()), vec2(col.width, hdr.height()));
                ui.painter().rect_filled(r, 0.0, SILVER);
                bevel::paint(ui.painter(), r, Bevel::Raised);
                ui.painter().with_clip_rect(r.shrink(2.0)).text(
                    r.left_center() + vec2(4.0, 0.0),
                    Align2::LEFT_CENTER,
                    col.title,
                    font.clone(),
                    BLACK,
                );
                x += col.width;
            }
        }

        ScrollArea::vertical()
            .id_salt(self.id.with("scroll"))
            .auto_shrink([false, false])
            .show_rows(ui, ROW_HEIGHT, self.row_count, |ui, range| {
                for row in range {
                    let (rect, resp) = ui.allocate_exact_size(
                        vec2(ui.available_width(), ROW_HEIGHT),
                        Sense::click(),
                    );
                    let is_sel = selected == Some(row);
                    let cells: Vec<Cell> = (0..self.columns.len()).map(|c| cell(row, c)).collect();
                    let label = cells.first().map(|c| c.text.clone()).unwrap_or_default();
                    resp.widget_info(|| {
                        WidgetInfo::selected(WidgetType::SelectableLabel, true, is_sel, &label)
                    });
                    if is_sel {
                        ui.painter().rect_filled(rect, 0.0, NAVY);
                    }
                    let mut x = rect.left();
                    for (col, c) in self.columns.iter().zip(&cells) {
                        let r =
                            Rect::from_min_size(pos2(x, rect.top()), vec2(col.width, ROW_HEIGHT));
                        let color = match (is_sel, c.dimmed) {
                            (true, _) => WHITE,
                            (false, true) => GRAY,
                            (false, false) => BLACK,
                        };
                        ui.painter().with_clip_rect(r.shrink(1.0)).text(
                            r.left_center() + vec2(4.0, 0.0),
                            Align2::LEFT_CENTER,
                            &c.text,
                            font.clone(),
                            color,
                        );
                        x += col.width;
                    }
                    if resp.clicked() {
                        out.clicked = Some(row);
                    }
                    if resp.double_clicked() {
                        out.double_clicked = Some(row);
                    }
                    if resp.secondary_clicked() {
                        out.secondary_clicked = Some(row);
                    }
                    if let Some(menu) = self.context_menu.as_mut() {
                        resp.context_menu(|ui| menu(row, ui));
                    }
                }
            });
        out
    }
}
```

`ScrollArea::show_rows` only lays out visible rows, so 10 000 repositories cost the same as 20 (the test asserts row 5000 is not built).

- [ ] **Step 8: Implement `crates/win95/src/text_field.rs`**

Insert this **above** the existing `#[cfg(test)]` module in `crates/win95/src/text_field.rs`.

`crates/win95/src/text_field.rs`:

```rust
use egui::{Response, TextEdit, Ui};

use crate::bevel::Bevel;
use crate::panel::bevel_frame;
use crate::theme::WHITE;

/// Single-line white sunken text box.
pub fn text_field(ui: &mut Ui, text: &mut String, width: f32, password: bool) -> Response {
    bevel_frame(ui, Bevel::Field, WHITE, 1, |ui| {
        ui.add(
            TextEdit::singleline(text)
                .frame(egui::Frame::NONE)
                .password(password)
                .desired_width(width),
        )
    })
    .inner
}
```

- [ ] **Step 9: Implement `crates/win95/src/tabs.rs`**

Insert this **above** the existing `#[cfg(test)]` module in `crates/win95/src/tabs.rs`.

`crates/win95/src/tabs.rs`:

```rust
use egui::{Align2, Rect, Sense, Ui, WidgetInfo, WidgetType, pos2, vec2};

use crate::theme::{self, BLACK, GRAY, LIGHT, SILVER, WHITE};

/// Row of Win95 tabs. Returns `true` if the selection changed.
/// Draw the tab page right below with `bevel_frame(ui, Bevel::Window, ...)`.
pub fn tabs(ui: &mut Ui, selected: &mut usize, labels: &[&str]) -> bool {
    let mut changed = false;
    let font = theme::font(theme::FONT_SIZE);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        for (i, label) in labels.iter().enumerate() {
            let galley = ui
                .painter()
                .layout_no_wrap(label.to_string(), font.clone(), BLACK);
            let size = vec2(galley.size().x + 16.0, 20.0);
            let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
            let is_sel = *selected == i;
            let owned = label.to_string();
            resp.widget_info(|| {
                WidgetInfo::selected(WidgetType::SelectableLabel, true, is_sel, &owned)
            });
            if resp.clicked() && !is_sel {
                *selected = i;
                changed = true;
            }
            // Selected tab is 2px taller and merges with the page below.
            let r = if is_sel {
                rect
            } else {
                Rect::from_min_max(pos2(rect.left(), rect.top() + 2.0), rect.max)
            };
            let p = ui.painter();
            p.rect_filled(r, 0.0, SILVER);
            p.rect_filled(Rect::from_min_size(r.min, vec2(r.width(), 1.0)), 0.0, WHITE);
            p.rect_filled(
                Rect::from_min_size(r.min, vec2(1.0, r.height())),
                0.0,
                WHITE,
            );
            p.rect_filled(
                Rect::from_min_size(pos2(r.max.x - 1.0, r.min.y), vec2(1.0, r.height())),
                0.0,
                BLACK,
            );
            p.rect_filled(
                Rect::from_min_size(
                    pos2(r.max.x - 2.0, r.min.y + 1.0),
                    vec2(1.0, r.height() - 1.0),
                ),
                0.0,
                GRAY,
            );
            if !is_sel {
                p.rect_filled(
                    Rect::from_min_size(pos2(r.min.x, r.max.y - 1.0), vec2(r.width(), 1.0)),
                    0.0,
                    LIGHT,
                );
            }
            p.text(
                r.center(),
                Align2::CENTER_CENTER,
                *label,
                font.clone(),
                BLACK,
            );
        }
    });
    changed
}
```

- [ ] **Step 10: Implement the progress bar**

Insert this **above** the existing `#[cfg(test)]` module in `crates/win95/src/progress.rs`.

`crates/win95/src/progress.rs`:

```rust
use egui::{Rect, Response, Sense, Ui, Widget, pos2, vec2};

use crate::bevel::{self, Bevel};
use crate::theme::{NAVY, SILVER};

const BLOCK: f32 = 8.0;
const GAP: f32 = 2.0;

/// Segmented blue progress bar. `fraction = None` shows an animated marquee.
pub struct ProgressBar95 {
    fraction: Option<f32>,
    width: f32,
}

impl ProgressBar95 {
    pub fn new(fraction: Option<f32>) -> ProgressBar95 {
        ProgressBar95 {
            fraction,
            width: 260.0,
        }
    }

    pub fn width(mut self, width: f32) -> ProgressBar95 {
        self.width = width;
        self
    }
}

/// Number of blocks drawn for `fraction` in a bar whose inner width is `inner_width`.
pub fn block_count(fraction: f32, inner_width: f32) -> usize {
    let total = ((inner_width + GAP) / (BLOCK + GAP)).floor().max(0.0) as usize;
    ((fraction.clamp(0.0, 1.0) * total as f32).round() as usize).min(total)
}

impl Widget for ProgressBar95 {
    fn ui(self, ui: &mut Ui) -> Response {
        let (rect, resp) = ui.allocate_exact_size(vec2(self.width, 20.0), Sense::hover());
        let p = ui.painter();
        p.rect_filled(rect, 0.0, SILVER);
        bevel::paint(p, rect, Bevel::Shallow);
        let inner = rect.shrink(3.0);
        let total = block_count(1.0, inner.width());
        let block_at = |i: usize| {
            let x = inner.left() + i as f32 * (BLOCK + GAP);
            Rect::from_min_max(
                pos2(x, inner.top()),
                pos2((x + BLOCK).min(inner.right()), inner.bottom()),
            )
        };
        match self.fraction {
            Some(f) => {
                for i in 0..block_count(f, inner.width()) {
                    p.rect_filled(block_at(i), 0.0, NAVY);
                }
            }
            None => {
                // Marquee: 5 blocks sliding, repaint only while visible.
                let step = (ui.input(|i| i.time) * 10.0) as usize;
                let span = total + 5;
                for k in 0..5 {
                    let i = (step + k) % span.max(1);
                    if i >= 5 && i - 5 < total {
                        p.rect_filled(block_at(i - 5), 0.0, NAVY);
                    }
                }
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_millis(100));
            }
        }
        resp
    }
}
```

The marquee requests a repaint every 100 ms only while it is on screen; nothing else animates, so the idle app stays at ~0 % CPU.

- [ ] **Step 11: Run the tests to see them pass**

Run: `cargo test -p win95`

Expected: unit tests `13 passed`, `tests/theme.rs` `1 passed`.

- [ ] **Step 12: Lint**

Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings`

Expected: clean.

- [ ] **Step 13: Commit**

```bash
git add crates/win95
git commit -m "feat(win95): virtualised list view, text field, tabs and progress bar"
```


### Task 8: `retrogit` app crate: strings, formatting, config, logging, protocol

**Files:**
- Create: `crates/app/Cargo.toml`, `crates/app/src/lib.rs`, `crates/app/src/main.rs` (placeholder), `crates/app/src/strings.rs`, `crates/app/src/format.rs`, `crates/app/src/config.rs`, `crates/app/src/logging.rs`, `crates/app/src/protocol.rs`

**Interfaces:**
- Consumes: `gitcore::{CloneProgress, GitError, RepoSummary}`, `github::{DeviceFlowFailure, GithubError, RepoInfo, TokenStoreError, User}`.
- Produces: `retrogit::GITHUB_CLIENT_ID: &str`; `strings::*` constants; `format::{format_epoch(i64) -> String, format_bytes(usize) -> String}`; `Config { recent: Vec<RecentRepo>, last_clone_dir: Option<PathBuf>, window: Option<WindowGeometry> }` with `default_path()`, `load_from(&Path) -> Config`, `save_to(&self, &Path) -> io::Result<()>`, `add_recent(&mut self, &str, &Path)`, `remove_recent(&mut self, &Path)`, `MAX_RECENT = 20`; `RecentRepo { name: String, path: PathBuf }`; `WindowGeometry { width, height: f32, x, y: Option<f32> }`; `logging::{init(&Path), add_secret(&str), redact(&str, &[String]) -> String, open_log_file(&Path, u64), MAX_LOG_BYTES}`; `protocol::{Command, Event, Op, Severity, AppError}`:
  - `Command::{ValidateToken, StartDeviceFlow, SavePat(String), SignOut, ListRepos, Clone { url: String, dest: PathBuf }, OpenRepo(PathBuf)}`
  - `Event::{SignedIn(User), SignedOut, DeviceCode { user_code, verification_uri: String }, DeviceFlowCancelled, ReposLoaded(Vec<RepoInfo>), CloneProgress(CloneProgress), CloneDone(RepoSummary), CloneCancelled, RepoOpened(RepoSummary), Error { during: Op, error: AppError }}`
  - `Op::{Auth, Repos, Clone, Open(PathBuf), Internal}`; `Severity::{Error, Warning, Info}`
  - `AppError { severity, message: String, detail: Option<String>, link: Option<String> }` with `new`, `from_github`, `from_git`, `from_device_flow`, `from_store`.

- [ ] **Step 1: Create the manifest**

The package is named `retrogit` (binary and library) and lives in `crates/app`. All dependencies for S1 are declared now.

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

[dev-dependencies]
mockito.workspace = true
tempfile.workspace = true
git2.workspace = true

[lints]
workspace = true
```

- [ ] **Step 2: Write the failing tests in `crates/app/src/format.rs`**

Create `crates/app/src/format.rs` containing only this test module for now (the implementation goes above it in a later step).

`crates/app/src/format.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_formatting() {
        assert_eq!(format_epoch(0), "1970-01-01 00:00");
        assert_eq!(format_epoch(1_700_000_000), "2023-11-14 22:13");
        assert_eq!(format_epoch(951_782_400), "2000-02-29 00:00");
        assert_eq!(format_epoch(-86_400), "1969-12-31 00:00");
    }

    #[test]
    fn byte_formatting() {
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1536), "1.5 KB");
        assert_eq!(format_bytes(5 * 1024 * 1024), "5.0 MB");
    }
}
```

- [ ] **Step 3: Write the failing tests in `crates/app/src/config.rs`**

Create `crates/app/src/config.rs` containing only this test module for now (the implementation goes above it in a later step).

`crates/app/src/config.rs`:

```rust
#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn roundtrip() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("sub").join("config.json");
        let mut c = Config::default();
        c.add_recent("demo", Path::new("/tmp/demo"));
        c.last_clone_dir = Some("/tmp".into());
        c.window = Some(WindowGeometry {
            width: 800.0,
            height: 600.0,
            x: Some(10.0),
            y: None,
        });
        c.save_to(&p).unwrap();
        assert_eq!(Config::load_from(&p), c);
    }

    #[test]
    fn missing_or_corrupt_file_gives_default() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("config.json");
        assert_eq!(Config::load_from(&p), Config::default());
        std::fs::write(&p, "{ not json").unwrap();
        assert_eq!(Config::load_from(&p), Config::default());
    }

    #[test]
    fn unknown_and_missing_fields_are_tolerated() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("config.json");
        std::fs::write(&p, r#"{"last_clone_dir":"/x","future_field":1}"#).unwrap();
        let c = Config::load_from(&p);
        assert_eq!(c.last_clone_dir, Some(PathBuf::from("/x")));
        assert!(c.recent.is_empty());
    }

    #[test]
    fn add_recent_dedupes_moves_to_front_and_caps() {
        let mut c = Config::default();
        for i in 0..25 {
            c.add_recent(&format!("r{i}"), Path::new(&format!("/r{i}")));
        }
        assert_eq!(c.recent.len(), MAX_RECENT);
        assert_eq!(c.recent[0].name, "r24");
        c.add_recent("r10", Path::new("/r10"));
        assert_eq!(c.recent[0].path, PathBuf::from("/r10"));
        assert_eq!(
            c.recent
                .iter()
                .filter(|r| r.path == Path::new("/r10"))
                .count(),
            1
        );
        c.remove_recent(Path::new("/r10"));
        assert!(c.recent.iter().all(|r| r.path != Path::new("/r10")));
    }
}
```

- [ ] **Step 4: Write the failing tests in `crates/app/src/logging.rs`**

Create `crates/app/src/logging.rs` containing only this test module for now (the implementation goes above it in a later step).

`crates/app/src/logging.rs`:

```rust
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

- [ ] **Step 5: Create the crate root (Task 8 version)**

`crates/app/src/lib.rs`:

```rust
//! RetroGit application: state, background worker and screens.

pub mod config;
pub mod format;
pub mod logging;
pub mod protocol;
pub mod strings;

/// OAuth App client ID (public, no secret). Paste yours here, or build with
/// `RETROGIT_GITHUB_CLIENT_ID=Ov23li... cargo build`.
pub const GITHUB_CLIENT_ID: &str = match option_env!("RETROGIT_GITHUB_CLIENT_ID") {
    Some(id) => id,
    None => "",
};
```

- [ ] **Step 6: Create a placeholder binary**

Replaced in Task 11.

`crates/app/src/main.rs`:

```rust
fn main() {}
```

- [ ] **Step 7: Run the tests to see them fail**

Run: `cargo test -p retrogit --lib`

Expected: compile errors: missing modules `strings`, `protocol`, and missing functions in `format`, `config`, `logging`.

- [ ] **Step 8: Add every user-visible string**

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

pub const ABOUT_TITLE: &str = "About RetroGit";
pub const ABOUT_TAGLINE: &str = "A Git client with a Windows 95 look.";
pub const ABOUT_FONT: &str = "Font: W95FA by Alina Sava (SIL Open Font License 1.1)";

pub const ERR_TITLE: &str = "RetroGit";
pub const ERR_NO_NETWORK: &str = "Could not reach GitHub. Check your network connection.";
pub const ERR_UNAUTHORIZED: &str = "GitHub rejected your session. Please sign in again.";
pub const ERR_PAT_REJECTED: &str = "GitHub rejected this token.";
pub const ERR_SSO: &str = "Your organization requires SSO authorization for this token. Open the link below, authorize, then try again.";
pub const ERR_RATE_LIMIT: &str = "GitHub API rate limit reached. Try again in a few minutes.";
pub const ERR_DEVICE_EXPIRED: &str = "The sign-in code expired. Click Sign in to get a new one.";
pub const ERR_DEVICE_DENIED: &str = "Authorization was denied on github.com.";
pub const ERR_NO_CLIENT_ID: &str = "This build has no GitHub OAuth App client ID. Use the Advanced tab to paste a personal access token.";
pub const ERR_DEST_NOT_EMPTY: &str = "The destination folder already exists and is not empty.";
pub const ERR_NOT_A_REPO: &str = "This folder is not a Git repository.";
pub const ERR_GIT_AUTH: &str = "GitHub refused access to this repository. Check that your token is authorized for the organization (SSO).";
pub const ERR_KEYCHAIN: &str = "Could not access the system credential store.";
pub const ERR_INTERNAL: &str = "An internal error occurred. Details were written to the log file.";
pub const INFO_CLONE_CANCELLED: &str = "Clone cancelled.";
```

- [ ] **Step 9: Implement `crates/app/src/format.rs`**

Insert this **above** the existing `#[cfg(test)]` module in `crates/app/src/format.rs`.

`crates/app/src/format.rs`:

```rust
//! Small, dependency-free formatting helpers.

/// `YYYY-MM-DD HH:MM` in UTC for a Unix timestamp.
pub fn format_epoch(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}",
        rem / 3600,
        (rem % 3600) / 60
    )
}

/// Howard Hinnant's days-to-civil algorithm.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Human-readable byte count: `512 B`, `1.5 KB`, `12.3 MB`.
pub fn format_bytes(bytes: usize) -> String {
    const KB: f64 = 1024.0;
    let b = bytes as f64;
    if b < KB {
        format!("{bytes} B")
    } else if b < KB * KB {
        format!("{:.1} KB", b / KB)
    } else if b < KB * KB * KB {
        format!("{:.1} MB", b / (KB * KB))
    } else {
        format!("{:.2} GB", b / (KB * KB * KB))
    }
}
```

- [ ] **Step 10: Implement the config**

Insert this **above** the existing `#[cfg(test)]` module in `crates/app/src/config.rs`.

`crates/app/src/config.rs`:

```rust
//! Persisted settings (JSON in the OS config dir). Never contains the token.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const MAX_RECENT: usize = 20;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecentRepo {
    pub name: String,
    pub path: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WindowGeometry {
    pub width: f32,
    pub height: f32,
    pub x: Option<f32>,
    pub y: Option<f32>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub recent: Vec<RecentRepo>,
    pub last_clone_dir: Option<PathBuf>,
    pub window: Option<WindowGeometry>,
}

impl Config {
    /// `<config dir>/RetroGit/config.json`
    pub fn default_path() -> Option<PathBuf> {
        dirs::config_dir().map(|d| d.join("RetroGit").join("config.json"))
    }

    /// Missing or corrupt file => default config (a corrupt file is logged, not fatal).
    pub fn load_from(path: &Path) -> Config {
        match std::fs::read_to_string(path) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_else(|e| {
                log::warn!("ignoring corrupt config {}: {e}", path.display());
                Config::default()
            }),
            Err(_) => Config::default(),
        }
    }

    /// Atomic write: temp file then rename.
    pub fn save_to(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        let text = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, path)
    }

    /// Put `path` first in the recent list (no duplicates, capped).
    pub fn add_recent(&mut self, name: &str, path: &Path) {
        self.recent.retain(|r| r.path != path);
        self.recent.insert(
            0,
            RecentRepo {
                name: name.to_string(),
                path: path.to_path_buf(),
            },
        );
        self.recent.truncate(MAX_RECENT);
    }

    pub fn remove_recent(&mut self, path: &Path) {
        self.recent.retain(|r| r.path != path);
    }
}
```

Corrupt JSON falls back to defaults (logged), writes go through a temp file + rename so a crash never leaves a half-written config.

- [ ] **Step 11: Implement the redacting file logger**

Insert this **above** the existing `#[cfg(test)]` module in `crates/app/src/logging.rs`.

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
```

- [ ] **Step 12: Implement the UI ↔ worker protocol**

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

- [ ] **Step 13: Run the tests to see them pass**

Run: `cargo test -p retrogit --lib`

Expected: `8 passed` (2 format + 4 config + 2 logging).

- [ ] **Step 14: Lint**

Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings`

Expected: clean.

- [ ] **Step 15: Commit**

```bash
git add Cargo.lock crates/app
git commit -m "feat(app): strings, formatting, config, redacting logger and protocol"
```


### Task 9: `AppState` and its pure `apply` function

**Files:**
- Create: `crates/app/src/state.rs`
- Modify: `crates/app/src/lib.rs` (add `pub mod state;`)
- Test: `crates/app/tests/state.rs`

**Interfaces:**
- Consumes: `Config` (Task 8), `protocol::*` (Task 8), `gitcore::{CloneProgress, RepoSummary}`, `github::{RepoInfo, User}`.
- Produces: `Auth::{Checking, SignedOut, Starting, Waiting { user_code, verification_uri: String }, SignedIn(User)}`; `SignInDialog { tab: usize, pat: String, pat_submitted: bool }`; `CloneDialog { filter: String, selected: Option<String> /*full_name*/, dest_parent: String, progress: Option<CloneProgress>, cloning_name: String }`; `AppState { config, config_dirty, auth, repos, repos_loading, sign_in: Option<SignInDialog>, clone: Option<CloneDialog>, about: bool, current: Option<RepoSummary>, missing: HashSet<PathBuf>, messages: VecDeque<AppError> }` with `new(Config)`, `user(&self) -> Option<&User>`, `apply(&mut self, Event)`, `remove_recent(&mut self, &Path)`; `state::filter_repos(&[RepoInfo], &str) -> Vec<usize>`.

- [ ] **Step 1: Write the failing tests**

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
```

- [ ] **Step 2: Declare the module**

In `crates/app/src/lib.rs`, add `pub mod state;` after `pub mod protocol;`.

- [ ] **Step 3: Run the tests to see them fail**

Run: `cargo test -p retrogit --test state`

Expected: compile error `file not found for module state`.

- [ ] **Step 4: Implement the state**

`apply` is the only place events change state. It never does I/O except `Path::exists` for recent entries at startup / after an open error.

`crates/app/src/state.rs`:

```rust
//! All UI state, updated by the pure `apply` function.

use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};

use gitcore::{CloneProgress, RepoSummary};
use github::{RepoInfo, User};

use crate::config::Config;
use crate::protocol::{AppError, Event, Op};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Auth {
    /// Validating the stored token at startup.
    Checking,
    SignedOut,
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
                self.remember(&summary);
                self.current = Some(summary);
            }
            Event::CloneCancelled => {
                if let Some(c) = self.clone.as_mut() {
                    c.progress = None;
                }
            }
            Event::RepoOpened(summary) => {
                self.remember(&summary);
                self.current = Some(summary);
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
            Op::Internal => {
                self.repos_loading = false;
                if let Some(c) = self.clone.as_mut() {
                    c.progress = None;
                }
            }
        }
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

- [ ] **Step 5: Run the tests to see them pass**

Run: `cargo test -p retrogit --test state`

Expected: `9 passed`.

- [ ] **Step 6: Lint**

Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings`

Expected: clean.

- [ ] **Step 7: Commit**

```bash
git add crates/app
git commit -m "feat(app): application state with pure event reducer"
```


### Task 10: Background worker

**Files:**
- Create: `crates/app/src/worker.rs`
- Modify: `crates/app/src/lib.rs` (add `pub mod worker;`)
- Test: `crates/app/tests/worker.rs`

**Interfaces:**
- Consumes: `Command`, `Event`, `Op`, `AppError`, `Severity` (Task 8); `logging::add_secret`, `logging::redact` (Task 8); `github::{Client, DeviceFlow, Step, TokenStore, GithubError, SCOPES}` (Tasks 3–4); `gitcore::{clone, CloneRequest, Credentials, Repo, GitError}` (Tasks 1–2).
- Produces: `WorkerDeps { client: github::Client, store: Arc<dyn TokenStore>, client_id: String }`; `worker::spawn(WorkerDeps, notify: impl Fn() + Send + 'static) -> WorkerHandle`; `WorkerHandle { pub events: Receiver<Event>, .. }` with `send(&self, Command)`, `cancel_device_flow(&self)`, `cancel_clone(&self)`, `shutdown(&self, Duration) -> bool`; `Throttle::new(Duration)` / `ready(&mut self, Instant) -> bool`.

- [ ] **Step 1: Write the failing integration tests**

These drive the real worker thread against a mockito server and a `MemoryStore`. The Device Flow mock uses `interval: 1`, so two tests take about one second each.

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
```

- [ ] **Step 2: Write the failing tests in `crates/app/src/worker.rs`**

Create `crates/app/src/worker.rs` containing only this test module for now (the implementation goes above it in a later step).

`crates/app/src/worker.rs`:

```rust
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

Then add `pub mod worker;` to `crates/app/src/lib.rs` after `pub mod strings;`.

- [ ] **Step 3: Run the tests to see them fail**

Run: `cargo test -p retrogit`

Expected: compile errors: `WorkerDeps`, `spawn`, `Throttle`, `sleep_unless_cancelled` not found.

- [ ] **Step 4: Implement the worker**

Insert this **above** the existing `#[cfg(test)]` module in `crates/app/src/worker.rs`.

`crates/app/src/worker.rs`:

```rust
//! The single background thread doing all network and Git work.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use gitcore::{CloneRequest, Credentials, GitError, Repo};
use github::{Client, DeviceFlow, GithubError, Step, TokenStore};

use crate::logging;
use crate::protocol::{AppError, Command, Event, Op, Severity};
use crate::strings as s;

pub struct WorkerDeps {
    pub client: Client,
    pub store: Arc<dyn TokenStore>,
    /// Empty = Device Flow unavailable (PAT only).
    pub client_id: String,
}

/// UI-side handle. Cancellation flags bypass the command queue so they act immediately.
pub struct WorkerHandle {
    tx: Sender<Command>,
    pub events: Receiver<Event>,
    cancel_flow: Arc<AtomicBool>,
    cancel_clone: Arc<AtomicBool>,
    busy: Arc<AtomicBool>,
}

impl WorkerHandle {
    pub fn send(&self, cmd: Command) {
        match cmd {
            Command::StartDeviceFlow => self.cancel_flow.store(false, Ordering::SeqCst),
            Command::Clone { .. } => self.cancel_clone.store(false, Ordering::SeqCst),
            _ => {}
        }
        if self.tx.send(cmd).is_err() {
            log::error!("worker thread is gone");
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
    let mut worker = Worker {
        deps,
        token: None,
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
    }
}

struct Worker {
    deps: WorkerDeps,
    token: Option<String>,
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
                Ok(summary) => self.emit(Event::RepoOpened(summary)),
                Err(e) => self.fail(Op::Open(path), AppError::from_git(&e)),
            },
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
            Err(e) => self.fail(Op::Auth, AppError::from_github(&e)),
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
            Ok(repos) => self.emit(Event::ReposLoaded(repos)),
            Err(GithubError::Unauthorized) => {
                self.token = None;
                let _ = self.deps.store.clear();
                self.fail(Op::Repos, AppError::from_github(&GithubError::Unauthorized));
                self.emit(Event::SignedOut);
            }
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
            Ok(summary) => self.emit(Event::CloneDone(summary)),
            Err(GitError::Cancelled) => self.emit(Event::CloneCancelled),
            Err(e) => self.fail(Op::Clone, AppError::from_git(&e)),
        }
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
```

Notes:
- Cancel flags are atomics shared with the handle, so Cancel works even while the worker is blocked inside a clone or a poll sleep. `send` resets the matching flag *on the UI thread* before queuing, so a Cancel pressed right after Start is never lost.
- Each command runs inside `catch_unwind`; a panic becomes an `Error { during: Op::Internal }` event instead of killing the thread.
- `busy` lets `shutdown` wait for the partial-clone cleanup when the app quits.
- Clone progress is throttled to one event per 50 ms (≤ 20/s), and the final progress is always sent.

- [ ] **Step 5: Run the tests to see them pass**

Run: `cargo test -p retrogit`

Expected: unit `10 passed` (8 from Task 8 + 2 worker), `tests/state.rs` `9 passed`, `tests/worker.rs` `13 passed`.

- [ ] **Step 6: Lint**

Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings`

Expected: clean.

- [ ] **Step 7: Commit**

```bash
git add crates/app
git commit -m "feat(app): background worker for sign-in, repo listing, clone and open"
```


### Task 11: Screens, eframe app and `main`

**Files:**
- Create: `crates/app/src/ui/mod.rs`, `crates/app/src/ui/main_window.rs`, `crates/app/src/ui/sign_in.rs`, `crates/app/src/ui/clone_dialog.rs`, `crates/app/src/ui/message.rs`, `crates/app/src/ui/about.rs`, `crates/app/src/app.rs`
- Modify: `crates/app/src/lib.rs` (final), `crates/app/src/main.rs` (real entry point)

**Interfaces:**
- Consumes: everything above.
- Produces: `ui::Ctx<'a> { state: &'a mut AppState, worker: &'a WorkerHandle }`; `ui::{main_window, sign_in, clone_dialog, message, about}::show(..)`; `clone_dialog::open(&mut Ctx)`; `RetroGitApp::new(AppState, WorkerHandle, Option<PathBuf>)` implementing `eframe::App` (`logic` drains events + saves config, `ui` draws, `on_exit` shuts the worker down and saves). No new automated tests here: the logic is already covered by Tasks 5–10; this task is verified by the manual smoke test below.

- [ ] **Step 1: Screen context**

`crates/app/src/ui/mod.rs`:

```rust
//! Screens. Each function draws from `AppState` and sends `Command`s to the worker.

pub mod about;
pub mod clone_dialog;
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

- [ ] **Step 2: Main window**

Layout: custom title bar → menu bar → toolbar (top panel), status bar (bottom panel), resizable recent-repos list (left panel), repository summary (central). Fetch/Pull/Push are visible but disabled until S3.

`crates/app/src/ui/main_window.rs`:

```rust
use egui::{Panel, RichText, UiBuilder, ViewportCommand};
use win95::{
    Bevel, Button95, Cell, Column, ListView, TitleAction, TitleBar, bevel_frame, status_bar,
};

use super::{Ctx, clone_dialog};
use crate::format::format_epoch;
use crate::protocol::Command;
use crate::state::Auth;
use crate::strings as s;
use gitcore::Head;

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
            .show(ui, |ui| summary(ui, cx));
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
    match TitleBar::new(&text).active(focused).show(ui) {
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
                cx.state.sign_in.get_or_insert_with(Default::default);
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

fn summary(ui: &mut egui::Ui, cx: &mut Ctx<'_>) {
    bevel_frame(ui, Bevel::Field, win95::theme::WHITE, 8, |ui| {
        ui.set_min_size(ui.available_size());
        let Some(c) = &cx.state.current else {
            ui.label(s::NO_REPO);
            return;
        };
        ui.label(RichText::new(&c.name).heading());
        ui.add_space(6.0);
        egui::Grid::new("summary")
            .num_columns(2)
            .spacing([12.0, 4.0])
            .show(ui, |ui| {
                ui.label(s::BRANCH);
                ui.label(match &c.head {
                    Head::Branch(b) => b.clone(),
                    Head::Unborn(b) => format!("{b} {}", s::NO_COMMITS),
                    Head::Detached(id) => format!("{id} {}", s::DETACHED),
                });
                ui.end_row();
                ui.label(s::REMOTE);
                ui.label(c.origin_url.as_deref().unwrap_or(s::NO_REMOTE));
                ui.end_row();
                ui.label(s::LAST_COMMIT);
                ui.label(match &c.last_commit {
                    Some(lc) => format!(
                        "{} \"{}\" - {}, {}",
                        lc.short_id,
                        lc.summary,
                        lc.author,
                        format_epoch(lc.time)
                    ),
                    None => s::NO_COMMITS.to_string(),
                });
                ui.end_row();
            });
        ui.add_space(8.0);
        ui.label(RichText::new(c.path.display().to_string()).color(win95::theme::GRAY));
    });
}

fn status(ui: &mut egui::Ui, cx: &mut Ctx<'_>) {
    let who = match &cx.state.auth {
        Auth::SignedIn(u) => format!("Signed in: @{}", u.login),
        Auth::Checking => s::CHECKING.to_string(),
        _ => s::NOT_SIGNED_IN.to_string(),
    };
    let activity = if cx.state.repos_loading {
        s::LOADING
    } else {
        s::READY
    };
    status_bar(ui, &[(&who, Some(260.0)), (activity, None)]);
}
```

- [ ] **Step 3: Sign-in dialog**

`crates/app/src/ui/sign_in.rs`:

```rust
use egui::RichText;
use win95::{Bevel, Button95, Dialog, ProgressBar95, bevel_frame, tabs, text_field};

use super::Ctx;
use crate::protocol::Command;
use crate::state::Auth;
use crate::strings as s;

pub fn show(egui_ctx: &egui::Context, cx: &mut Ctx<'_>) {
    if cx.state.sign_in.is_none() {
        return;
    }
    let mut close = false;
    let r = Dialog::new("sign_in", s::SIGN_IN_TITLE)
        .width(380.0)
        .show(egui_ctx, |ui| {
            let Some(dialog) = cx.state.sign_in.as_mut() else {
                return;
            };
            tabs(ui, &mut dialog.tab, &[s::TAB_STANDARD, s::TAB_ADVANCED]);
            let tab = dialog.tab;
            bevel_frame(ui, Bevel::Window, win95::theme::SILVER, 8, |ui| {
                ui.set_min_height(150.0);
                ui.set_width(ui.available_width());
                if tab == 0 {
                    standard_tab(ui, cx, &mut close);
                } else {
                    advanced_tab(ui, cx);
                }
            });
        });
    if r.close_requested || close {
        cx.worker.cancel_device_flow();
        cx.state.sign_in = None;
    }
}

fn standard_tab(ui: &mut egui::Ui, cx: &mut Ctx<'_>, close: &mut bool) {
    match cx.state.auth.clone() {
        Auth::Waiting {
            user_code,
            verification_uri,
        } => {
            ui.label(s::DEVICE_GO_TO);
            ui.hyperlink(&verification_uri);
            ui.label(s::DEVICE_ENTER_CODE);
            ui.vertical_centered(|ui| {
                ui.label(
                    RichText::new(&user_code)
                        .font(win95::theme::font(26.0))
                        .strong(),
                );
            });
            ui.horizontal(|ui| {
                if ui
                    .add(Button95::new(s::COPY_CODE).min_size(egui::vec2(90.0, 23.0)))
                    .clicked()
                {
                    ui.ctx().copy_text(user_code.clone());
                }
                if ui
                    .add(Button95::new(s::OPEN_BROWSER).min_size(egui::vec2(100.0, 23.0)))
                    .clicked()
                {
                    ui.ctx().open_url(egui::OpenUrl::new_tab(&verification_uri));
                }
            });
            ui.add_space(6.0);
            ui.label(s::WAITING_AUTH);
            ui.add(ProgressBar95::new(None).width(ui.available_width()));
            ui.add_space(6.0);
            if ui.add(Button95::new(s::CANCEL)).clicked() {
                cx.worker.cancel_device_flow();
                *close = true;
            }
        }
        auth => {
            ui.add(egui::Label::new(s::SIGN_IN_INTRO).wrap());
            ui.add_space(12.0);
            let busy = matches!(auth, Auth::Starting | Auth::Checking);
            if ui
                .add(Button95::new(s::SIGN_IN_BUTTON).enabled(!busy))
                .clicked()
            {
                cx.state.auth = Auth::Starting;
                cx.worker.send(Command::StartDeviceFlow);
            }
        }
    }
}

fn advanced_tab(ui: &mut egui::Ui, cx: &mut Ctx<'_>) {
    let Some(dialog) = cx.state.sign_in.as_mut() else {
        return;
    };
    ui.label(s::PAT_LABEL);
    let resp = text_field(ui, &mut dialog.pat, ui.available_width() - 8.0, true);
    ui.add(egui::Label::new(RichText::new(s::PAT_HELP).color(win95::theme::GRAY)).wrap());
    ui.add_space(8.0);
    let can_submit = !dialog.pat.trim().is_empty() && !dialog.pat_submitted;
    let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
    if (ui.add(Button95::new(s::OK).enabled(can_submit)).clicked() || enter) && can_submit {
        dialog.pat_submitted = true;
        let pat = std::mem::take(&mut dialog.pat);
        cx.worker.send(Command::SavePat(pat));
    }
}
```

- [ ] **Step 4: Clone dialog and progress dialog**

`crates/app/src/ui/clone_dialog.rs`:

```rust
use std::path::PathBuf;

use win95::{Button95, Cell, Column, Dialog, ListView, ProgressBar95, text_field};

use super::Ctx;
use crate::format::format_bytes;
use crate::protocol::Command;
use crate::state::{Auth, CloneDialog, filter_repos};
use crate::strings as s;

const COLUMNS: &[Column] = &[
    Column {
        title: s::COL_NAME,
        width: 190.0,
    },
    Column {
        title: s::COL_OWNER,
        width: 140.0,
    },
    Column {
        title: s::COL_PRIVATE,
        width: 55.0,
    },
    Column {
        title: s::COL_UPDATED,
        width: 85.0,
    },
];

/// Open the clone dialog (or the sign-in dialog if needed) and fetch repos once.
pub fn open(cx: &mut Ctx<'_>) {
    if !matches!(cx.state.auth, Auth::SignedIn(_)) {
        cx.state.sign_in.get_or_insert_with(Default::default);
        return;
    }
    let parent = cx
        .state
        .config
        .last_clone_dir
        .clone()
        .or_else(dirs::home_dir)
        .unwrap_or_default();
    cx.state.clone = Some(CloneDialog {
        dest_parent: parent.display().to_string(),
        ..Default::default()
    });
    if cx.state.repos.is_empty() && !cx.state.repos_loading {
        cx.state.repos_loading = true;
        cx.worker.send(Command::ListRepos);
    }
}

pub fn show(egui_ctx: &egui::Context, cx: &mut Ctx<'_>) {
    let Some(dialog) = cx.state.clone.as_ref() else {
        return;
    };
    if dialog.progress.is_some() {
        progress(egui_ctx, cx);
    } else {
        picker(egui_ctx, cx);
    }
}

fn progress(egui_ctx: &egui::Context, cx: &mut Ctx<'_>) {
    let Some(dialog) = cx.state.clone.as_ref() else {
        return;
    };
    let p = dialog.progress.unwrap_or_default();
    let title = format!("{} {}", s::CLONING_TITLE, dialog.cloning_name);
    let r = Dialog::new("clone_progress", &title)
        .width(340.0)
        .show(egui_ctx, |ui| {
            ui.label(format!(
                "{} {} / {}  ({})",
                s::RECEIVING,
                p.received_objects,
                p.total_objects,
                format_bytes(p.received_bytes)
            ));
            ui.add_space(4.0);
            ui.add(ProgressBar95::new(Some(p.fraction())).width(ui.available_width()));
            ui.add_space(8.0);
            ui.vertical_centered(|ui| ui.add(Button95::new(s::CANCEL)).clicked())
                .inner
        });
    if r.inner || r.close_requested {
        cx.worker.cancel_clone();
    }
}

fn picker(egui_ctx: &egui::Context, cx: &mut Ctx<'_>) {
    let mut start: Option<(String, PathBuf, String)> = None;
    let mut refresh = false;
    let mut close = false;
    let loading = cx.state.repos_loading;
    let repos = &cx.state.repos;
    let Some(dialog) = cx.state.clone.as_mut() else {
        return;
    };

    let r = Dialog::new("clone", s::CLONE_TITLE)
        .width(500.0)
        .show(egui_ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(s::FILTER);
                text_field(ui, &mut dialog.filter, 330.0, false);
                refresh = ui
                    .add(Button95::new(s::REFRESH).enabled(!loading))
                    .clicked();
            });
            ui.add_space(4.0);
            let visible = filter_repos(repos, &dialog.filter);
            let selected_row = visible
                .iter()
                .position(|&i| Some(&repos[i].full_name) == dialog.selected.as_ref());
            let list = ListView::new("clone_list", COLUMNS, visible.len())
                .height(240.0)
                .show(ui, selected_row, |row, col| {
                    let r = &repos[visible[row]];
                    match col {
                        0 => Cell::from(r.name.as_str()),
                        1 => Cell::from(r.owner.as_str()),
                        2 => Cell::from(if r.private { s::YES } else { "" }),
                        _ => Cell::from(r.updated_at.get(..10).unwrap_or("")),
                    }
                });
            if loading {
                ui.label(s::LOADING);
            }
            if let Some(row) = list.clicked.or(list.double_clicked) {
                dialog.selected = Some(repos[visible[row]].full_name.clone());
            }
            ui.add_space(6.0);
            ui.label(s::DEST_FOLDER);
            ui.horizontal(|ui| {
                text_field(ui, &mut dialog.dest_parent, 380.0, false);
                if ui.add(Button95::new(s::BROWSE)).clicked() {
                    let mut picker = rfd::FileDialog::new();
                    if !dialog.dest_parent.is_empty() {
                        picker = picker.set_directory(&dialog.dest_parent);
                    }
                    if let Some(folder) = picker.pick_folder() {
                        dialog.dest_parent = folder.display().to_string();
                    }
                }
            });
            let chosen = dialog
                .selected
                .as_ref()
                .and_then(|f| repos.iter().find(|r| &r.full_name == f));
            let parent = dialog.dest_parent.trim();
            if let Some(repo) = chosen
                && !parent.is_empty()
            {
                let dest = PathBuf::from(parent).join(&repo.name);
                ui.label(format!("{} {}", s::WILL_CLONE_INTO, dest.display()));
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let can_clone = chosen.is_some() && !parent.is_empty();
                let clicked = ui.add(Button95::new(s::CLONE).enabled(can_clone)).clicked();
                let double = list.double_clicked.is_some() && !parent.is_empty();
                if (clicked || double)
                    && let Some(repo) = dialog
                        .selected
                        .as_ref()
                        .and_then(|f| repos.iter().find(|r| &r.full_name == f))
                {
                    start = Some((
                        repo.clone_url.clone(),
                        PathBuf::from(parent).join(&repo.name),
                        repo.name.clone(),
                    ));
                }
                close = ui.add(Button95::new(s::CANCEL)).clicked();
            });
        });

    if let Some((url, dest, name)) = start {
        let parent = dialog.dest_parent.trim().to_string();
        dialog.progress = Some(Default::default());
        dialog.cloning_name = name;
        cx.state.config.last_clone_dir = Some(PathBuf::from(parent));
        cx.state.config_dirty = true;
        cx.worker.send(Command::Clone { url, dest });
    } else if r.close_requested || close {
        cx.state.clone = None;
    }
    if refresh {
        cx.state.repos_loading = true;
        cx.worker.send(Command::ListRepos);
    }
}
```

- [ ] **Step 5: Message box**

`crates/app/src/ui/message.rs`:

```rust
use win95::{Button95, Dialog, Icon};

use super::Ctx;
use crate::protocol::Severity;
use crate::strings as s;

/// Show the oldest queued message box, if any.
pub fn show(egui_ctx: &egui::Context, cx: &mut Ctx<'_>) {
    let Some(msg) = cx.state.messages.front().cloned() else {
        return;
    };
    let icon = match msg.severity {
        Severity::Error => Icon::Error,
        Severity::Warning => Icon::Warning,
        Severity::Info => Icon::Info,
    };
    let r = Dialog::new("message", s::ERR_TITLE)
        .width(380.0)
        .show(egui_ctx, |ui| {
            ui.horizontal(|ui| {
                win95::icon::icon(ui, icon);
                ui.vertical(|ui| {
                    ui.add(egui::Label::new(&msg.message).wrap());
                    if let Some(link) = &msg.link {
                        ui.hyperlink(link);
                    }
                    if let Some(detail) = &msg.detail {
                        ui.add(
                            egui::Label::new(egui::RichText::new(detail).color(win95::theme::GRAY))
                                .wrap(),
                        );
                    }
                });
            });
            ui.add_space(8.0);
            ui.vertical_centered(|ui| ui.add(Button95::new(s::OK)).clicked())
                .inner
        });
    if r.inner || r.close_requested {
        cx.state.messages.pop_front();
    }
}
```

- [ ] **Step 6: About box**

`crates/app/src/ui/about.rs`:

```rust
use win95::{Button95, Dialog, Icon};

use super::Ctx;
use crate::strings as s;

pub fn show(egui_ctx: &egui::Context, cx: &mut Ctx<'_>) {
    if !cx.state.about {
        return;
    }
    let r = Dialog::new("about", s::ABOUT_TITLE)
        .width(320.0)
        .show(egui_ctx, |ui| {
            ui.horizontal(|ui| {
                win95::icon::icon(ui, Icon::Info);
                ui.vertical(|ui| {
                    ui.label(format!("{} {}", s::APP_NAME, env!("CARGO_PKG_VERSION")));
                    ui.label(s::ABOUT_TAGLINE);
                    ui.label(s::ABOUT_FONT);
                });
            });
            ui.add_space(8.0);
            ui.vertical_centered(|ui| ui.add(Button95::new(s::OK)).clicked())
                .inner
        });
    if r.inner || r.close_requested {
        cx.state.about = false;
    }
}
```

- [ ] **Step 7: eframe application**

`crates/app/src/app.rs`:

```rust
//! The eframe application: drains worker events, draws screens, persists config.

use std::path::PathBuf;

use crate::config::WindowGeometry;
use crate::protocol::Command;
use crate::state::AppState;
use crate::ui::{self, Ctx};
use crate::worker::WorkerHandle;

pub struct RetroGitApp {
    state: AppState,
    worker: WorkerHandle,
    config_path: Option<PathBuf>,
    geometry: Option<WindowGeometry>,
}

impl RetroGitApp {
    pub fn new(state: AppState, worker: WorkerHandle, config_path: Option<PathBuf>) -> RetroGitApp {
        worker.send(Command::ValidateToken);
        RetroGitApp {
            state,
            worker,
            config_path,
            geometry: None,
        }
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
        if !self.worker.shutdown(std::time::Duration::from_secs(3)) {
            log::warn!("worker still busy at exit");
        }
        self.save_config();
    }
}
```

- [ ] **Step 8: Final crate root**

`crates/app/src/lib.rs`:

```rust
//! RetroGit application: state, background worker and screens.

pub mod app;
pub mod config;
pub mod format;
pub mod logging;
pub mod protocol;
pub mod state;
pub mod strings;
pub mod ui;
pub mod worker;

/// OAuth App client ID (public, no secret). Paste yours here, or build with
/// `RETROGIT_GITHUB_CLIENT_ID=Ov23li... cargo build`.
pub const GITHUB_CLIENT_ID: &str = match option_env!("RETROGIT_GITHUB_CLIENT_ID") {
    Some(id) => id,
    None => "",
};
```

- [ ] **Step 9: Entry point**

`with_decorations(false)`: we draw our own title bar. `windows_subsystem = "windows"` stops a console window from opening on Windows release builds.

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

- [ ] **Step 10: Build, lint and run all tests**

Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`

Expected: clippy clean; 80 tests pass, 0 failed.

- [ ] **Step 11: Manual smoke test (no OAuth App yet)**

Run: `cargo run -p retrogit`

Check, and fix before committing if anything is off:
1. A 900×600 window with no native title bar, a blue→light-blue gradient title "RetroGit", _ □ X buttons, grey 3D look, W95FA pixel font.
2. The window can be dragged by the title bar, resized from all edges and corners, minimized, maximized (button and double-click on the title) and closed.
3. "Sign in to GitHub" opens automatically (no token yet). Standard tab → **Sign in** shows the message box "This build has no GitHub OAuth App client ID…" (expected until Task 12).
4. Advanced tab: paste a GitHub PAT (`repo`, `read:org`, SSO-authorized for ExampleOrg) → OK → dialog closes, status bar shows `Signed in: @<login>`.
5. **Clone** → the list fills with personal and organization repos; typing in Filter narrows it; pick a small repo, **Browse…** a destination, **Clone** → progress dialog with blue blocks → the repo appears in the left list and its summary (branch, remote, last commit) in the centre.
6. **Clone** a bigger repo and press **Cancel** during the transfer → an "Clone cancelled." info box appears and the destination folder does not exist.
7. File > Open… on a plain folder → "This folder is not a Git repository." error box. On a repository → it is added to the list.
8. Quit, delete one cloned folder in Finder, relaunch → that entry is greyed; right-click → **Remove from list** removes it.
9. At rest, Activity Monitor shows ~0 % CPU for `retrogit`.
10. Font size: if W95FA looks blurry, try `FONT_SIZE` values 12.0 / 13.0 / 16.0 in `crates/win95/src/theme.rs` and keep the crispest; if you change it, re-run `cargo test --workspace`.

- [ ] **Step 12: Commit**

```bash
git add crates/app
git commit -m "feat(app): Win95 main window, sign-in, clone and message dialogs"
```


### Task 12: OAuth App, release checks and CI workflow

**Files:**
- Create: `.github/workflows/ci.yml`
- Modify: `crates/app/src/lib.rs` (paste the OAuth client ID)

**Interfaces:**
- Consumes: the finished app.
- Produces: a release binary verified for size and dynamic links, a working Device Flow, and a CI workflow ready for when the repo is pushed.

- [ ] **Step 1: Create the GitHub OAuth App (manual, 5 minutes)**

On github.com: Settings → Developer settings → OAuth Apps → **New OAuth App**.
- Application name: `RetroGit`
- Homepage URL: `https://github.com/<your-login>`
- Authorization callback URL: `http://127.0.0.1/` (unused by Device Flow, but required)
- Tick **Enable Device Flow**, then **Register application**.

Copy the **Client ID** (starts with `Ov23` or `Iv1.`). Do **not** generate a client secret.

- [ ] **Step 2: Embed the client ID**

In `crates/app/src/lib.rs`, replace `None => "",` with `None => "<your client id>",`. The ID is public by design (Device Flow uses no secret), so committing it is fine.

Run: `cargo test --workspace`

Expected: 80 passed (worker tests pass their own client IDs and are unaffected).

- [ ] **Step 3: Manual Device Flow check**

Sign out first if the PAT from Task 11 is still stored (File > Sign out). Then `cargo run -p retrogit`:
1. Standard tab → **Sign in** → an 8-character code and `https://github.com/login/device` appear, with the marquee progress bar.
2. **Copy code**, **Open browser**, paste the code, **Authorize**, and click **Authorize** next to ExampleOrg on the consent page (SSO).
3. Within a few seconds the dialog closes and the status bar shows `Signed in: @<login>`.
4. Quit and relaunch: you are signed in straight away (token read from the Keychain; macOS may ask once to allow access — choose **Always Allow**).
5. Keychain Access: an item `RetroGit` / `github.com` exists. `grep -r "gho_" "$HOME/Library/Application Support/RetroGit" "$HOME/Library/Application Support/RetroGit/retrogit.log"` finds nothing.
6. On the corporate network (TLS inspection proxy) repeat steps 1–3 and a clone: they must succeed (Review Focus 5).
7. Start a sign-in, then quit with the window's X while the code is displayed: the app closes within ~1 s.

- [ ] **Step 4: Release build checks (macOS)**

```bash
cargo build --release -p retrogit
ls -lh target/release/retrogit
otool -L target/release/retrogit
```

Expected: size roughly 10–15 MB (12 MB when this plan was written); `otool` lists only `/System/Library/...` and `/usr/lib/...` entries — **no** `/opt/homebrew/...openssl` line (OpenSSL is vendored).

- [ ] **Step 5: Add the CI workflow**

The repository stays local for now (agreed), so this workflow only runs once the repo is pushed to GitHub. It builds natively on each OS rather than cross-compiling (libgit2 is C).

`.github/workflows/ci.yml`:

```yaml
name: CI

on:
  push:
  pull_request:

jobs:
  build:
    strategy:
      fail-fast: false
      matrix:
        include:
          - os: macos-14
            target: aarch64-apple-darwin
            bin: retrogit
          - os: windows-latest
            target: x86_64-pc-windows-msvc
            bin: retrogit.exe
    runs-on: ${{ matrix.os }}
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          components: rustfmt, clippy
          targets: ${{ matrix.target }}
      - uses: Swatinem/rust-cache@v2
      - run: cargo fmt --all --check
      - run: cargo clippy --workspace --all-targets -- -D warnings
      - run: cargo test --workspace
      - run: cargo build --release -p retrogit --target ${{ matrix.target }}
      - uses: actions/upload-artifact@v4
        with:
          name: retrogit-${{ matrix.target }}
          path: target/${{ matrix.target }}/release/${{ matrix.bin }}
```

- [ ] **Step 6: Windows check (manual, when a Windows x86_64 machine or the CI artifact is available)**

On Windows with the MSVC toolchain and Perl available (needed only if you build locally; the vendored OpenSSL is not used on Windows — libgit2 uses WinHTTP): `cargo test --workspace` then `cargo run --release -p retrogit`, and repeat Task 11 smoke steps 1–7 and this task's Device Flow steps 1–4 (the token goes to Windows Credential Manager, entry `RetroGit`). No console window must open with the release build.

- [ ] **Step 7: Commit**

```bash
git add .github/workflows/ci.yml crates/app/src/lib.rs
git commit -m "chore: embed OAuth client ID and add CI workflow"
```
