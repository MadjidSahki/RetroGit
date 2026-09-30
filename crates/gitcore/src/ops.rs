use crate::{GitError, Repo};

/// A multi-step operation left in progress (conflicts to resolve).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operation {
    Merge,
    Rebase,
}

impl Repo {
    pub fn operation_in_progress(&self) -> Option<Operation> {
        use git2::RepositoryState as S;
        match self.git().state() {
            S::Merge => Some(Operation::Merge),
            S::Rebase | S::RebaseInteractive | S::RebaseMerge => Some(Operation::Rebase),
            _ => None,
        }
    }

    pub fn abort_operation(&self) -> Result<(), GitError> {
        match self.operation_in_progress() {
            Some(Operation::Merge) => self.git_ok(&["merge", "--abort"]).map(|_| ()),
            Some(Operation::Rebase) => self.git_ok(&["rebase", "--abort"]).map(|_| ()),
            None => Ok(()),
        }
    }

    /// Continue a rebase once conflicts are resolved and staged.
    pub fn continue_rebase(&self) -> Result<(), GitError> {
        let out = self.run_git(&["-c", "core.editor=true", "rebase", "--continue"])?;
        if out.success {
            Ok(())
        } else {
            Err(crate::classify_commit_failure(&out.text))
        }
    }
}
