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
