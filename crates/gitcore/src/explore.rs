//! Explore: the files of any version, blame and history of a file.

use std::sync::atomic::AtomicBool;

use crate::{GitError, LogEntry, Refusal, Repo};

/// Files larger than this are not loaded for display.
pub const MAX_FILE_BYTES: u64 = 5 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    File,
    Dir,
    Submodule,
    Symlink,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeEntry {
    /// `/`-separated path from the repository root.
    pub path: String,
    pub kind: EntryKind,
    /// Size in bytes (files and links; 0 for directories and submodules).
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileContent {
    Text(String),
    Binary,
    TooLarge(u64),
}

/// Consecutive lines of a file last changed by the same commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlameBlock {
    pub commit: String,
    pub author: String,
    /// Author time, seconds since the Unix epoch.
    pub time: i64,
    pub summary: String,
    /// First line (1-based) and number of lines, in the blamed version.
    pub start: usize,
    pub count: usize,
    /// Path and first line in `commit` (for "Blame the parent").
    pub orig_path: String,
    pub orig_start: usize,
    /// The commit is the first of the history (no parent to blame).
    pub boundary: bool,
    /// Text of the lines (without line endings).
    pub lines: Vec<String>,
    /// Parent commit and the file's path there, to blame the lines before this commit
    /// (`None`: the commit added them).
    pub previous: Option<(String, String)>,
}

/// A commit that changed a file, with the file's name in that commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileCommit {
    pub entry: LogEntry,
    pub path: String,
    pub renamed_from: Option<String>,
}

#[derive(Default)]
struct CommitInfo {
    author: String,
    time: i64,
    summary: String,
    boundary: bool,
    previous: Option<(String, String)>,
}

