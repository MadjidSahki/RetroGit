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
    PullRequests,
    Stashes,
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
    /// Syntax colors of `detail_diff` (computed in the background).
    pub detail_colors: crate::highlight::Colors,
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
            detail_colors: crate::highlight::Colors::NotRequested,
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
                    h.detail_colors = crate::highlight::Colors::NotRequested;
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
            Event::PushRejected { can_force } => {
                self.dialog = Some(PendingDialog::PushRejected { can_force })
            }
            Event::WouldOverwrite { branch, files } => {
                self.dialog = Some(PendingDialog::WouldOverwrite { branch, files });
            }
            Event::NotMerged(name) => self.dialog = Some(PendingDialog::DeleteNotMerged { name }),
            Event::ColorsLoaded {
                target,
                diff,
                colors,
                dark,
            } => {
                if dark != self.colors_dark {
                    return;
                }
                use crate::highlight::{Colors, Target};
                let value = match colors {
                    Some(c) => Colors::Ready(c),
                    None => Colors::Plain,
                };
                // Results for a diff that is no longer displayed are dropped.
                match target {
                    Target::Changes if self.changes.diff.as_ref() == Some(&diff) => {
                        self.changes.diff_colors = value
                    }
                    Target::History if self.history.detail_diff.as_ref() == Some(&diff) => {
                        self.history.detail_colors = value
                    }
                    Target::History if self.stashes.diff.as_ref() == Some(&diff) => {
                        self.stashes.colors = value
                    }
                    Target::Pull if self.pulls.file_diff.as_ref() == Some(&diff) => {
                        self.pulls.file_colors = value
                    }
                    Target::ConflictMine | Target::ConflictTheirs | Target::ConflictResult => {
                        if let Some(ed) = self.changes.conflict.as_mut() {
                            ed.colors_loaded(target, &diff, value);
                        }
                    }
                    _ => {}
                }
            }
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
            h.detail_colors = crate::highlight::Colors::NotRequested;
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
    if !ref_name_ok(n) {
        return Some(format!("'{n}' is not a valid branch name."));
    }
    if existing.iter().any(|b| !b.remote && b.name == n) {
        return Some(format!("A branch named '{n}' already exists."));
    }
    None
}

/// Pure check of a new tag name against Git's rules and the existing tags.
pub fn tag_name_error(name: &str, existing: &[gitcore::Tag]) -> Option<String> {
    let n = name.trim();
    if n.is_empty() {
        return Some("Enter a tag name.".into());
    }
    if !ref_name_ok(n) {
        return Some(format!("'{n}' is not a valid tag name."));
    }
    if existing.iter().any(|t| t.name == n) {
        return Some(format!("A tag named '{n}' already exists."));
    }
    None
}

/// Git's rules for a reference name (`git check-ref-format`).
fn ref_name_ok(n: &str) -> bool {
    let bad_char = n
        .chars()
        .any(|c| c.is_whitespace() || c.is_control() || "~^:?*[\\".contains(c));
    !(bad_char
        || n.starts_with('-')
        || n.starts_with('/')
        || n.ends_with('/')
        || n.ends_with('.')
        || n.ends_with(".lock")
        || n.contains("..")
        || n.contains("//")
        || n.contains("@{")
        || n == "@"
        || n.split('/').any(|part| part.starts_with('.')))
}
