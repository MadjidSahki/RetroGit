//! Throwing away working-tree changes (the index is never touched).

use std::path::Path;

use crate::diff::{LineKind, Side};
use crate::stage::{Direction, Selection, apply_selection};
use crate::status::Change;
use crate::{FileDiff, GitError, Repo};

impl Repo {
    /// Revert a selection of the unstaged changes of `path` in the working tree.
    /// `shown` is the diff the selection was made on (stale-selection check).
    pub fn discard(
        &self,
        path: &str,
        selection: &Selection,
        shown: Option<&FileDiff>,
    ) -> Result<(), GitError> {
        if *selection == Selection::All {
            return self.discard_files(&[path]);
        }
        let rel = Path::new(path);
        let file = self.workdir()?.join(rel);
        if file.symlink_metadata().is_err() {
            return Err(GitError::Unsupported(
                "deleted files can only be restored as a whole".into(),
            ));
        }
        if let Some(f) = self.status()?.into_iter().find(|f| f.path == path)
            && f.unstaged == Some(Change::Conflicted)
        {
            return Err(GitError::Unsupported("resolve conflicts first".into()));
        }
        let filtered = self
            .git()
            .get_attr(rel, "filter", git2::AttrCheckFlags::FILE_THEN_INDEX)
            .ok()
            .flatten()
            .is_some();
        if filtered {
            return Err(GitError::Unsupported(
                "files with a Git filter (e.g. LFS) can only be discarded as a whole".into(),
            ));
        }
        let mut current = self.diff_file(path, Side::Unstaged)?;
        if shown.is_some_and(|s| *s != current) {
            return Err(GitError::StaleSelection);
        }
        if current.binary {
            return Err(GitError::Unsupported(
                "binary files can only be discarded as a whole".into(),
            ));
        }
        let io = |e: std::io::Error| GitError::Other(format!("cannot update '{path}': {e}"));
        let base = std::fs::read(&file).map_err(io)?;
        // The diff is in Git's (LF) form; lines put back into a CRLF checkout get CRLF.
        if base.windows(2).any(|w| w == b"\r\n") {
            for line in current.hunks.iter_mut().flat_map(|h| h.lines.iter_mut()) {
                if line.kind == LineKind::Removed
                    && line.raw.ends_with(b"\n")
                    && !line.raw.ends_with(b"\r\n")
                {
                    line.raw.pop();
                    line.raw.extend_from_slice(b"\r\n");
                }
            }
        }
        // Reverting working-tree changes = "unstaging" them from the working tree's side.
        let content = apply_selection(&base, &current, selection, Direction::Unstage)?;
        write_atomically(&file, &content).map_err(io)
    }

    /// Restore whole files to their index version; untracked files go to the OS trash.
    pub fn discard_files(&self, paths: &[&str]) -> Result<(), GitError> {
        let index = self.git().index().map_err(|e| GitError::from_git2(&e))?;
        let (tracked, untracked): (Vec<&str>, Vec<&str>) = paths
            .iter()
            .partition(|p| index.get_path(Path::new(p), 0).is_some());
        if !untracked.is_empty() {
            let dir = self.workdir()?;
            let files: Vec<_> = untracked.iter().map(|p| dir.join(p)).collect();
            trash::delete_all(&files)
                .map_err(|e| GitError::Other(format!("cannot move to the trash: {e}")))?;
        }
        if tracked.is_empty() {
            return Ok(());
        }
        if crate::git_available() {
            return self.git_on_paths(&["checkout"], &tracked);
        }
        let mut opts = git2::build::CheckoutBuilder::new();
        opts.force();
        for p in &tracked {
            opts.path(p);
        }
        self.git()
            .checkout_index(None, Some(&mut opts))
            .map_err(|e| GitError::from_git2(&e))
    }
}

/// Replace `file` with `content` via a temporary file, keeping its permissions.
fn write_atomically(file: &Path, content: &[u8]) -> std::io::Result<()> {
    let perms = std::fs::metadata(file)?.permissions();
    let name = file
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = file.with_file_name(format!(".{name}.retrogit-tmp"));
    std::fs::write(&tmp, content)?;
    std::fs::set_permissions(&tmp, perms)?;
    std::fs::rename(&tmp, file)
}
