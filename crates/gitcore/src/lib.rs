//! RetroGit's internal Git API. No `git2` type is ever exposed publicly.

mod branch;
mod cli;
mod clone;
mod commit;
mod commit_detail;
mod conflict;
mod diff;
mod discard;
mod error;
mod explore;
mod github;
mod graph;
mod ignore;
mod log;
mod net;
mod ops;
mod rebase_todo;
mod remote;
mod repo;
mod search;
mod signing;
mod stage;
mod stash;
mod status;
mod suggestion;
mod tags;

pub use branch::{Branch, parse_overwritten_files};
pub use commit_detail::{ChangedFile, CommitDetail, SignatureStatus, parse_signature_status};
pub use conflict::{
    Choice, ConflictFile, ConflictKind, Pane, Pick, Segment, apply_choice, block_lines,
    conflict_count, has_marker_lines, locate_blocks, parse_conflicts,
};
pub use github::parse_github_slug;
pub use graph::{Edge, GraphRow, GraphState, layout};
pub use log::{LogEntry, RefKind, RefLabel};
pub use net::{
    ASKPASS_TOKEN_VAR, NetAuth, NetSettings, askpass_answer, net_settings, set_askpass_program,
};
pub use ops::{OpOutcome, Operation, ResetMode};
pub use rebase_todo::{TodoAction, TodoItem, TodoText, todo_text, validate_todo};
pub use remote::{
    NetProgress, PullMode, PullOutcome, PushMode, classify_net_failure, parse_progress,
    retry_without_token, strip_progress,
};
pub use signing::{SigningConfig, SigningFormat};
pub use stash::StashEntry;
pub use suggestion::replace_lines;
pub use tags::Tag;

pub use clone::{CloneProgress, CloneRequest, Credentials, clone};
pub use commit::{
    CommitBackend, CommitOutcome, classify_commit_failure, git_available, set_git_search_path,
};
pub use diff::{DiffLine, FileDiff, Hunk, LineKind, Side};
pub use error::GitError;
pub use explore::{
    BlameBlock, EntryKind, FileCommit, FileContent, MAX_FILE_BYTES, TreeEntry,
    parse_blame_porcelain,
};
pub use repo::{CommitInfo, Head, Repo, RepoSummary};
pub use search::{ExploreRef, GrepMatch, GrepResult, LogSearch, parse_grep};
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
