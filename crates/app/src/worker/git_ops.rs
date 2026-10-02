//! Worker side of sub-project 6c: cherry-pick, revert, reset, interactive rebase, stashes
//! and tags.

use gitcore::{GitError, OpOutcome, Repo};

use super::Worker;
use crate::protocol::{AppError, Command, Event, Op, Severity};
use crate::strings as s;

impl Worker {
    pub(super) fn handle_git_ops(&mut self, cmd: Command) {
        match cmd {
            Command::CherryPick(ref id) => {
                self.history_op(s::NOTE_CHERRY_PICKED, &cmd, |r| r.cherry_pick(id))
            }
            Command::Revert { ref id, mainline } => {
                self.history_op(s::NOTE_REVERTED, &cmd, |r| r.revert(id, mainline))
            }
            Command::Reset { ref id, mode } => self.history_op(s::NOTE_RESET, &cmd, |r| {
                r.reset(id, mode).map(|()| OpOutcome::Done)
            }),
            Command::StashAndRetry(retry) => {
                let Some(r) = self.open_current(Op::Changes) else {
                    return;
                };
                match r.stash_save(s::STASH_RETRY_MESSAGE, true) {
                    Ok(_) => self.handle_git_ops(*retry),
                    Err(e) => self.fail(Op::Changes, AppError::from_git(&e)),
                }
                self.send_stashes();
            }
            Command::LoadResetInfo(id) => {
                if let Some(r) = self.open_current(Op::History) {
                    let drops_pushed = r.reset_drops_pushed(&id);
                    let overwrites = r.reset_overwrites_untracked(&id);
                    self.emit(Event::ResetInfo {
                        id,
                        drops_pushed,
                        overwrites,
                    });
                }
            }
            Command::LoadRebaseList(base) => self.load_rebase_list(base),
            Command::InteractiveRebase {
                ref base,
                ref items,
            } => self.history_op(s::NOTE_REBASED, &cmd, |r| r.interactive_rebase(base, items)),
            Command::ContinueOperation => {
                self.history_op(s::NOTE_CONTINUED, &cmd, Repo::continue_operation)
            }
            Command::SkipOperation => self.history_op(s::NOTE_SKIPPED, &cmd, Repo::skip_operation),
            Command::LoadStashes => self.send_stashes(),
            Command::StashSave { message, untracked } => {
                let Some(r) = self.open_current(Op::Changes) else {
                    return;
                };
                match r.stash_save(&message, untracked) {
                    Ok(true) => self.note(s::NOTE_STASHED),
                    Ok(false) => self.fail(
                        Op::Changes,
                        AppError::new(Severity::Info, s::INFO_NOTHING_TO_STASH),
                    ),
                    Err(e) => self.fail(Op::Changes, AppError::from_git(&e)),
                }
                self.after_stash(&r);
            }
            Command::StashApply { index, id } => self.stash_op(index, &id, false),
            Command::StashPop { index, id } => self.stash_op(index, &id, true),
            Command::StashDrop { index: i, id } => {
                let Some(r) = self.open_current(Op::Changes) else {
                    return;
                };
                if !self.stash_is(&r, i, &id) {
                    return;
                }
                match r.stash_drop(i) {
                    Ok(id) => self.note(&s::NOTE_STASH_DROPPED.replace("{id}", &id)),
                    Err(e) => self.fail(Op::Changes, AppError::from_git(&e)),
                }
                self.send_stashes();
            }
            Command::LoadStashFiles(index) => {
                if let Some(r) = self.open_current(Op::Changes) {
                    match r.stash_files(index) {
                        Ok(files) => self.emit(Event::StashFilesLoaded { index, files }),
                        Err(e) => self.fail(Op::Changes, AppError::from_git(&e)),
                    }
                }
            }
            Command::LoadStashFileDiff { index, path } => {
                if let Some(r) = self.open_current(Op::Changes) {
                    match r.stash_file_diff(index, &path) {
                        Ok(diff) => self.emit(Event::StashFileDiffLoaded { index, diff }),
                        Err(e) => self.fail(Op::Changes, AppError::from_git(&e)),
                    }
                }
            }
            Command::LoadTags => self.send_tags(),
            Command::CreateTag { name, id, message } => {
                if let Some(r) = self.open_current(Op::History) {
                    match r.create_tag(name.trim(), &id, message.as_deref()) {
                        Ok(()) => self.note(s::NOTE_TAG_CREATED),
                        Err(e) => self.fail(Op::History, AppError::from_git(&e)),
                    }
                    self.after_ref_change(&r);
                    self.send_tags();
                }
            }
            Command::DeleteTag { name, remote } => {
                let Some(r) = self.open_current(Op::History) else {
                    return;
                };
                let auth = self.repo_net_auth();
                let never = std::sync::atomic::AtomicBool::new(false);
                let result = (if remote {
                    r.delete_remote_tag(&auth, &name, &never)
                } else {
                    Ok(())
                })
                .and_then(|()| r.delete_tag(&name));
                match result {
                    Ok(()) => self.note(s::NOTE_TAG_DELETED),
                    Err(e) => self.fail(Op::History, AppError::from_git(&e)),
                }
                self.after_ref_change(&r);
                self.send_tags();
            }
            Command::PushTags(name) => {
                let Some(r) = self.open_current(Op::Sync) else {
                    return;
                };
                let auth = self.repo_net_auth();
                let never = std::sync::atomic::AtomicBool::new(false);
                let result = match &name {
                    Some(n) => r.push_tag(&auth, n, &never),
                    None => r.push_tags(&auth, &never),
                };
                match result {
                    Ok(()) => self.note(s::NOTE_TAGS_PUSHED),
                    Err(e) => self.fail(Op::Sync, AppError::from_git(&e)),
                }
            }
            _ => {}
        }
    }

