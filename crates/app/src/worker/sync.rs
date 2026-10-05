//! Worker side of sub-project 3: history, branches, fetch / pull / push.

use std::time::{Duration, Instant};

use gitcore::{GitError, NetAuth, PullMode, PushMode, Repo};

use super::{Throttle, Worker};
use crate::protocol::{AppError, Event, Op, SyncOp};
use crate::state::LOG_PAGE;
use crate::strings as s;

impl Worker {
    /// HEAD or refs changed: resend everything that depends on them.
    pub(super) fn after_ref_change(&mut self, repo: &Repo) {
        if let Ok(summary) = repo.summary() {
            self.emit(Event::RepoOpened(summary));
        }
        self.load_branches();
        self.load_log(0);
        self.refresh();
        // A pull or a switch may leave a stash behind (autostash, stash-and-reapply).
        self.send_stashes();
    }

    /// Everything the History tab and toolbar need for a freshly opened repo.
    pub(super) fn load_repo_extras(&mut self, repo: &Repo) {
        self.emit(Event::SigningLoaded(repo.signing_config().ok()));
        self.load_branches();
        self.load_log(0);
    }

    pub(super) fn load_log(&mut self, skip: usize) {
        let Some(repo) = self.open_current(Op::History) else {
            return;
        };
        match repo.log(skip, LOG_PAGE) {
            Ok(entries) => self.emit(Event::LogLoaded { skip, entries }),
            Err(e) => self.fail(Op::History, AppError::from_git(&e)),
        }
    }

    pub(super) fn load_commit(&mut self, id: &str) {
        let Some(repo) = self.open_current(Op::History) else {
            return;
        };
        match repo.commit_detail(id) {
            Ok(detail) => self.emit(Event::CommitLoaded(detail)),
            Err(e) => return self.fail(Op::History, AppError::from_git(&e)),
        }
        // Slow (runs gpg): sent separately after the detail.
        let status = repo
            .signature_status(id)
            .unwrap_or(gitcore::SignatureStatus::Unknown);
        self.emit(Event::SignatureLoaded {
            id: id.to_string(),
            status,
        });
    }

    pub(super) fn load_commit_file_diff(&mut self, id: &str, path: &str) {
        let Some(repo) = self.open_current(Op::History) else {
            return;
        };
        match repo.commit_file_diff(id, path) {
            Ok(diff) => self.emit(Event::CommitFileDiffLoaded {
                id: id.to_string(),
                diff,
            }),
            Err(e) => self.fail(Op::History, AppError::from_git(&e)),
        }
    }

    pub(super) fn load_branches(&mut self) {
        let Some(repo) = self.open_current(Op::History) else {
            return;
        };
        match repo.branches() {
            Ok(b) => self.emit(Event::BranchesLoaded(b)),
            Err(e) => self.fail(Op::History, AppError::from_git(&e)),
        }
    }

    /// Run a branch command, then resync; `WouldOverwrite` / `NotMerged` become dialogs.
    fn branch_op<E: Into<BranchFail>>(
        &mut self,
        run: impl FnOnce(&Repo) -> Result<(), E>,
        branch: &str,
    ) {
        let Some(repo) = self.open_current(Op::History) else {
            return;
        };
        let err = match run(&repo).map_err(Into::into) {
            Ok(()) => None,
            Err(BranchFail::Shown(a)) => {
                self.fail(Op::History, a);
                None
            }
            Err(BranchFail::Git(e)) => Some(e),
        };
        match err {
            None => {}
            Some(GitError::WouldOverwrite { files }) => {
                self.emit(Event::WouldOverwrite {
                    branch: branch.to_string(),
                    files,
                });
            }
            Some(GitError::NotMerged(name)) => self.emit(Event::NotMerged(name)),
            Some(e) => self.fail(Op::History, AppError::from_git(&e)),
        }
        self.after_ref_change(&repo);
    }

    pub(super) fn create_branch(&mut self, name: &str, switch: bool) {
        let name = name.trim().to_string();
        self.branch_op(|r| r.create_branch(&name, switch), &name.clone());
    }

