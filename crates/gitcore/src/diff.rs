use std::path::Path;

use crate::{GitError, Refusal, Repo};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Side {
    /// Index -> working tree.
    Unstaged,
    /// HEAD -> index.
    Staged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    Context,
    Added,
    Removed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: LineKind,
    pub old_no: Option<u32>,
    pub new_no: Option<u32>,
    /// Line content for display (lossy UTF-8), including its line ending when it has one.
    pub text: String,
    /// Exact bytes of the line (what staging writes: never lossy).
    pub raw: Vec<u8>,
    /// The line is the last of its file and has no trailing newline.
    pub no_newline_at_eof: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
    pub header: String,
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
    pub lines: Vec<DiffLine>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDiff {
    pub path: String,
    pub side: Side,
    pub binary: bool,
    pub hunks: Vec<Hunk>,
}

impl FileDiff {
    pub fn line_count(&self) -> usize {
        self.hunks.iter().map(|h| h.lines.len()).sum()
    }
}

impl Repo {
    /// Unified diff (3 context lines) of one file. Untracked files diff against empty content.
    pub fn diff_file(&self, path: &str, side: Side) -> Result<FileDiff, GitError> {
        let repo = self.git();
        let mut opts = git2::DiffOptions::new();
        opts.pathspec(path)
            .disable_pathspec_match(true)
            .include_untracked(true)
            .recurse_untracked_dirs(true)
            .show_untracked_content(true)
            .context_lines(3);
        let map = |e: git2::Error| GitError::from_git2(&e);
        let diff = match side {
            Side::Unstaged => repo
                .diff_index_to_workdir(None, Some(&mut opts))
                .map_err(map)?,
            Side::Staged => {
                let head_tree = repo.head().ok().and_then(|h| h.peel_to_tree().ok());
                repo.diff_tree_to_index(head_tree.as_ref(), None, Some(&mut opts))
                    .map_err(map)?
            }
        };
        file_diff_from(&diff, path, side)
    }

    pub(crate) fn workdir(&self) -> Result<&Path, GitError> {
        self.git()
            .workdir()
            .ok_or(GitError::Refused(Refusal::BareRepository))
    }
}

/// Convert a libgit2 diff (limited to one file) into our model.
pub(crate) fn file_diff_from(
    diff: &git2::Diff<'_>,
    path: &str,
    side: Side,
) -> Result<FileDiff, GitError> {
    let map = |e: git2::Error| GitError::from_git2(&e);
    let mut out = FileDiff {
        path: path.to_string(),
        side,
        binary: false,
        hunks: Vec::new(),
    };
    for idx in 0..diff.deltas().len() {
        let Some(patch) = git2::Patch::from_diff(diff, idx).map_err(map)? else {
            out.binary = true;
            continue;
        };
        if patch.delta().flags().is_binary() {
            out.binary = true;
            continue;
        }
        for h in 0..patch.num_hunks() {
            let (hunk, n) = patch.hunk(h).map_err(map)?;
            let mut lines = Vec::with_capacity(n);
            for l in 0..n {
                let line = patch.line_in_hunk(h, l).map_err(map)?;
                let kind = match line.origin() {
                    ' ' => LineKind::Context,
                    '+' => LineKind::Added,
                    '-' => LineKind::Removed,
                    // "\ No newline at end of file" markers: encoded in `text` already.
                    _ => continue,
                };
                let raw = line.content().to_vec();
                lines.push(DiffLine {
                    kind,
                    old_no: line.old_lineno(),
                    new_no: line.new_lineno(),
                    no_newline_at_eof: raw.last() != Some(&b'\n'),
                    text: String::from_utf8_lossy(&raw).into_owned(),
                    raw,
                });
            }
            out.hunks.push(Hunk {
                header: String::from_utf8_lossy(hunk.header())
                    .trim_end()
                    .to_string(),
                old_start: hunk.old_start(),
                old_lines: hunk.old_lines(),
                new_start: hunk.new_start(),
                new_lines: hunk.new_lines(),
                lines,
            });
        }
    }
    Ok(out)
}