/// Undo git's C-style quoting of unusual paths (`"a\tb"`, `"\303\251"`).
pub(crate) fn unquote_path(s: &str) -> String {
    let Some(inner) = s.strip_prefix('"').and_then(|s| s.strip_suffix('"')) else {
        return s.to_string();
    };
    let mut out: Vec<u8> = Vec::new();
    let b = inner.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\' && i + 1 < b.len() {
            let c = b[i + 1];
            if c.is_ascii_digit() && i + 3 < b.len() {
                let oct = std::str::from_utf8(&b[i + 1..i + 4])
                    .ok()
                    .and_then(|o| u8::from_str_radix(o, 8).ok());
                if let Some(v) = oct {
                    out.push(v);
                    i += 4;
                    continue;
                }
            }
            out.push(match c {
                b'n' => b'\n',
                b't' => b'\t',
                b'r' => b'\r',
                other => other,
            });
            i += 2;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Blocks of `git blame --porcelain` output, in file order.
pub fn parse_blame_porcelain(text: &str) -> Vec<BlameBlock> {
    let mut infos: std::collections::HashMap<String, CommitInfo> = Default::default();
    let mut blocks: Vec<BlameBlock> = Vec::new();
    let mut current: Option<String> = None;
    for line in text.lines() {
        if let Some(text) = line.strip_prefix('\t') {
            if let Some(b) = blocks.last_mut() {
                b.lines.push(text.trim_end_matches('\r').to_string());
            }
            continue;
        }
        let mut parts = line.split(' ');
        let first = parts.next().unwrap_or("");
        let is_header = first.len() == 40 && first.bytes().all(|c| c.is_ascii_hexdigit());
        if is_header {
            let nums: Vec<usize> = parts.filter_map(|p| p.parse().ok()).collect();
            current = Some(first.to_string());
            if let [orig, fin, count] = nums[..] {
                blocks.push(BlameBlock {
                    commit: first.to_string(),
                    author: String::new(),
                    time: 0,
                    summary: String::new(),
                    start: fin,
                    count,
                    orig_path: String::new(),
                    orig_start: orig,
                    boundary: false,
                    lines: Vec::new(),
                    previous: None,
                });
            }
            continue;
        }
        let Some(commit) = &current else { continue };
        let info = infos.entry(commit.clone()).or_default();
        let (key, value) = line.split_once(' ').unwrap_or((line, ""));
        match key {
            "author" => info.author = value.to_string(),
            "author-time" => info.time = value.parse().unwrap_or(0),
            "summary" => info.summary = value.to_string(),
            "boundary" => info.boundary = true,
            "previous" => {
                if let Some((sha, path)) = value.split_once(' ') {
                    info.previous = Some((sha.to_string(), unquote_path(path)));
                }
            }
            "filename" => {
                if let Some(b) = blocks.last_mut()
                    && b.orig_path.is_empty()
                {
                    b.orig_path = unquote_path(value);
                }
            }
            _ => {}
        }
    }
    for b in &mut blocks {
        if let Some(i) = infos.get(&b.commit) {
            b.author = i.author.clone();
            b.time = i.time;
            b.summary = i.summary.clone();
            b.boundary = i.boundary;
            b.previous = i.previous.clone();
        }
    }
    blocks
}

/// Commits of `git log --follow --name-status -z` with the format of [`HISTORY_FORMAT`].
fn parse_file_history(text: &str) -> Vec<FileCommit> {
    let mut out = Vec::new();
    for record in text.split('\x1e').filter(|r| !r.is_empty()) {
        let mut fields = record.splitn(7, '\0');
        let mut next = || fields.next().unwrap_or("").to_string();
        let (id, parents, author, email, time, summary) =
            (next(), next(), next(), next(), next(), next());
        let rest = next();
        let tokens: Vec<&str> = rest
            .split('\0')
            .map(|t| t.trim_start_matches('\n'))
            .filter(|t| !t.is_empty())
            .collect();
        let (path, renamed_from) = match tokens.as_slice() {
            [status, old, new, ..] if status.starts_with('R') || status.starts_with('C') => {
                (new.to_string(), Some(old.to_string()))
            }
            [_, path, ..] => (path.to_string(), None),
            _ => continue,
        };
        out.push(FileCommit {
            entry: LogEntry {
                short_id: id.chars().take(7).collect(),
                parents: parents
                    .split(' ')
                    .filter(|p| !p.is_empty())
                    .map(String::from)
                    .collect(),
                author,
                email,
                time: time.parse().unwrap_or(0),
                summary,
                refs: Vec::new(),
                id,
            },
            path,
            renamed_from,
        });
    }
    out
}

const HISTORY_FORMAT: &str = "--format=%x1e%H%x00%P%x00%an%x00%ae%x00%at%x00%s";

impl Repo {
    /// Commit id of `rev` (branch, tag, `HEAD~2`...), if it names a commit.
    pub fn resolve(&self, rev: &str) -> Option<String> {
        self.git()
            .revparse_single(rev)
            .ok()?
            .peel_to_commit()
            .ok()
            .map(|c| c.id().to_string())
    }

    /// [`Repo::resolve`], or an error naming `rev`.
    pub(crate) fn commit_id(&self, rev: &str) -> Result<String, GitError> {
        self.resolve(rev)
            .ok_or_else(|| GitError::Refused(Refusal::UnknownRev(rev.to_string())))
    }

    fn tree_of(&self, rev: &str) -> Result<git2::Tree<'_>, GitError> {
        let map = |e: git2::Error| GitError::from_git2(&e);
        self.git()
            .revparse_single(rev)
            .map_err(map)?
            .peel_to_commit()
            .map_err(map)?
            .tree()
            .map_err(map)
    }

    /// Every file and directory of `rev`, sorted by path (empty before the first commit).
    pub fn tree(&self, rev: &str) -> Result<Vec<TreeEntry>, GitError> {
        if self.git().head().is_err() && rev == "HEAD" {
            return Ok(Vec::new());
        }
        let tree = self.tree_of(rev)?;
        let odb = self.git().odb().map_err(|e| GitError::from_git2(&e))?;
        let mut out = Vec::new();
        tree.walk(git2::TreeWalkMode::PreOrder, |dir, entry| {
            let path = format!("{dir}{}", entry.name().unwrap_or(""));
            let kind = match entry.kind() {
                Some(git2::ObjectType::Tree) => EntryKind::Dir,
                Some(git2::ObjectType::Commit) => EntryKind::Submodule,
                _ if entry.filemode() == 0o120_000 => EntryKind::Symlink,
                _ => EntryKind::File,
            };
            let size = match kind {
                EntryKind::File | EntryKind::Symlink => odb
                    .read_header(entry.id())
                    .map(|(s, _)| s as u64)
                    .unwrap_or(0),
                _ => 0,
            };
            out.push(TreeEntry { path, kind, size });
            git2::TreeWalkResult::Ok
        })
        .map_err(|e| GitError::from_git2(&e))?;
        out.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(out)
    }

    /// Content of `path` in `rev`.
    pub fn file_at(&self, rev: &str, path: &str) -> Result<FileContent, GitError> {
        let map = |e: git2::Error| GitError::from_git2(&e);
        let tree = self.tree_of(rev)?;
        let entry = tree.get_path(std::path::Path::new(path)).map_err(map)?;
        let blob = self.git().find_blob(entry.id()).map_err(map)?;
        let size = blob.size() as u64;
        if size > MAX_FILE_BYTES {
            return Ok(FileContent::TooLarge(size));
        }
        if blob.is_binary() {
            return Ok(FileContent::Binary);
        }
        Ok(FileContent::Text(
            String::from_utf8_lossy(blob.content()).into_owned(),
        ))
    }

    /// Who last changed each line of `path` in `rev`.
    pub fn blame(
        &self,
        rev: &str,
        path: &str,
        cancel: &AtomicBool,
    ) -> Result<Vec<BlameBlock>, GitError> {
        // A commit id, never a name: a ref named `--output=x` must not become an option.
        let commit = self.commit_id(rev)?;
        let out = self.run_git_cancel(&["blame", "--porcelain", &commit, "--", path], cancel)?;
        if !out.success {
            return Err(GitError::Other(out.text));
        }
        Ok(parse_blame_porcelain(&out.stdout))
    }

    /// The `limit` latest commits that changed `path` (as of `rev`), following renames.
    pub fn file_history(
        &self,
        rev: &str,
        path: &str,
        limit: usize,
        cancel: &AtomicBool,
    ) -> Result<Vec<FileCommit>, GitError> {
        let commit = self.commit_id(rev)?;
        let n = format!("-n{limit}");
        let out = self.run_git_cancel(
            &[
                "--literal-pathspecs",
                "log",
                "--follow",
                "--name-status",
                "-z",
                &n,
                HISTORY_FORMAT,
                &commit,
                "--",
                path,
            ],
            cancel,
        )?;
        if !out.success {
            return Err(GitError::Other(out.text));
        }
        Ok(parse_file_history(&out.stdout))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoted_paths_are_unquoted() {
        assert_eq!(unquote_path("plain.txt"), "plain.txt");
        assert_eq!(unquote_path("\"a\\tb\""), "a\tb");
        assert_eq!(unquote_path("\"caf\\303\\251.txt\""), "café.txt");
        assert_eq!(unquote_path("\"q\\\"uote\""), "q\"uote");
    }
}
