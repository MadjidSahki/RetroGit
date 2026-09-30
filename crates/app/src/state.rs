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
    /// Syntax colors of `diff`, computed once by the UI: `Some(None)` = not highlightable.
    pub diff_colors: Option<Option<crate::highlight::DiffColors>>,
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
                        c.diff_colors = None;
                        c.diff = Some(diff);
                        c.selected_lines.clear();
                        c.show_large = false;
                    }
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
            | Event::PushRejected { .. }
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
