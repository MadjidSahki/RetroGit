//! Worker side of sub-project 6c: cherry-pick, revert, reset, interactive rebase, stashes
//! and tags.

use gitcore::{GitError, OpOutcome, Repo};

use super::Worker;
use crate::protocol::{AppError, Command, Event, Op, Severity, SyncOp};
use crate::strings as s;

impl Worker {
    pub(super) fn handle_git_ops(&mut self, cmd: Command) {
        self.git_op(cmd, true);
    }

    /// `offer`: local changes in the way open the Stash and retry dialog (otherwise they are
    /// an error: the retry after a stash must not ask again).
    fn git_op(&mut self, cmd: Command, offer: bool) {
        match cmd {
            Command::CherryPick(ref id) => {
                self.history_op(s::NOTE_CHERRY_PICKED, &cmd, offer, |r| r.cherry_pick(id))
            }
            Command::Revert { ref id, mainline } => {
                self.history_op(s::NOTE_REVERTED, &cmd, offer, |r| r.revert(id, mainline))
            }
            Command::Reset { ref id, mode } => self.history_op(s::NOTE_RESET, &cmd, offer, |r| {
                r.reset(id, mode).map(|()| OpOutcome::Done)
            }),
            Command::StashAndRetry { retry, files } => {
                let Some(r) = self.open_current(Op::Changes) else {
                    return;
                };
                match r.stash_save(s::STASH_RETRY_MESSAGE, true) {
                    Ok(true) => self.git_op(*retry, false),
                    // Ignored or skip-worktree files, case clashes: the same block again.
                    Ok(false) => self.fail(
                        Op::Changes,
                        AppError::new(Severity::Info, &s::nothing_to_stash(&files)),
                    ),
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
            } => self.history_op(s::NOTE_REBASED, &cmd, offer, |r| {
                r.interactive_rebase(base, items)
            }),
            Command::ContinueOperation => {
                self.history_op(s::NOTE_CONTINUED, &cmd, offer, Repo::continue_operation)
            }
            Command::SkipOperation => {
                self.history_op(s::NOTE_SKIPPED, &cmd, offer, Repo::skip_operation)
            }
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
                        Ok(()) => {
                            self.note(s::NOTE_TAG_CREATED);
                            self.emit(Event::TagsStatus(
                                s::TAG_CREATED_STATUS.replace("{name}", name.trim()),
                            ));
                        }
                        Err(e) => {
                            self.emit(Event::TagsStatus(s::TAG_ACTION_FAILED.into()));
                            self.fail(Op::History, AppError::from_git(&e));
                        }
                    }
                    self.after_ref_change(&r);
                    self.send_tags();
                }
            }
            Command::DeleteTag { name, remote } => {
                let r = if remote {
                    let Some((r, result)) = self.network(SyncOp::Push, false, |r, a, p, c| {
                        r.delete_remote_tag(a, &name, p, c)
                    }) else {
                        return;
                    };
                    // Origin refused or the network failed: the local tag is kept.
                    if let Err(e) = result {
                        self.tag_net_failed(&e);
                        self.send_tags();
                        return;
                    }
                    self.emit(Event::SyncFinished {
                        op: SyncOp::Push,
                        ok: true,
                    });
                    r
                } else {
                    let Some(r) = self.open_current(Op::History) else {
                        return;
                    };
                    r
                };
                match r.delete_tag(&name) {
                    Ok(()) => {
                        self.note(s::NOTE_TAG_DELETED);
                        self.emit(Event::TagsStatus(
                            s::TAG_DELETED_STATUS.replace("{name}", &name),
                        ));
                    }
                    Err(e) => {
                        self.emit(Event::TagsStatus(s::TAG_ACTION_FAILED.into()));
                        self.fail(Op::History, AppError::from_git(&e));
                    }
                }
                self.after_ref_change(&r);
                self.send_tags();
            }
            Command::PushTags(name) => {
                let Some((_r, result)) =
                    self.network(SyncOp::Push, false, |r, a, p, c| match &name {
                        Some(n) => r.push_tag(a, n, p, c),
                        None => r.push_tags(a, p, c),
                    })
                else {
                    return;
                };
                match result {
                    Ok(()) => {
                        self.emit(Event::SyncFinished {
                            op: SyncOp::Push,
                            ok: true,
                        });
                        self.note(s::NOTE_TAGS_PUSHED);
                        let text = match &name {
                            Some(n) => s::TAG_PUSHED_STATUS.replace("{name}", n),
                            None => s::TAGS_PUSHED_STATUS.to_string(),
                        };
                        self.emit(Event::TagsStatus(text));
                    }
                    Err(e) => self.tag_net_failed(&e),
                }
            }
            _ => {}
        }
    }

    /// A tag push or remote delete failed (or was cancelled): say so in the Tags window too.
    fn tag_net_failed(&mut self, e: &GitError) {
        let status = match e {
            GitError::Cancelled => s::CANCELLED,
            _ => s::TAG_ACTION_FAILED,
        };
        self.emit(Event::TagsStatus(status.into()));
        self.net_failed(SyncOp::Push, false, e);
    }

    fn note(&self, note: &str) {
        self.emit(Event::OpFinished {
            outcome: OpOutcome::Done,
            note: note.to_string(),
            stash_kept: false,
        });
    }

    /// Run a history operation, reload what depends on HEAD, then report its outcome.
    /// `offer_stash`: see `git_op`.
    fn history_op(
        &mut self,
        note: &str,
        cmd: &Command,
        offer_stash: bool,
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
                    stash_kept: false,
                });
            }
            Err(GitError::WouldOverwrite { files }) if offer_stash => self.emit(Event::OpBlocked {
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
                stash_kept: true,
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
