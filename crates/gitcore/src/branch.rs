//! Branches: listing with libgit2, changes through the git command line.

use crate::{GitError, Repo};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Branch {
    /// `main` for a local branch, `origin/main` for a remote one.
    pub name: String,
    pub remote: bool,
    pub is_head: bool,
    /// Upstream of a local branch, e.g. `origin/main`.
    pub upstream: Option<String>,
    /// Commits on the branch that its upstream lacks.
    pub ahead: usize,
    /// Commits on the upstream that the branch lacks.
    pub behind: usize,
}

/// Files listed by git in "... would be overwritten by checkout:" errors.
pub fn parse_overwritten_files(output: &str) -> Vec<String> {
    let mut files = Vec::new();
    let mut in_list = false;
    for line in output.lines() {
        if line.contains("would be overwritten by") {
            in_list = true;
            continue;
        }
        if in_list {
            if let Some(f) = line.strip_prefix('\t') {
                files.push(f.trim().to_string());
            } else {
                in_list = false;
            }
        }
    }
    files
}

impl Repo {
    /// Local branches (alphabetical), then remote branches (alphabetical).
    pub fn branches(&self) -> Result<Vec<Branch>, GitError> {
        let repo = self.git();
        let map = |e: git2::Error| GitError::from_git2(&e);
        let mut out = Vec::new();
        for item in repo.branches(None).map_err(map)? {
            let (b, kind) = item.map_err(map)?;
            let Ok(Some(name)) = b.name().map(|n| n.map(str::to_string)) else {
                continue;
            };
            let remote = kind == git2::BranchType::Remote;
            if remote && name.ends_with("/HEAD") {
                continue;
            }
            let (mut upstream, mut ahead, mut behind) = (None, 0, 0);
            if !remote && let Ok(up) = b.upstream() {
                upstream = up.name().ok().flatten().map(str::to_string);
                if let (Some(l), Some(u)) = (b.get().target(), up.get().target()) {
                    (ahead, behind) = repo.graph_ahead_behind(l, u).unwrap_or((0, 0));
                }
            }
            out.push(Branch {
                is_head: b.is_head(),
                name,
                remote,
                upstream,
                ahead,
                behind,
            });
        }
        out.sort_by(|a, b| {
            (a.remote, a.name.to_lowercase()).cmp(&(b.remote, b.name.to_lowercase()))
        });
        Ok(out)
    }

    /// Current local branch, if HEAD is not detached.
    pub fn current_branch(&self) -> Option<Branch> {
        self.branches()
            .ok()?
            .into_iter()
            .find(|b| b.is_head && !b.remote)
    }

    /// `Err(reason)` if `name` is not a valid new branch name.
    pub fn validate_branch_name(&self, name: &str) -> Result<(), String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("Enter a branch name.".into());
        }
        if self
            .branches()
            .unwrap_or_default()
            .iter()
            .any(|b| !b.remote && b.name == name)
        {
            return Err(format!("A branch named '{name}' already exists."));
        }
        match self.run_git(&["check-ref-format", "--branch", name]) {
            Ok(out) if out.success => Ok(()),
            Ok(_) => Err(format!("'{name}' is not a valid branch name.")),
            Err(_) => {
                let bad = name.contains(' ')
                    || name.contains("..")
                    || name.starts_with('-')
                    || name.ends_with('/');
                if bad {
                    Err(format!("'{name}' is not a valid branch name."))
                } else {
                    Ok(())
                }
            }
        }
    }

    pub fn create_branch(&self, name: &str, switch: bool) -> Result<(), GitError> {
        let args: &[&str] = if switch {
            &["switch", "-c", name]
        } else {
            &["branch", name]
        };
        self.git_ok(args).map(|_| ())
    }

    /// `git switch <name>`; `WouldOverwrite` if local changes are in the way.
    pub fn switch_branch(&self, name: &str) -> Result<(), GitError> {
        let out = self.run_git(&["switch", name])?;
        if out.success {
            return Ok(());
        }
        let files = parse_overwritten_files(&out.text);
        if files.is_empty() {
            Err(GitError::Other(out.text))
        } else {
            Err(GitError::WouldOverwrite { files })
        }
    }

    /// Create the local branch tracking `remote_name` (e.g. `origin/feature`) and switch to it.
    pub fn checkout_remote_branch(&self, remote_name: &str) -> Result<(), GitError> {
        let out = self.run_git(&["switch", "--track", remote_name])?;
        if out.success {
            return Ok(());
        }
        let files = parse_overwritten_files(&out.text);
        if files.is_empty() {
            Err(GitError::Other(out.text))
        } else {
            Err(GitError::WouldOverwrite { files })
        }
    }

    pub fn rename_branch(&self, old: &str, new: &str) -> Result<(), GitError> {
        self.git_ok(&["branch", "-m", old, new]).map(|_| ())
    }

    /// Delete a local branch; `NotMerged` unless `force` when it has unmerged commits.
    pub fn delete_branch(&self, name: &str, force: bool) -> Result<(), GitError> {
        let out = self.run_git(&["branch", if force { "-D" } else { "-d" }, name])?;
        if out.success {
            Ok(())
        } else if out.text.contains("not fully merged") {
            Err(GitError::NotMerged(name.to_string()))
        } else {
            Err(GitError::Other(out.text))
        }
    }
}