    pub(super) fn switch_branch(&mut self, name: &str, stash: bool) {
        let remote = name.starts_with("origin/");
        let target = name.to_string();
        self.branch_op(
            |r| -> Result<(), BranchFail> {
                let switch = |r: &Repo| {
                    if remote {
                        r.checkout_remote_branch(&target)
                    } else {
                        r.switch_branch(&target)
                    }
                };
                if !stash {
                    return switch(r).map_err(BranchFail::Git);
                }
                let label = s::switch_stash_label(&target);
                let stashed = r.stash_push(&label)?;
                if let Err(e) = switch(r) {
                    // Put the changes back where they were; never lose track of them.
                    if stashed && r.stash_pop().is_err() {
                        let mut a = AppError::from_git(&e);
                        a.message = s::changes_kept_in_stash(&a.message, &label);
                        return Err(BranchFail::Shown(a));
                    }
                    // Already stashed: a second "would be overwritten" must not reopen the
                    // same dialog in a loop.
                    return Err(match e {
                        e @ GitError::WouldOverwrite { .. } => {
                            BranchFail::Shown(AppError::from_git(&e))
                        }
                        other => BranchFail::Git(other),
                    });
                }
                if stashed {
                    r.stash_pop().map_err(BranchFail::Git)
                } else {
                    Ok(())
                }
            },
            name,
        );
    }

    pub(super) fn rename_branch(&mut self, old: &str, new: &str) {
        let new = new.trim().to_string();
        self.branch_op(|r| r.rename_branch(old, &new), old);
    }

    pub(super) fn delete_branch(&mut self, name: &str, force: bool) {
        self.branch_op(|r| r.delete_branch(name, force), name);
    }

    pub(super) fn abort_operation(&mut self) {
        self.branch_op(|r| r.abort_operation(), "");
    }

    pub(super) fn continue_rebase(&mut self) {
        let Some(repo) = self.open_current(Op::Commit) else {
            return;
        };
        if let Err(e) = repo.continue_rebase() {
            self.fail(Op::Commit, AppError::from_git(&e));
        }
        self.after_ref_change(&repo);
    }

    /// Common shape of fetch / pull / push: start event, throttled progress, finish event.
    pub(super) fn network<T>(
        &mut self,
        op: SyncOp,
        background: bool,
        run: impl FnOnce(
            &Repo,
            &NetAuth,
            &mut dyn FnMut(gitcore::NetProgress),
            &std::sync::atomic::AtomicBool,
        ) -> Result<T, GitError>,
    ) -> Option<(Repo, Result<T, GitError>)> {
        let repo = self.open_current(Op::Sync)?;
        // A Cancel pressed for an earlier operation must not kill this one (the automatic
        // fetch at open is started here, not through `WorkerHandle::send`).
        self.cancel_net
            .store(false, std::sync::atomic::Ordering::SeqCst);
        self.emit(Event::SyncStarted { op, background });
        let auth = self.repo_net_auth();
        let mut throttle = Throttle::new(Duration::from_millis(100));
        let emit = &self.emit;
        let mut on_progress = |p: gitcore::NetProgress| {
            if throttle.ready(Instant::now()) {
                emit(Event::SyncProgress(p));
            }
        };
        let result = run(&repo, &auth, &mut on_progress, &self.cancel_net);
        Some((repo, result))
    }

    /// Report a network failure (background fetches only log it).
    pub(super) fn net_failed(&mut self, op: SyncOp, background: bool, e: &GitError) {
        if background {
            log::info!("background fetch failed: {e}");
        } else {
            let mut error = match e {
                GitError::Cancelled => AppError::new(crate::protocol::Severity::Info, s::CANCELLED),
                GitError::Auth(detail) if detail.contains("401") => {
                    AppError::from_github(&github::GithubError::Unauthorized)
                }
                _ => AppError::from_git(e),
            };
            if matches!(e, GitError::Auth(_)) {
                error.message = format!("{}\n\n{}", s::ERR_NET_AUTH_HELP, error.message);
            }
            if matches!(e, GitError::AccessDenied(_)) {
                error.link = Some(self.sso_settings_link());
            }
            self.fail(Op::Sync, error);
        }
        self.emit(Event::SyncFinished { op, ok: false });
    }