    fn note(&self, note: &str) {
        self.emit(Event::OpFinished {
            outcome: OpOutcome::Done,
            note: note.to_string(),
        });
    }

    /// Run a history operation, reload what depends on HEAD, then report its outcome.
    fn history_op(
        &mut self,
        note: &str,
        cmd: &Command,
        run: impl FnOnce(&Repo) -> Result<OpOutcome, GitError>,
    ) {
        let Some(repo) = self.open_current(Op::History) else {
            return;
        };
        let result = run(&repo);
        self.after_ref_change(&repo);
        match result {
            Ok(outcome) => {
                if outcome == OpOutcome::Empty {
                    self.fail(
                        Op::History,
                        AppError::new(Severity::Info, s::INFO_EMPTY_COMMIT),
                    );
                }
                self.emit(Event::OpFinished {
                    outcome,
                    note: note.to_string(),
                });
            }
            Err(GitError::WouldOverwrite { files }) => self.emit(Event::OpBlocked {
                retry: Box::new(cmd.clone()),
                files,
            }),
            Err(e) => self.fail(Op::History, AppError::from_git(&e)),
        }
    }

    fn load_rebase_list(&mut self, base: Option<String>) {
        let Some(repo) = self.open_current(Op::History) else {
            return;
        };
        // A commit picked in History must be in the current branch: rebasing onto another
        // branch (`--onto`) is not offered.
        if let Some(b) = &base
            && !repo.head_descends_from(b)
        {
            return self.fail(
                Op::History,
                AppError::new(Severity::Info, s::ERR_REBASE_NOT_ANCESTOR),
            );
        }
        let Some(base) = base.or_else(|| repo.upstream_oid()) else {
            return self.fail(
                Op::History,
                AppError::new(Severity::Info, s::ERR_REBASE_NO_BASE),
            );
        };
        match repo.rebase_list(&base) {
            Ok(items) => {
                let pushed = items.iter().filter(|i| repo.is_pushed(&i.id)).count();
                self.emit(Event::RebaseListLoaded {
                    base,
                    items,
                    pushed,
                });
            }
            Err(e) => self.fail(Op::History, AppError::from_git(&e)),
        }
    }

    /// Whether `stash@{index}` is still `id`; if not, says so and reloads the list.
    fn stash_is(&mut self, r: &Repo, index: usize, id: &str) -> bool {
        let same = r
            .stashes()
            .map(|l| l.iter().any(|e| e.index == index && e.id == id))
            .unwrap_or(false);
        if !same {
            self.fail(
                Op::Changes,
                AppError::new(Severity::Warning, s::ERR_STASH_LIST_CHANGED),
            );
            self.send_stashes();
        }
        same
    }

    fn stash_op(&mut self, index: usize, id: &str, pop: bool) {
        let Some(r) = self.open_current(Op::Changes) else {
            return;
        };
        if !self.stash_is(&r, index, id) {
            return;
        }
        let result = if pop {
            r.stash_pop_at(index)
        } else {
            r.stash_apply(index)
        };
        match result {
            Ok(()) => self.note(s::NOTE_STASH_APPLIED),
            Err(GitError::StashConflict) => self.emit(Event::OpFinished {
                outcome: OpOutcome::Conflicts,
                note: s::NOTE_STASH_APPLIED.to_string(),
            }),
            Err(e) => self.fail(Op::Changes, AppError::from_git(&e)),
        }
        self.after_stash(&r);
    }

    fn after_stash(&mut self, r: &Repo) {
        self.refresh();
        let _ = r;
        self.send_stashes();
    }

    pub(super) fn send_stashes(&mut self) {
        if let Some(r) = self.open_current(Op::Changes) {
            match r.stashes() {
                Ok(list) => self.emit(Event::StashesLoaded(list)),
                Err(e) => self.fail(Op::Changes, AppError::from_git(&e)),
            }
        }
    }

    fn send_tags(&mut self) {
        if let Some(r) = self.open_current(Op::History) {
            match r.tags() {
                Ok(tags) => self.emit(Event::TagsLoaded(tags)),
                Err(e) => self.fail(Op::History, AppError::from_git(&e)),
            }
        }
    }
}
