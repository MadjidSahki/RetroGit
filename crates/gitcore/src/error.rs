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
    /// RetroGit refuses the action; the app words it (gitcore has no user texts).
    #[error("refused: {0:?}")]
    Refused(Refusal),
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
    /// The server refused access ("Repository not found", HTTP 403): wrong account, missing
    /// SSO authorization, or an organization blocking the OAuth App.
    #[error("access denied: {0}")]
    AccessDenied(String),
    /// A push changing `.github/workflows` with a token without the `workflow` scope.
    #[error("the push changes GitHub workflows, which the token may not do")]
    MissingWorkflowScope(String),
    /// The lines a suggestion replaces changed since it was written.
    #[error("the lines changed since the suggestion was written")]
    SuggestionOutdated,
    /// The lines a suggestion replaces already read as the suggestion.
    #[error("the suggestion is already applied")]
    SuggestionApplied,
    /// During an interactive rebase, a new commit message was refused (hook, signing):
    /// the rebase is paused and continuing keeps the old message.
    #[error("the new commit message was refused")]
    MessageRefused { output: String },
    #[error("{0}")]
    Other(String),
}

/// Why an action is refused before anything is changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    LocalChangesFirst,
    ResolveConflictsFirst,
    OperationInProgress,
    /// A merge is finished by committing, not by "continue".
    FinishMergeByCommit,
    NothingToAmend,
    DetachedHead,
    /// `publish`: the branch can be published to get one (pull); otherwise just missing.
    NoUpstream {
        publish: bool,
    },
    InvalidTagName(String),
    InvalidIgnorePattern,
    /// Interactive rebase of merge commits.
    MergesInRange,
    BareRepository,
    /// A version gitcore could not resolve to a commit (file at, blame, tree...).
    UnknownRev(String),
    /// A revision the Explore view could not resolve.
    UnknownRevision(String),
    NotInConflict(String),
    PullNotFetched(u64),
    CommitNotFound,
    /// A partial (hunk or line) action on a file that only takes it whole.
    WholeFileOnly {
        kind: WholeKind,
        action: WholeAction,
    },
    /// The interactive rebase list cannot be run.
    Todo(crate::TodoError),
}

/// The kind of file a partial action refuses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WholeKind {
    Deleted,
    Symlink,
    Untracked,
    /// A Git filter (e.g. LFS).
    Filter,
    Binary,
    Renamed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WholeAction {
    Stage,
    Unstage,
    Discard,
    Restore,
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
