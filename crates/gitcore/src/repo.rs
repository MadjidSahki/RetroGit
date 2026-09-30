use std::path::{Path, PathBuf};

use crate::GitError;

/// An open local repository.
pub struct Repo {
    inner: git2::Repository,
    path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Head {
    /// HEAD points to a branch that has at least one commit.
    Branch(String),
    /// HEAD points to a branch with no commit yet (fresh `git init`).
    Unborn(String),
    /// HEAD is detached at this short commit id.
    Detached(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitInfo {
    pub short_id: String,
    pub summary: String,
    pub author: String,
    /// Seconds since the Unix epoch.
    pub time: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoSummary {
    /// Folder name of the working directory.
    pub name: String,
    pub path: PathBuf,
    pub head: Head,
    pub origin_url: Option<String>,
    pub last_commit: Option<CommitInfo>,
}

impl Repo {
    /// Open the repository whose working directory (or bare dir) is exactly `path`.
    pub fn open(path: &Path) -> Result<Repo, GitError> {
        let flags = git2::RepositoryOpenFlags::NO_SEARCH;
        match git2::Repository::open_ext(path, flags, std::iter::empty::<&std::ffi::OsStr>()) {
            Ok(inner) => Ok(Repo {
                inner,
                path: path.to_path_buf(),
            }),
            Err(e) if e.code() == git2::ErrorCode::NotFound => {
                Err(GitError::NotARepository(path.to_path_buf()))
            }
            Err(e) => Err(GitError::from_git2(&e)),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn summary(&self) -> Result<RepoSummary, GitError> {
        let name = self
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.path.display().to_string());
        let origin_url = self
            .inner
            .find_remote("origin")
            .ok()
            .and_then(|r| r.url().ok().map(str::to_owned));
        let (head, last_commit) = self.head_info()?;
        Ok(RepoSummary {
            name,
            path: self.path.clone(),
            head,
            origin_url,
            last_commit,
        })
    }

    fn head_info(&self) -> Result<(Head, Option<CommitInfo>), GitError> {
        let head_ref = match self.inner.head() {
            Ok(r) => r,
            Err(e) if e.code() == git2::ErrorCode::UnbornBranch => {
                let branch = self
                    .inner
                    .find_reference("HEAD")
                    .ok()
                    .and_then(|r| r.symbolic_target().ok().flatten().map(str::to_owned))
                    .map(|t| t.trim_start_matches("refs/heads/").to_string())
                    .unwrap_or_else(|| "HEAD".to_string());
                return Ok((Head::Unborn(branch), None));
            }
            Err(e) => return Err(GitError::from_git2(&e)),
        };
        let commit = head_ref
            .peel_to_commit()
            .map_err(|e| GitError::from_git2(&e))?;
        let info = CommitInfo {
            short_id: short_id(&commit),
            summary: commit.summary().ok().flatten().unwrap_or("").to_string(),
            author: commit.author().name().unwrap_or("").to_string(),
            time: commit.time().seconds(),
        };
        let detached = self
            .inner
            .head_detached()
            .map_err(|e| GitError::from_git2(&e))?;
        let head = if detached {
            Head::Detached(info.short_id.clone())
        } else {
            Head::Branch(head_ref.shorthand().unwrap_or("HEAD").to_string())
        };
        Ok((head, Some(info)))
    }
}

fn short_id(commit: &git2::Commit<'_>) -> String {
    let full = commit.id().to_string();
    full.chars().take(7).collect()
}
