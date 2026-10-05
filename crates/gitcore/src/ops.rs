use crate::{GitError, Refusal, Repo};
use std::path::Path;

/// A multi-step operation left in progress (conflicts to resolve).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operation {
    Merge,
    Rebase,
    CherryPick,
    Revert,
}

/// How a cherry-pick, revert or rebase step ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpOutcome {
    Done,
    /// Stopped on conflicts: resolve, then continue (or abort).
    Conflicts,
    /// Nothing left to commit (already applied): skip, or abort.
    Empty,
}

/// How far `reset` goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResetMode {
    /// Keep the changes staged.
    Soft,
    /// Keep the changes, unstaged.
    Mixed,
    /// Throw the changes away (untracked files stay).
    Hard,
}

impl Repo {
    pub fn operation_in_progress(&self) -> Option<Operation> {
        use git2::RepositoryState as S;
        match self.git().state() {
            S::Merge => Some(Operation::Merge),
            S::Rebase | S::RebaseInteractive | S::RebaseMerge => Some(Operation::Rebase),
            S::CherryPick | S::CherryPickSequence => Some(Operation::CherryPick),
            S::Revert | S::RevertSequence => Some(Operation::Revert),
            _ => None,
        }
    }

    fn op_command(op: Operation) -> &'static str {
        match op {
            Operation::Merge => "merge",
            Operation::Rebase => "rebase",
            Operation::CherryPick => "cherry-pick",
            Operation::Revert => "revert",
        }
    }

    pub fn abort_operation(&self) -> Result<(), GitError> {
        if let Some(op) = self.operation_in_progress() {
            self.git_ok(&[Self::op_command(op), "--abort"])?;
        }
        self.forget_rebase_messages();
        Ok(())
    }

    /// Remove the message files of an interactive rebase once no operation needs them;
    /// returns the operation still in progress (read once).
    pub(crate) fn forget_rebase_messages(&self) -> Option<Operation> {
        let in_progress = self.operation_in_progress();
        if in_progress.is_none() {
            let _ = std::fs::remove_dir_all(self.git().path().join("retrogit-rebase"));
        }
        in_progress
    }

    /// Continue a rebase once conflicts are resolved and staged.
    pub fn continue_rebase(&self) -> Result<(), GitError> {
        let out = self.run_git(&["-c", "core.editor=true", "rebase", "--continue"])?;
        self.forget_rebase_messages();
        if out.success {
            Ok(())
        } else {
            Err(crate::classify_commit_failure(&out.text))
        }
    }

    /// Continue the cherry-pick, revert or rebase in progress (conflicts resolved and
    /// staged). A merge is finished by committing.
    pub fn continue_operation(&self) -> Result<OpOutcome, GitError> {
        let Some(op) = self.operation_in_progress() else {
            return Ok(OpOutcome::Done);
        };
        if op == Operation::Merge {
            return Err(GitError::Refused(Refusal::FinishMergeByCommit));
        }
        let out = self.run_git(&["-c", "core.editor=true", Self::op_command(op), "--continue"])?;
        self.outcome(out)
    }

    /// Skip the current commit of a cherry-pick, revert or rebase (e.g. already applied).
    pub fn skip_operation(&self) -> Result<OpOutcome, GitError> {
        let Some(op) = self.operation_in_progress() else {
            return Ok(OpOutcome::Done);
        };
        let out = self.run_git(&[Self::op_command(op), "--skip"])?;
        self.outcome(out)
    }

    /// Whether commit `id` has several parents.
    pub fn is_merge(&self, id: &str) -> Result<bool, GitError> {
        let oid = git2::Oid::from_str(id).map_err(|e| GitError::from_git2(&e))?;
        let c = self
            .git()
            .find_commit(oid)
            .map_err(|e| GitError::from_git2(&e))?;
        Ok(c.parent_count() > 1)
    }

    /// Copy commit `id` onto the current branch; a merge needs the parent its changes are
    /// taken against (`mainline`, 1-based).
    pub fn cherry_pick(&self, id: &str, mainline: Option<u32>) -> Result<OpOutcome, GitError> {
        let m = mainline.map(|m| m.to_string());
        let mut args = vec!["cherry-pick"];
        if let Some(m) = &m {
            args.extend(["-m", m]);
        }
        args.push(id);
        let out = self.run_git(&args)?;
        self.outcome(out)
    }

    /// Add a commit undoing `id`; a merge needs the parent to keep (`mainline`, 1-based).
    pub fn revert(&self, id: &str, mainline: Option<u32>) -> Result<OpOutcome, GitError> {
        let m = mainline.map(|m| m.to_string());
        let mut args = vec!["revert", "--no-edit"];
        if let Some(m) = &m {
            args.extend(["-m", m]);
        }
        args.push(id);
        let out = self.run_git(&args)?;
        self.outcome(out)
    }

    /// Move the current branch to `id`.
    pub fn reset(&self, id: &str, mode: ResetMode) -> Result<(), GitError> {
        let flag = match mode {
            ResetMode::Soft => "--soft",
            ResetMode::Mixed => "--mixed",
            ResetMode::Hard => "--hard",
        };
        self.git_ok(&["reset", "-q", flag, id]).map(|_| ())
    }

    /// Untracked files that `reset --hard id` would replace (they exist in `id`'s tree).
    pub fn reset_overwrites_untracked(&self, id: &str) -> Vec<String> {
        let Ok(files) = self.status() else {
            return Vec::new();
        };
        let repo = self.git();
        let Some(tree) = git2::Oid::from_str(id)
            .ok()
            .and_then(|o| repo.find_commit(o).ok())
            .and_then(|c| c.tree().ok())
        else {
            return Vec::new();
        };
        files
            .into_iter()
            .filter(|f| f.unstaged == Some(crate::Change::Untracked))
            .filter(|f| tree.get_path(Path::new(&f.path)).is_ok())
            .map(|f| f.path)
            .collect()
    }

    /// Done, stopped on conflicts, or empty (nothing to commit), from git's answer.
    pub(crate) fn outcome(&self, out: crate::cli::GitOutput) -> Result<OpOutcome, GitError> {
        let in_progress = self.forget_rebase_messages().is_some();
        if out.success && !in_progress {
            return Ok(OpOutcome::Done);
        }
        let text = out.text.to_lowercase();
        if !out.success && !in_progress && blocked_by_local_changes(&text) {
            return Err(GitError::WouldOverwrite {
                files: crate::parse_overwritten_files(&out.text),
            });
        }
        if in_progress {
            let empty = text.contains("nothing to commit")
                || text.contains("is now empty")
                || text.contains("previous cherry-pick is now empty");
            let conflicted = self
                .status()
                .map(|f| {
                    f.iter()
                        .any(|f| f.unstaged == Some(crate::Change::Conflicted))
                })
                .unwrap_or(false);
            if conflicted {
                return Ok(OpOutcome::Conflicts);
            }
            if empty {
                return Ok(OpOutcome::Empty);
            }
            if out.success {
                return Ok(OpOutcome::Done);
            }
            if text.contains("execution failed: git commit --amend") {
                return Err(GitError::MessageRefused {
                    output: out.text.trim().to_string(),
                });
            }
        }
        Err(crate::classify_commit_failure(&out.text))
    }
}

/// Git refused to start because of local changes ("would be overwritten", "cannot rebase:
/// You have unstaged changes"); nothing was changed.
fn blocked_by_local_changes(lowercase: &str) -> bool {
    lowercase.contains("would be overwritten by")
        || lowercase.contains("cannot rebase: you have unstaged changes")
        || lowercase.contains("cannot rebase: your index contains uncommitted changes")
}
