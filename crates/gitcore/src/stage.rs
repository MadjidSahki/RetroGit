use std::collections::BTreeSet;
use std::path::Path;

use crate::diff::{FileDiff, LineKind, Side};
use crate::{GitError, Repo};

/// What to stage or unstage in one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selection {
    All,
    /// Indexes into `FileDiff::hunks`.
    Hunks(Vec<usize>),
    /// `(hunk index, line index within the hunk)`; only added/removed lines matter.
    Lines(Vec<(usize, usize)>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Stage,
    Unstage,
}

impl Selection {
    fn picks(&self, hunk: usize, line: usize) -> bool {
        match self {
            Selection::All => true,
            Selection::Hunks(h) => h.contains(&hunk),
            Selection::Lines(l) => l.contains(&(hunk, line)),
        }
    }
}

fn push_line(out: &mut Vec<u8>, line: &[u8]) {
    // Never glue two lines together when the previous one had no trailing newline.
    if !out.is_empty() && out.last() != Some(&b'\n') {
        out.push(b'\n');
    }
    out.extend_from_slice(line);
}

/// Compute the new index content of a file.
///
/// - `Stage`: `base` is the index content, `diff` the unstaged diff (index -> working tree).
/// - `Unstage`: `base` is the index content, `diff` the staged diff (HEAD -> index).
///
/// Unselected changes are left as they are in `base`; selected ones are applied (stage)
/// or reverted (unstage). Same result as `git add -p` / `git reset -p`.
pub fn apply_selection(
    base: &[u8],
    diff: &FileDiff,
    selection: &Selection,
    direction: Direction,
) -> Result<Vec<u8>, GitError> {
    let lines: Vec<&[u8]> = base.split_inclusive(|b| *b == b'\n').collect();
    let mut out = Vec::with_capacity(base.len());
    let mut cursor = 0usize; // number of base lines consumed
    // In `base`, the lines we walk are the diff's old side (stage) or new side (unstage).
    let (base_kind, other_kind) = match direction {
        Direction::Stage => (LineKind::Removed, LineKind::Added),
        Direction::Unstage => (LineKind::Added, LineKind::Removed),
    };
    let stale = || GitError::StaleSelection;
    for (h, hunk) in diff.hunks.iter().enumerate() {
        let (start, count) = match direction {
            Direction::Stage => (hunk.old_start as usize, hunk.old_lines),
            Direction::Unstage => (hunk.new_start as usize, hunk.new_lines),
        };
        // With a zero-line range, `start` is the line *after which* the hunk applies.
        let first = if count == 0 {
            start
        } else {
            start.saturating_sub(1)
        };
        if first < cursor || first > lines.len() {
            return Err(stale());
        }
        for l in &lines[cursor..first] {
            push_line(&mut out, l);
        }
        cursor = first;
        for (i, line) in hunk.lines.iter().enumerate() {
            if line.kind == LineKind::Context {
                let l = lines.get(cursor).ok_or_else(stale)?;
                push_line(&mut out, l);
                cursor += 1;
            } else if line.kind == base_kind {
                // Present in base: kept unless selected.
                let l = lines.get(cursor).ok_or_else(stale)?;
                if !selection.picks(h, i) {
                    push_line(&mut out, l);
                }
                cursor += 1;
            } else if line.kind == other_kind && selection.picks(h, i) {
                // Absent from base: inserted only if selected.
                push_line(&mut out, &line.raw);
            }
        }
    }
    for l in &lines[cursor.min(lines.len())..] {
        push_line(&mut out, l);
    }
    Ok(out)
}

impl Repo {
    /// Stage a selection of the unstaged changes of `path`.
    /// `shown` is the diff the user made the selection on (ignored for `Selection::All`).
    pub fn stage(
        &self,
        path: &str,
        selection: &Selection,
        shown: Option<&FileDiff>,
    ) -> Result<(), GitError> {
        self.change_index(path, selection, shown, Direction::Stage)
    }

    /// Unstage a selection of the staged changes of `path`.
    pub fn unstage(
        &self,
        path: &str,
        selection: &Selection,
        shown: Option<&FileDiff>,
    ) -> Result<(), GitError> {
        self.change_index(path, selection, shown, Direction::Unstage)
    }

