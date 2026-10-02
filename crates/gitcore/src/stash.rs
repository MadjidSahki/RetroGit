use crate::{GitError, Repo};

impl Repo {
    /// Put local changes (untracked files included) aside. `Ok(false)` if there was nothing.
    pub fn stash_push(&self, message: &str) -> Result<bool, GitError> {
        // Compare refs/stash before and after instead of reading (translatable) messages.
        let before = self.git().refname_to_id("refs/stash").ok();
        self.git_ok(&["stash", "push", "--include-untracked", "-m", message])?;
        Ok(self.git().refname_to_id("refs/stash").ok() != before)
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

/// A stash entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StashEntry {
    /// `n` of `stash@{n}`.
    pub index: usize,
    pub id: String,
    pub message: String,
    /// Branch it was made on.
    pub branch: String,
    /// Seconds since the Unix epoch.
    pub time: i64,
}

/// `On main: msg` / `WIP on main: abc subject` => (branch, message).
fn split_subject(subject: &str) -> (String, String) {
    let rest = subject
        .strip_prefix("WIP on ")
        .or_else(|| subject.strip_prefix("On "))
        .unwrap_or(subject);
    match rest.split_once(": ") {
        Some((b, m)) => (b.to_string(), m.to_string()),
        None => (String::new(), rest.to_string()),
    }
}

impl Repo {
    pub fn stashes(&self) -> Result<Vec<StashEntry>, GitError> {
        let out = self.git_ok(&["stash", "list", "--format=%gd%x00%H%x00%gs%x00%ct"])?;
        Ok(out
            .stdout
            .lines()
            .filter_map(|l| {
                let mut p = l.split('\0');
                let name = p.next()?;
                let index = name
                    .strip_prefix("stash@{")?
                    .strip_suffix('}')?
                    .parse()
                    .ok()?;
                let id = p.next()?.to_string();
                let (branch, message) = split_subject(p.next()?);
                let time = p.next()?.parse().unwrap_or(0);
                Some(StashEntry {
                    index,
                    id,
                    message,
                    branch,
                    time,
                })
            })
            .collect())
    }

    /// Stash local changes with `message` (untracked files too if asked). `Ok(false)`:
    /// nothing to stash.
    pub fn stash_save(&self, message: &str, include_untracked: bool) -> Result<bool, GitError> {
        let before = self.git().refname_to_id("refs/stash").ok();
        let mut args = vec!["stash", "push", "-m", message];
        if include_untracked {
            args.push("--include-untracked");
        }
        self.git_ok(&args)?;
        Ok(self.git().refname_to_id("refs/stash").ok() != before)
    }

    fn stash_ref(index: usize) -> String {
        format!("stash@{{{index}}}")
    }

    /// Re-apply stash `index`, keeping it. On conflicts it is kept: `StashConflict`.
    pub fn stash_apply(&self, index: usize) -> Result<(), GitError> {
        self.stash_run("apply", index)
    }

    /// Re-apply stash `index` and drop it (kept if it conflicts).
    pub fn stash_pop_at(&self, index: usize) -> Result<(), GitError> {
        self.stash_run("pop", index)
    }

    fn stash_run(&self, verb: &str, index: usize) -> Result<(), GitError> {
        let out = self.run_git(&["stash", verb, &Self::stash_ref(index)])?;
        if out.success {
            Ok(())
        } else if out.text.contains("CONFLICT") || out.text.contains("conflict") {
            Err(GitError::StashConflict)
        } else {
            Err(GitError::Other(out.text))
        }
    }

    /// Delete stash `index`; returns its commit id (it can still be recovered with it).
    pub fn stash_drop(&self, index: usize) -> Result<String, GitError> {
        let id = self
            .git_ok(&["rev-parse", &Self::stash_ref(index)])?
            .stdout
            .trim()
            .to_string();
        self.git_ok(&["stash", "drop", &Self::stash_ref(index)])?;
        Ok(id)
    }

    /// Files a stash changes (tracked files; untracked ones are not listed).
    pub fn stash_files(&self, index: usize) -> Result<Vec<crate::ChangedFile>, GitError> {
        let id = self
            .git_ok(&["rev-parse", &Self::stash_ref(index)])?
            .stdout
            .trim()
            .to_string();
        Ok(self.commit_detail(&id)?.files)
    }

    pub fn stash_file_diff(&self, index: usize, path: &str) -> Result<crate::FileDiff, GitError> {
        let id = self
            .git_ok(&["rev-parse", &Self::stash_ref(index)])?
            .stdout
            .trim()
            .to_string();
        self.commit_file_diff(&id, path)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn stash_subjects_give_branch_and_message() {
        assert_eq!(
            super::split_subject("On main: my work"),
            ("main".into(), "my work".into())
        );
        assert_eq!(
            super::split_subject("WIP on feat/x: abc123 Fix"),
            ("feat/x".into(), "abc123 Fix".into())
        );
    }
}
