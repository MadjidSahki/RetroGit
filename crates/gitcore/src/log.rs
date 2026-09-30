//! Commit history across all branches.

use std::collections::HashMap;

use crate::{GitError, Repo};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefKind {
    Head,
    LocalBranch,
    RemoteBranch,
    Tag,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefLabel {
    pub name: String,
    pub kind: RefKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogEntry {
    pub id: String,
    pub short_id: String,
    pub parents: Vec<String>,
    pub author: String,
    pub email: String,
    /// Author time, seconds since the Unix epoch.
    pub time: i64,
    pub summary: String,
    pub refs: Vec<RefLabel>,
}

impl Repo {
    /// `limit` commits after skipping `skip`, from every local and remote branch and HEAD,
    /// in topological then time order (children before parents).
    pub fn log(&self, skip: usize, limit: usize) -> Result<Vec<LogEntry>, GitError> {
        let repo = self.git();
        let map = |e: git2::Error| GitError::from_git2(&e);
        let mut walk = repo.revwalk().map_err(map)?;
        walk.set_sorting(git2::Sort::TOPOLOGICAL | git2::Sort::TIME)
            .map_err(map)?;
        if repo.head().is_err() {
            return Ok(Vec::new()); // no commit yet
        }
        walk.push_head().map_err(map)?;
        walk.push_glob("refs/heads").map_err(map)?;
        walk.push_glob("refs/remotes").map_err(map)?;
        let labels = self.ref_labels()?;
        let mut out = Vec::with_capacity(limit);
        for oid in walk.skip(skip).take(limit) {
            let oid = oid.map_err(map)?;
            let c = repo.find_commit(oid).map_err(map)?;
            let id = oid.to_string();
            out.push(LogEntry {
                short_id: id.chars().take(7).collect(),
                parents: c.parent_ids().map(|p| p.to_string()).collect(),
                author: c.author().name().unwrap_or("").to_string(),
                email: c.author().email().unwrap_or("").to_string(),
                time: c.author().when().seconds(),
                summary: c.summary().ok().flatten().unwrap_or("").to_string(),
                refs: labels.get(&id).cloned().unwrap_or_default(),
                id,
            });
        }
        Ok(out)
    }

    /// Commit id -> refs pointing at it (HEAD first, then branches, then tags).
    fn ref_labels(&self) -> Result<HashMap<String, Vec<RefLabel>>, GitError> {
        let repo = self.git();
        let mut labels: HashMap<String, Vec<RefLabel>> = HashMap::new();
        if let Ok(head) = repo.head()
            && let Ok(c) = head.peel_to_commit()
        {
            labels
                .entry(c.id().to_string())
                .or_default()
                .push(RefLabel {
                    name: "HEAD".into(),
                    kind: RefKind::Head,
                });
        }
        let refs = repo.references().map_err(|e| GitError::from_git2(&e))?;
        for r in refs.flatten() {
            let Ok(full) = r.name() else { continue };
            let (kind, name) = if let Some(n) = full.strip_prefix("refs/heads/") {
                (RefKind::LocalBranch, n)
            } else if let Some(n) = full.strip_prefix("refs/remotes/") {
                if n.ends_with("/HEAD") {
                    continue;
                }
                (RefKind::RemoteBranch, n)
            } else if let Some(n) = full.strip_prefix("refs/tags/") {
                (RefKind::Tag, n)
            } else {
                continue;
            };
            let Ok(c) = r.peel_to_commit() else { continue };
            labels
                .entry(c.id().to_string())
                .or_default()
                .push(RefLabel {
                    name: name.to_string(),
                    kind,
                });
        }
        for v in labels.values_mut() {
            v.sort_by_key(|l| (l.kind as u8, l.name.clone()));
        }
        Ok(labels)
    }
}
