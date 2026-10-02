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
    /// The server refused access ("Repository not found", HTTP 403): wrong account, missing
    /// SSO authorization, or an organization blocking the OAuth App.
    #[error("access denied: {0}")]
    AccessDenied(String),
    /// The lines a suggestion replaces changed since it was written.
    #[error("the lines changed since the suggestion was written")]
    SuggestionOutdated,
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
