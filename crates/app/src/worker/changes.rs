//! Worker side of sub-project 2: status, diff, staging, commit, .gitignore.

use gitcore::{FileDiff, Repo, RepoSummary, Selection, Side};

use super::Worker;
use crate::protocol::{AppError, Event, Op};

impl Worker {
    /// A repository was opened or cloned: it becomes the target of later commands.
    pub(super) fn opened(&mut self, summary: RepoSummary, cloned: bool) {
        let new_repo = self.repo.as_ref() != Some(&summary.path);
        if new_repo {
            self.shown = None;
            self.lease = None;
        }
        self.repo = Some(summary.path.clone());
        self.emit(if cloned {
            Event::CloneDone(summary)
        } else {
            Event::RepoOpened(summary)
        });
        self.refresh();
        let Some(repo) = self.open_current(Op::History) else {
            return;
        };
        self.load_repo_extras(&repo);
        if new_repo && !cloned {
            self.auto_fetch(&repo);
        }
    }

    pub(super) fn open_current(&self, during: Op) -> Option<Repo> {
        let path = self.repo.as_ref()?;
        match Repo::open(path) {
            Ok(r) => Some(r),
            Err(e) => {
                self.fail(during, AppError::from_git(&e));
                None
            }
        }
    }

    /// Send the status, and the diff of the displayed file.
    pub(super) fn refresh(&mut self) {
        let Some(repo) = self.open_current(Op::Changes) else {
            return;
        };
        match repo.status() {
            Ok(files) => self.emit(Event::StatusLoaded(files)),
            Err(e) => return self.fail(Op::Changes, AppError::from_git(&e)),
        }
        self.emit(Event::OperationChanged(repo.operation_in_progress()));
        if let Some((path, side)) = self.shown.clone() {
            self.send_diff(&repo, &path, side);
        }
    }

    fn send_diff(&self, repo: &Repo, path: &str, side: Side) {
        match repo.diff_file(path, side) {
            Ok(diff) => self.emit(Event::DiffLoaded(diff)),
            Err(e) => self.fail(Op::Changes, AppError::from_git(&e)),
        }
    }

    pub(super) fn load_diff(&mut self, path: String, side: Side) {
        let Some(repo) = self.open_current(Op::Changes) else {
            return;
        };
        self.send_diff(&repo, &path, side);
        self.shown = Some((path, side));
    }

    pub(super) fn stage(
        &mut self,
        path: &str,
        selection: &Selection,
        shown: Option<&FileDiff>,
        stage: bool,
    ) {
        let Some(repo) = self.open_current(Op::Changes) else {
            return;
        };
        let result = if stage {
            repo.stage(path, selection, shown)
        } else {
            repo.unstage(path, selection, shown)
        };
        if let Err(e) = result {
            self.fail(Op::Changes, AppError::from_git(&e));
        }
        // Always resync: on a stale selection this reloads the diff the user must redo.
        self.refresh();
    }

    pub(super) fn discard(&mut self, path: &str, selection: &Selection, shown: Option<&FileDiff>) {
        let Some(repo) = self.open_current(Op::Changes) else {
            return;
        };
        if let Err(e) = repo.discard(path, selection, shown) {
            self.fail(Op::Changes, AppError::from_git(&e));
        }
        self.refresh();
    }

    pub(super) fn discard_files(&mut self, paths: &[String]) {
        let Some(repo) = self.open_current(Op::Changes) else {
            return;
        };
        let refs: Vec<&str> = paths.iter().map(String::as_str).collect();
        if let Err(e) = repo.discard_files(&refs) {
            self.fail(Op::Changes, AppError::from_git(&e));
        }
        self.refresh();
    }

    pub(super) fn stage_files(&mut self, paths: &[String], stage: bool) {
        let Some(repo) = self.open_current(Op::Changes) else {
            return;
        };
        let refs: Vec<&str> = paths.iter().map(String::as_str).collect();
        let result = if stage {
            repo.stage_files(&refs)
        } else {
            repo.unstage_files(&refs)
        };
        if let Err(e) = result {
            self.fail(Op::Changes, AppError::from_git(&e));
        }
        self.refresh();
    }

    pub(super) fn commit(&mut self, message: &str, amend: bool) {
        let Some(repo) = self.open_current(Op::Commit) else {
            return;
        };
        // Rewriting a pushed commit: remember where the remote branch was (force-push lease).
        let lease = if amend && repo.head_is_pushed().unwrap_or(false) {
            repo.current_branch()
                .zip(repo.upstream_oid())
                .map(|(b, oid)| (b.name, oid))
        } else {
            None
        };
        match repo.commit(message, amend, self.deps.commit_backend) {
            Ok(outcome) => {
                if lease.is_some() {
                    self.lease = lease;
                }
                self.emit(Event::Committed(outcome));
            }
            Err(e) => self.fail(Op::Commit, AppError::from_git(&e)),
        }
        self.after_ref_change(&repo);
    }

    pub(super) fn add_to_gitignore(&mut self, pattern: &str) {
        let Some(repo) = self.open_current(Op::Changes) else {
            return;
        };
        if let Err(e) = repo.add_to_gitignore(pattern) {
            self.fail(Op::Changes, AppError::from_git(&e));
        }
        self.refresh();
    }

    pub(super) fn amend_info(&mut self) {
        let Some(repo) = self.open_current(Op::Commit) else {
            return;
        };
        let message = repo.last_commit_message().unwrap_or(None);
        let pushed = repo.head_is_pushed().unwrap_or(false);
        self.emit(Event::AmendInfo { message, pushed });
    }
}