    pub(super) fn fetch(&mut self, background: bool) {
        let Some((repo, result)) =
            self.network(SyncOp::Fetch, background, |r, a, p, c| r.fetch(a, p, c))
        else {
            return;
        };
        match result {
            Ok(()) => self.emit(Event::SyncFinished {
                op: SyncOp::Fetch,
                ok: true,
            }),
            Err(e) => self.net_failed(SyncOp::Fetch, background, &e),
        }
        self.load_branches();
        self.load_log(0);
        drop(repo);
    }

    pub(super) fn pull(&mut self, mode: PullMode) {
        let Some((repo, result)) =
            self.network(SyncOp::Pull, false, |r, a, p, c| r.pull(a, mode, p, c))
        else {
            return;
        };
        match result {
            Ok(outcome) => {
                self.emit(Event::Pulled(outcome));
                self.emit(Event::SyncFinished {
                    op: SyncOp::Pull,
                    ok: true,
                });
            }
            Err(GitError::Diverged { ahead, behind }) => {
                self.emit(Event::SyncFinished {
                    op: SyncOp::Pull,
                    ok: false,
                });
                self.emit(Event::Diverged { ahead, behind });
            }
            Err(e) => self.net_failed(SyncOp::Pull, false, &e),
        }
        self.after_ref_change(&repo);
    }

    /// Whether the current branch is the one whose pushed commit was amended.
    fn lease_for(&self, repo: &Repo) -> Option<String> {
        let (branch, oid) = self.lease.as_ref()?;
        (repo.current_branch()?.name == *branch).then(|| oid.clone())
    }

    pub(super) fn push(&mut self, mode: PushMode) {
        let Some((repo, result)) =
            self.network(SyncOp::Push, false, |r, a, p, c| r.push(a, mode, p, c))
        else {
            return;
        };
        match result {
            Ok(()) => {
                self.lease = None;
                self.emit(Event::SyncFinished {
                    op: SyncOp::Push,
                    ok: true,
                });
            }
            Err(GitError::PushRejected) => {
                self.emit(Event::SyncFinished {
                    op: SyncOp::Push,
                    ok: false,
                });
                let can_force = self.lease_for(&repo).is_some();
                self.emit(Event::PushRejected { can_force });
            }
            Err(e) => self.net_failed(SyncOp::Push, false, &e),
        }
        self.after_ref_change(&repo);
    }

    pub(super) fn force_push(&mut self) {
        let Some(repo) = self.open_current(Op::Sync) else {
            return;
        };
        let Some(expected) = self.lease_for(&repo) else {
            return self.fail(Op::Sync, AppError::from_git(&GitError::PushRejected));
        };
        drop(repo);
        let Some((repo, result)) = self.network(SyncOp::Push, false, |r, a, p, c| {
            r.push_force_with_lease(a, &expected, p, c)
        }) else {
            return;
        };
        match result {
            Ok(()) => {
                self.lease = None;
                self.emit(Event::SyncFinished {
                    op: SyncOp::Push,
                    ok: true,
                });
            }
            Err(GitError::PushRejected) => {
                // Someone pushed after our amend: never force over their work.
                self.emit(Event::SyncFinished {
                    op: SyncOp::Push,
                    ok: false,
                });
                self.emit(Event::PushRejected { can_force: false });
            }
            Err(e) => self.net_failed(SyncOp::Push, false, &e),
        }
        self.after_ref_change(&repo);
    }

    /// Fetch once when a repo is opened, unless it would need credentials we don't have.
    pub(super) fn auto_fetch(&mut self, repo: &Repo) {
        let url = repo
            .summary()
            .ok()
            .and_then(|s| s.origin_url)
            .unwrap_or_default();
        if url.is_empty()
            || (url.starts_with("https://github.com/") && self.accounts.list().is_empty())
        {
            return;
        }
        self.fetch(true);
    }
}

/// Why a branch operation failed: from git, or already worded for the user.
enum BranchFail {
    Git(GitError),
    Shown(AppError),
}

impl From<GitError> for BranchFail {
    fn from(e: GitError) -> Self {
        BranchFail::Git(e)
    }
}
