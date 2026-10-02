//! State of sub-project 6c: stashes, tags, and the dialogs of history operations.

use gitcore::{ChangedFile, FileDiff, OpOutcome, ResetMode, StashEntry, TodoItem};

use super::{AppState, Tab};
use crate::protocol::{AppError, Event, Severity};
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

impl AppState {
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
            Event::ResetInfo { id, drops_pushed } => {
                if let Some(GitDialog::Reset {
                    id: shown,
                    drops_pushed: d,
                    ..
                }) = self.git_dialog.as_mut()
                    && *shown == id
                {
                    *d = drops_pushed;
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
