//! State of sub-project 6c: stashes, tags, and the dialogs of history operations.

use gitcore::{ChangedFile, FileDiff, OpOutcome, ResetMode, StashEntry, TodoItem};

use super::{AppState, Tab};
use crate::protocol::{AppError, Command, Event, Severity};
use crate::strings as s;

/// Dialogs of history operations, stashes and tags (at most one at a time).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitDialog {
    /// Revert of a merge: which parent to keep.
    RevertMerge {
        id: String,
        parent: u32,
    },
    Reset {
        id: String,
        mode: ResetMode,
        hard_confirmed: bool,
        drops_pushed: bool,
        overwrites: Vec<String>,
    },
    Rebase {
        base: String,
        items: Vec<TodoItem>,
        pushed: usize,
    },
    CreateTag {
        id: String,
        name: String,
        message: String,
        annotated: bool,
    },
    DeleteTag {
        name: String,
        remote: bool,
    },
    Tags {
        filter: String,
        selected: Option<String>,
    },
    StashSave {
        message: String,
        untracked: bool,
    },
    StashDrop {
        index: usize,
        id: String,
        message: String,
    },
    /// Skip leaves the current commit out (and any resolution done on it).
    ConfirmSkip,
    /// Local changes block `retry`: offer to stash them and run it again.
    StashRetry {
        retry: Box<Command>,
        files: Vec<String>,
    },
}

/// Move `items[i]` up (or down) by one, within bounds.
pub fn move_item(items: &mut [TodoItem], i: usize, up: bool) {
    if up && i > 0 && i < items.len() {
        items.swap(i, i - 1);
    } else if !up && i + 1 < items.len() {
        items.swap(i, i + 1);
    }
}

/// The Stashes tab.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StashesView {
    pub list: Vec<StashEntry>,
    /// `index` of the selected stash.
    pub selected: Option<usize>,
    pub files: Vec<ChangedFile>,
    pub file: Option<String>,
    pub diff: Option<FileDiff>,
    pub colors: crate::highlight::Colors,
}

impl StashesView {
    pub fn select(&mut self, index: usize) {
        if self.selected != Some(index) {
            self.selected = Some(index);
            self.files.clear();
            self.file = None;
            self.diff = None;
            self.colors = crate::highlight::Colors::NotRequested;
        }
    }
}

/// Entries of the History right-click menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryAction {
    CherryPick,
    Revert,
    Reset,
    RebaseFrom,
    CreateTag,
}

impl AppState {
    /// Runs a History menu entry on `e`: opens its dialog and/or returns the command to send.
    pub fn history_action(
        &mut self,
        action: HistoryAction,
        e: &gitcore::LogEntry,
    ) -> Option<Command> {
        let id = e.id.clone();
        match action {
            HistoryAction::CherryPick => Some(Command::CherryPick(id)),
            HistoryAction::Revert if e.parents.len() > 1 => {
                self.git_dialog = Some(GitDialog::RevertMerge { id, parent: 1 });
                None
            }
            HistoryAction::Revert => Some(Command::Revert { id, mainline: None }),
            HistoryAction::Reset => {
                self.git_dialog = Some(GitDialog::Reset {
                    id: id.clone(),
                    mode: ResetMode::Mixed,
                    hard_confirmed: false,
                    drops_pushed: false,
                    overwrites: Vec::new(),
                });
                Some(Command::LoadResetInfo(id))
            }
            HistoryAction::RebaseFrom => Some(Command::LoadRebaseList(Some(id))),
            HistoryAction::CreateTag => {
                self.git_dialog = Some(GitDialog::CreateTag {
                    id,
                    name: String::new(),
                    message: String::new(),
                    annotated: true,
                });
                None
            }
        }
    }

    pub(super) fn apply_git_ops(&mut self, event: Event) {
        match event {
            Event::OpFinished { outcome, note } => match outcome {
                OpOutcome::Done => self.sync.note = Some(note),
                OpOutcome::Conflicts => {
                    self.tab = Tab::Changes;
                    self.messages
                        .push_back(AppError::new(Severity::Info, s::INFO_CONFLICTS));
                }
                OpOutcome::Empty => {}
            },
            Event::OpBlocked { retry, files } => {
                self.git_dialog = Some(GitDialog::StashRetry { retry, files });
            }
            Event::ResetInfo {
                id,
                drops_pushed,
                overwrites,
            } => {
                if let Some(GitDialog::Reset {
                    id: shown,
                    drops_pushed: d,
                    overwrites: o,
                    ..
                }) = self.git_dialog.as_mut()
                    && *shown == id
                {
                    *d = drops_pushed;
                    *o = overwrites;
                }
            }
            Event::RebaseListLoaded {
                base,
                items,
                pushed,
            } => {
                self.git_dialog = Some(GitDialog::Rebase {
                    base,
                    items,
                    pushed,
                });
            }
            Event::StashesLoaded(list) => {
                let v = &mut self.stashes;
                if v.list != list {
                    v.list = list;
                    v.selected = None;
                    v.files.clear();
                    v.file = None;
                    v.diff = None;
                }
            }
            Event::StashFilesLoaded { index, files } => {
                if self.stashes.selected == Some(index) {
                    self.stashes.files = files;
                }
            }
            Event::StashFileDiffLoaded { index, diff } => {
                let v = &mut self.stashes;
                if v.selected == Some(index) && v.file.as_deref() == Some(diff.path.as_str()) {
                    v.diff = Some(diff);
                    v.colors = crate::highlight::Colors::NotRequested;
                }
            }
            Event::TagsLoaded(tags) => self.tags = tags,
            _ => {}
        }
    }
}
