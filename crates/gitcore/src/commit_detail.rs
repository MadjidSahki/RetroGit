//! One commit: message, files and per-file diffs; signature status.

use crate::diff::{FileDiff, Side, file_diff_from};
use crate::status::Change;
use crate::{GitError, LogEntry, Repo};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangedFile {
    pub path: String,
    pub change: Change,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitDetail {
    pub id: String,
    pub short_id: String,
    pub parents: Vec<String>,
    pub author: String,
    pub email: String,
    pub time: i64,
    pub committer: String,
    /// Full message (summary, blank line, body).
    pub message: String,
    /// Changes against the first parent (everything is "added" for a root commit).
    pub files: Vec<ChangedFile>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignatureStatus {
    Good {
        signer: String,
    },
    Bad,
    /// Signed, but the signature could not be checked (missing key, expired...).
    Unknown,
    Unsigned,
}

/// Map `git log --format=%G?%n%GS` output to a status.
pub fn parse_signature_status(output: &str) -> SignatureStatus {
    let mut lines = output.lines();
    let code = lines.next().unwrap_or("").trim();
    let signer = lines.next().unwrap_or("").trim().to_string();
    match code {
        "G" | "U" => SignatureStatus::Good { signer },
        "B" | "R" => SignatureStatus::Bad,
        "N" | "" => SignatureStatus::Unsigned,
        _ => SignatureStatus::Unknown,
    }
}

impl Repo {
    fn find_commit(&self, id: &str) -> Result<git2::Commit<'_>, GitError> {
        let map = |e: git2::Error| GitError::from_git2(&e);
        let oid = git2::Oid::from_str(id).map_err(map)?;
        self.git().find_commit(oid).map_err(map)
    }

    fn commit_diff(
        &self,
        c: &git2::Commit<'_>,
        path: Option<&str>,
    ) -> Result<git2::Diff<'_>, GitError> {
        let map = |e: git2::Error| GitError::from_git2(&e);
        let tree = c.tree().map_err(map)?;
        let parent_tree = match c.parent(0) {
            Ok(p) => Some(p.tree().map_err(map)?),
            Err(_) => None,
        };
        let mut opts = git2::DiffOptions::new();
        if let Some(p) = path {
            opts.pathspec(p).disable_pathspec_match(true);
        }
        let mut diff = self
            .git()
            .diff_tree_to_tree(parent_tree.as_ref(), Some(&tree), Some(&mut opts))
            .map_err(map)?;
        if path.is_none() {
            diff.find_similar(None).map_err(map)?;
        }
        Ok(diff)
    }

    pub fn commit_detail(&self, id: &str) -> Result<CommitDetail, GitError> {
        let c = self.find_commit(id)?;
        let diff = self.commit_diff(&c, None)?;
        let files = diff
            .deltas()
            .filter_map(|d| {
                let path = d.new_file().path().or_else(|| d.old_file().path())?;
                let path = path.to_string_lossy().replace('\\', "/");
                let change = match d.status() {
                    git2::Delta::Added => Change::Added,
                    git2::Delta::Deleted => Change::Deleted,
                    git2::Delta::Renamed => Change::Renamed {
                        from: d
                            .old_file()
                            .path()
                            .map(|p| p.to_string_lossy().replace('\\', "/"))
                            .unwrap_or_default(),
                    },
                    git2::Delta::Typechange => Change::TypeChange,
                    _ => Change::Modified,
                };
                Some(ChangedFile { path, change })
            })
            .collect();
        let full = c.id().to_string();
        Ok(CommitDetail {
            short_id: full.chars().take(7).collect(),
            id: full,
            parents: c.parent_ids().map(|p| p.to_string()).collect(),
            author: c.author().name().unwrap_or("").to_string(),
            email: c.author().email().unwrap_or("").to_string(),
            time: c.author().when().seconds(),
            committer: c.committer().name().unwrap_or("").to_string(),
            message: c.message().ok().unwrap_or("").trim_end().to_string(),
            files,
        })
    }

    /// Diff of one file in a commit, against its first parent.
    pub fn commit_file_diff(&self, id: &str, path: &str) -> Result<FileDiff, GitError> {
        let c = self.find_commit(id)?;
        let diff = self.commit_diff(&c, Some(path))?;
        file_diff_from(&diff, path, Side::Staged)
    }

    /// Signature status of a commit (runs gpg/ssh through `git`: call for one commit at a time).
    pub fn signature_status(&self, id: &str) -> Result<SignatureStatus, GitError> {
        let out = self.git_ok(&["log", "-1", "--format=%G?%n%GS", id])?;
        Ok(parse_signature_status(&out.stdout))
    }
}

impl From<&CommitDetail> for LogEntry {
    fn from(d: &CommitDetail) -> LogEntry {
        LogEntry {
            id: d.id.clone(),
            short_id: d.short_id.clone(),
            parents: d.parents.clone(),
            author: d.author.clone(),
            email: d.email.clone(),
            time: d.time,
            summary: d.message.lines().next().unwrap_or("").to_string(),
            refs: vec![],
        }
    }
}
