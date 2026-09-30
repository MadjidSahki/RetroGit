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
    retry_without_token,
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
