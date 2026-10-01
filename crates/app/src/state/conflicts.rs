//! State of the conflict editor (sub-project 6a).

use gitcore::{Change, Choice, ConflictFile, Pick, apply_choice, conflict_count};

use super::{AppState, ChangesView};
use crate::protocol::{AppError, Severity};
use crate::strings as s;

/// What waits for the user's confirmation in the conflict editor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConflictConfirm {
    /// Keep one side for the whole file.
    WholeFile(Pick),
    /// Mark resolved although conflict markers remain.
    ResolveWithMarkers,
    /// Leave the edited file for another one (`Some(path)`) or close the editor (`None`).
    Discard(Option<String>),
}

/// The file being resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConflictEditor {
    pub file: ConflictFile,
    /// The result pane's text (starts as the file Git left, with its markers).
    pub result: String,
    /// Conflict block shown as current (0-based, among those left in `result`).
    pub current: usize,
    /// The user changed `result`: reloads must not overwrite it.
    pub edited: bool,
    /// The file changed on disk while edited: offered with Reload.
    pub on_disk: Option<ConflictFile>,
    pub confirm: Option<ConflictConfirm>,
}

impl ConflictEditor {
    pub fn new(file: ConflictFile) -> ConflictEditor {
        ConflictEditor {
            result: file.working.clone().unwrap_or_default(),
            file,
            current: 0,
            edited: false,
            on_disk: None,
            confirm: None,
        }
    }

    pub fn conflicts_left(&self) -> usize {
        conflict_count(&self.result)
    }

    /// Replace the current block by `choice`; the next block becomes current.
    pub fn choose(&mut self, choice: Choice) {
        if self.current >= self.conflicts_left() {
            return;
        }
        self.result = apply_choice(&self.result, self.current, choice);
        self.edited = true;
        self.clamp();
    }

    pub fn next(&mut self) {
        self.current += 1;
        self.clamp();
    }

    pub fn previous(&mut self) {
        self.current = self.current.saturating_sub(1);
    }

    /// Keep `current` on an existing block (after edits).
    pub fn clamp(&mut self) {
        self.current = self.current.min(self.conflicts_left().saturating_sub(1));
    }

    /// The user typed in the result pane.
    pub fn edit(&mut self, text: String) {
        if text != self.result {
            self.result = text;
            self.edited = true;
            self.clamp();
        }
    }

    /// Take the version found on disk, dropping the edits.
    pub fn reload(&mut self) {
        if let Some(file) = self.on_disk.take() {
            *self = ConflictEditor::new(file);
        }
    }
}

impl ChangesView {
    /// Conflicted files, in status order.
    pub fn conflicted(&self) -> impl Iterator<Item = &gitcore::FileStatus> {
        self.files
            .iter()
            .filter(|f| f.unstaged == Some(Change::Conflicted))
    }

    /// Ask to show `path` in the conflict editor (the UI then loads it). With unsaved edits
    /// on another file, asks first.
    pub fn open_conflict(&mut self, path: &str) -> bool {
        if let Some(ed) = self.conflict.as_mut()
            && ed.edited
            && ed.file.path != path
        {
            ed.confirm = Some(ConflictConfirm::Discard(Some(path.to_string())));
            return false;
        }
        self.conflict_path = Some(path.to_string());
        self.shown = None;
        self.diff = None;
        true
    }

    /// Close the editor (asks first if there are unsaved edits).
    pub fn close_conflict(&mut self) {
        if let Some(ed) = self.conflict.as_mut()
            && ed.edited
        {
            ed.confirm = Some(ConflictConfirm::Discard(None));
            return;
        }
        self.conflict = None;
        self.conflict_path = None;
    }

    /// The user confirmed dropping the edits: go where they wanted.
    pub fn discard_conflict_edits(&mut self) -> Option<String> {
        let target = match self.conflict.as_ref().and_then(|e| e.confirm.clone()) {
            Some(ConflictConfirm::Discard(t)) => t,
            _ => return None,
        };
        self.conflict = None;
        self.conflict_path = None;
        match target {
            Some(path) => {
                self.open_conflict(&path);
                Some(path)
            }
            None => None,
        }
    }
}

impl AppState {
    pub(super) fn conflict_loaded(&mut self, file: ConflictFile) {
        let c = &mut self.changes;
        if c.conflict_path.as_deref() != Some(file.path.as_str()) {
            return;
        }
        match c.conflict.as_mut() {
            Some(ed) if ed.file.path == file.path && ed.edited => {
                // Never overwrite what the user typed; offer the new version instead.
                if file.working != ed.file.working {
                    ed.on_disk = Some(file);
                }
            }
            Some(ed) if ed.file == file => {}
            _ => c.conflict = Some(ConflictEditor::new(file)),
        }
    }

    /// `path` was resolved: show the next conflicted file, or say it is over.
    pub(super) fn conflict_resolved(&mut self, path: &str) {
        let c = &mut self.changes;
        if c.conflict.as_ref().is_some_and(|e| e.file.path == path) {
            c.conflict = None;
            c.conflict_path = None;
        }
        let next = c.conflicted().map(|f| f.path.clone()).find(|p| p != path);
        match next {
            Some(p) => {
                c.open_conflict(&p);
                c.load_conflict = true;
            }
            None => {
                let message = match self.operation {
                    Some(gitcore::Operation::Rebase) => s::ALL_RESOLVED_REBASE,
                    Some(gitcore::Operation::Merge) => s::ALL_RESOLVED_MERGE,
                    None => s::ALL_RESOLVED,
                };
                self.messages
                    .push_back(AppError::new(Severity::Info, message));
            }
        }
    }

    /// After a status refresh: an editor whose file is no longer in conflict closes.
    pub(super) fn sync_conflict_editor(&mut self) {
        let c = &mut self.changes;
        if let Some(path) = c.conflict_path.clone()
            && !c.conflicted().any(|f| f.path == path)
        {
            c.conflict = None;
            c.conflict_path = None;
        }
    }
}
