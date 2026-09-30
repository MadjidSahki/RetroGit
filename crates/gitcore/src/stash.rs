use crate::{GitError, Repo};

impl Repo {
    /// Put local changes (untracked files included) aside. `Ok(false)` if there was nothing.
    pub fn stash_push(&self, message: &str) -> Result<bool, GitError> {
        let out = self.git_ok(&["stash", "push", "--include-untracked", "-m", message])?;
        Ok(!out.text.contains("No local changes to save"))
    }

    /// Re-apply the last stash. On conflicts the stash is kept and `StashConflict` returned.
    pub fn stash_pop(&self) -> Result<(), GitError> {
        let out = self.run_git(&["stash", "pop"])?;
        if out.success {
            Ok(())
        } else if out.text.contains("CONFLICT") || out.text.contains("conflict") {
            Err(GitError::StashConflict)
        } else {
            Err(GitError::Other(out.text))
        }
    }
}
