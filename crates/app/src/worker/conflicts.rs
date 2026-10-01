//! Worker side of sub-project 6a: loading and resolving conflicted files.

use gitcore::{GitError, Repo};

use super::Worker;
use crate::protocol::{AppError, Event, Op};

impl Worker {
    pub(super) fn load_conflict(&mut self, path: &str) {
        let Some(repo) = self.open_current(Op::Changes) else {
            return;
        };
        self.shown = None;
        match repo.conflict(path) {
            Ok(file) => {
                self.shown_conflict = Some(path.to_string());
                self.emit(Event::ConflictLoaded(Box::new(file)));
            }
            Err(e) => self.fail(Op::Changes, AppError::from_git(&e)),
        }
    }

    /// Resolve `path` with `run`, refresh the status, then say it is resolved. On failure
    /// the editor keeps the user's text.
    pub(super) fn resolve(&mut self, path: &str, run: impl FnOnce(&Repo) -> Result<(), GitError>) {
        let Some(repo) = self.open_current(Op::Changes) else {
            return;
        };
        let result = run(&repo);
        if result.is_ok() && self.shown_conflict.as_deref() == Some(path) {
            self.shown_conflict = None;
        }
        self.refresh();
        match result {
            Ok(()) => self.emit(Event::ConflictResolved(path.to_string())),
            Err(e) => self.fail(Op::Changes, AppError::from_git(&e)),
        }
    }
}
