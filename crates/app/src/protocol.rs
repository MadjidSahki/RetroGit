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
