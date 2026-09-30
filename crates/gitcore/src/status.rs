use crate::{GitError, Repo};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    Added,
    Modified,
    Deleted,
    Renamed { from: String },
    TypeChange,
    Untracked,
    Conflicted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileStatus {
    /// Path relative to the repository root, `/`-separated.
    pub path: String,
    /// Change between HEAD and the index.
    pub staged: Option<Change>,
    /// Change between the index and the working tree.
    pub unstaged: Option<Change>,
}

impl Repo {
    /// Working tree status, sorted by path. Ignored files are excluded.
    pub fn status(&self) -> Result<Vec<FileStatus>, GitError> {
        let mut opts = git2::StatusOptions::new();
        opts.include_untracked(true)
            .recurse_untracked_dirs(true)
            .include_ignored(false)
            .exclude_submodules(true)
            .renames_head_to_index(true);
        let statuses = self
            .git()
            .statuses(Some(&mut opts))
            .map_err(|e| GitError::from_git2(&e))?;
        let mut out: Vec<FileStatus> = statuses
            .iter()
            .filter_map(|entry| {
                let s = entry.status();
                // For a staged rename, `entry.path()` is the old path: use the new one.
                let renamed_to = entry
                    .head_to_index()
                    .filter(|_| s.is_index_renamed())
                    .and_then(|d| {
                        d.new_file()
                            .path()
                            .map(|p| p.to_string_lossy().replace('\\', "/"))
                    });
                let path = match renamed_to {
                    Some(p) => p,
                    None => entry.path().ok()?.to_string(),
                };
                if s.is_conflicted() {
                    return Some(FileStatus {
                        path,
                        staged: None,
                        unstaged: Some(Change::Conflicted),
                    });
                }
                let staged = if s.is_index_new() {
                    Some(Change::Added)
                } else if s.is_index_modified() {
                    Some(Change::Modified)
                } else if s.is_index_deleted() {
                    Some(Change::Deleted)
                } else if s.is_index_renamed() {
                    let from = entry
                        .head_to_index()
                        .and_then(|d| {
                            d.old_file()
                                .path()
                                .map(|p| p.to_string_lossy().replace('\\', "/"))
                        })
                        .unwrap_or_default();
                    Some(Change::Renamed { from })
                } else if s.is_index_typechange() {
                    Some(Change::TypeChange)
                } else {
                    None
                };
                let unstaged = if s.is_wt_new() {
                    Some(Change::Untracked)
                } else if s.is_wt_modified() {
                    Some(Change::Modified)
                } else if s.is_wt_deleted() {
                    Some(Change::Deleted)
                } else if s.is_wt_typechange() {
                    Some(Change::TypeChange)
                } else {
                    None
                };
                (staged.is_some() || unstaged.is_some()).then_some(FileStatus {
                    path,
                    staged,
                    unstaged,
                })
            })
            .collect();
        out.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(out)
    }
}