    fn change_index(
        &self,
        path: &str,
        selection: &Selection,
        shown: Option<&FileDiff>,
        direction: Direction,
    ) -> Result<(), GitError> {
        let repo = self.git();
        let map = |e: git2::Error| GitError::from_git2(&e);
        let rel = Path::new(path);
        if *selection == Selection::All {
            return match direction {
                Direction::Stage => {
                    let mut index = repo.index().map_err(map)?;
                    if self.workdir()?.join(rel).symlink_metadata().is_ok() {
                        index.add_path(rel).map_err(map)?;
                    } else {
                        index.remove_path(rel).map_err(map)?;
                    }
                    index.write().map_err(map)
                }
                Direction::Unstage => {
                    let head = repo.head().ok().and_then(|h| h.peel_to_commit().ok());
                    let target = head.as_ref().map(|c| c.as_object());
                    repo.reset_default(target, [path]).map_err(map)
                }
            };
        }

        let side = match direction {
            Direction::Stage => Side::Unstaged,
            Direction::Unstage => Side::Staged,
        };
        let current = self.diff_file(path, side)?;
        if shown.is_some_and(|s| *s != current) {
            return Err(GitError::StaleSelection);
        }
        if current.binary {
            return Err(GitError::Unsupported(
                "binary files can only be staged as a whole".into(),
            ));
        }
        let mut index = repo.index().map_err(map)?;
        let existing = index.get_path(rel, 0);
        let is_deletion = match direction {
            Direction::Stage => self.workdir()?.join(rel).symlink_metadata().is_err(),
            Direction::Unstage => existing.is_none(),
        };
        if is_deletion {
            return Err(GitError::Unsupported(
                "deleted files can only be staged as a whole".into(),
            ));
        }
        let base = match &existing {
            Some(entry) => repo.find_blob(entry.id).map_err(map)?.content().to_vec(),
            None => Vec::new(),
        };
        let content = apply_selection(&base, &current, selection, direction)?;
        let entry = match existing {
            Some(e) => e,
            None => new_entry(path, self.file_mode(rel)),
        };
        index.add_frombuffer(&entry, &content).map_err(map)?;
        index.write().map_err(map)?;
        // Unstaging everything of a file that is new in the index leaves an empty entry:
        // drop it so the file goes back to "untracked".
        if direction == Direction::Unstage && content.is_empty() {
            let head_has = repo
                .head()
                .ok()
                .and_then(|h| h.peel_to_tree().ok())
                .is_some_and(|t| t.get_path(rel).is_ok());
            if !head_has {
                let mut index = repo.index().map_err(map)?;
                index.remove_path(rel).map_err(map)?;
                index.write().map_err(map)?;
            }
        }
        Ok(())
    }

    fn file_mode(&self, rel: &Path) -> u32 {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(dir) = self.workdir()
                && let Ok(meta) = std::fs::metadata(dir.join(rel))
                && meta.permissions().mode() & 0o111 != 0
            {
                return 0o100755;
            }
        }
        let _ = rel;
        0o100644
    }
}

fn new_entry(path: &str, mode: u32) -> git2::IndexEntry {
    git2::IndexEntry {
        ctime: git2::IndexTime::new(0, 0),
        mtime: git2::IndexTime::new(0, 0),
        dev: 0,
        ino: 0,
        mode,
        uid: 0,
        gid: 0,
        file_size: 0,
        id: git2::Oid::ZERO_SHA1,
        flags: 0,
        flags_extended: 0,
        path: path.as_bytes().to_vec(),
    }
}

/// Selected `(hunk, line)` pairs that are real changes (context lines are never selectable).
pub fn selectable_lines(diff: &FileDiff) -> BTreeSet<(usize, usize)> {
    diff.hunks
        .iter()
        .enumerate()
        .flat_map(|(h, hunk)| {
            hunk.lines
                .iter()
                .enumerate()
                .filter(|(_, l)| l.kind != LineKind::Context)
                .map(move |(i, _)| (h, i))
        })
        .collect()
}
