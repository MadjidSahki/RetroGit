//! Searching: text in the files of a version (`git grep`), commits (`git log`).

use std::sync::atomic::AtomicBool;

use crate::{GitError, LogEntry, RefKind, Repo};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrepMatch {
    pub path: String,
    /// 1-based.
    pub line: usize,
    /// The line, without its line ending.
    pub text: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GrepResult {
    pub matches: Vec<GrepMatch>,
    /// More matches exist than the limit.
    pub truncated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LogSearch {
    Message,
    Author,
    Hash,
    ChangedText,
}

/// A version to explore.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExploreRef {
    pub name: String,
    pub kind: RefKind,
}

/// Lines of `git grep -n --null <commit>`: `<commit>:<path>\0<line>\0<text>`.
pub fn parse_grep(out: &str, commit: &str) -> Vec<GrepMatch> {
    let prefix = format!("{commit}:");
    out.lines()
        .filter_map(|l| {
            let rest = l.strip_prefix(&prefix).unwrap_or(l);
            let mut parts = rest.splitn(3, '\0');
            let path = parts.next()?.to_string();
            let line = parts.next()?.parse().ok()?;
            let text = parts.next()?.trim_end_matches('\r').to_string();
            Some(GrepMatch { path, line, text })
        })
        .collect()
}

const LOG_FORMAT: &str = "--format=%x1e%H%x00%P%x00%an%x00%ae%x00%at%x00%s";

fn parse_log(out: &str) -> Vec<LogEntry> {
    out.split('\x1e')
        .filter(|r| !r.trim().is_empty())
        .map(|r| {
            let mut f = r.trim_end_matches('\n').splitn(6, '\0');
            let mut next = || f.next().unwrap_or("").to_string();
            let (id, parents, author, email, time, summary) =
                (next(), next(), next(), next(), next(), next());
            LogEntry {
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
            }
        })
        .collect()
}

impl Repo {
    /// Lines of the files of `rev` containing `text` (exact text; binary files skipped).
    /// `paths`: space-separated pathspecs (`*.rs src/`), empty for every file.
    pub fn grep(
        &self,
        rev: &str,
        text: &str,
        match_case: bool,
        paths: &str,
        limit: usize,
        cancel: &AtomicBool,
    ) -> Result<GrepResult, GitError> {
        let commit = self.commit_id(rev)?;
        let mut args = vec!["grep", "-F", "-n", "--null", "-I", "--full-name"];
        if !match_case {
            args.push("-i");
        }
        args.extend(["-e", text, &commit, "--"]);
        args.extend(paths.split_whitespace());
        let (out, truncated) = self.run_git_capped(&args, cancel, limit)?;
        // Exit 1 without output: no match.
        if !out.success && !out.text.is_empty() {
            return Err(GitError::Other(out.text));
        }
        Ok(GrepResult {
            matches: parse_grep(&out.stdout, &commit),
            truncated,
        })
    }

    /// Commits of every branch matching `query`, newest first, `limit` at most
    /// (`true`: there are more).
    pub fn search_log(
        &self,
        kind: LogSearch,
        query: &str,
        limit: usize,
        cancel: &AtomicBool,
    ) -> Result<(Vec<LogEntry>, bool), GitError> {
        let mut found = match kind {
            LogSearch::Hash => {
                let hex = query.len() >= 4 && query.bytes().all(|c| c.is_ascii_hexdigit());
                let id = hex.then(|| self.resolve(query)).flatten();
                (id.into_iter().map(|id| self.entry(&id)).collect(), false)
            }
            // An empty repository (HEAD not born yet): no commits to search.
            _ if self.resolve("HEAD").is_none() => (Vec::new(), false),
            _ => {
                let filter = match kind {
                    LogSearch::Message => format!("--grep={query}"),
                    LogSearch::Author => format!("--author={query}"),
                    _ => format!("-S{query}"),
                };
                let args = [
                    "log",
                    "--branches",
                    "--remotes",
                    "HEAD",
                    "-i",
                    "--fixed-strings",
                    LOG_FORMAT,
                    &filter,
                ];
                let (out, truncated) = self.run_git_capped(&args, cancel, limit)?;
                if !out.success {
                    return Err(GitError::Other(out.text));
                }
                (parse_log(&out.stdout), truncated)
            }
        };
        let labels = self.ref_labels()?;
        for e in &mut found.0 {
            e.refs = labels.get(&e.id).cloned().unwrap_or_default();
        }
        Ok(found)
    }

    fn entry(&self, id: &str) -> LogEntry {
        let c = git2::Oid::from_str(id)
            .ok()
            .and_then(|o| self.git().find_commit(o).ok());
        LogEntry {
            id: id.to_string(),
            short_id: id.chars().take(7).collect(),
            parents: c
                .as_ref()
                .map(|c| c.parent_ids().map(|p| p.to_string()).collect())
                .unwrap_or_default(),
            author: c
                .as_ref()
                .and_then(|c| c.author().name().ok().map(String::from))
                .unwrap_or_default(),
            email: c
                .as_ref()
                .and_then(|c| c.author().email().ok().map(String::from))
                .unwrap_or_default(),
            time: c.as_ref().map(|c| c.author().when().seconds()).unwrap_or(0),
            summary: c
                .as_ref()
                .and_then(|c| c.summary().ok().flatten().map(String::from))
                .unwrap_or_default(),
            refs: Vec::new(),
        }
    }

    /// HEAD, then local branches, remote branches and tags (alphabetical).
    pub fn explore_refs(&self) -> Vec<ExploreRef> {
        let mut out = vec![ExploreRef {
            name: "HEAD".into(),
            kind: RefKind::Head,
        }];
        let Ok(refs) = self.git().references() else {
            return out;
        };
        let mut found: Vec<ExploreRef> = Vec::new();
        for r in refs.flatten() {
            let Ok(name) = r.name() else { continue };
            let (kind, short) = if let Some(n) = name.strip_prefix("refs/heads/") {
                (RefKind::LocalBranch, n)
            } else if let Some(n) = name.strip_prefix("refs/remotes/") {
                if n.ends_with("/HEAD") {
                    continue;
                }
                (RefKind::RemoteBranch, n)
            } else if let Some(n) = name.strip_prefix("refs/tags/") {
                // A tag can point to a tree or a blob: nothing to explore as a version.
                if r.peel_to_commit().is_err() {
                    continue;
                }
                (RefKind::Tag, n)
            } else {
                continue;
            };
            found.push(ExploreRef {
                name: short.to_string(),
                kind,
            });
        }
        let rank = |k: RefKind| match k {
            RefKind::LocalBranch => 0,
            RefKind::RemoteBranch => 1,
            _ => 2,
        };
        found.sort_by(|a, b| (rank(a.kind), &a.name).cmp(&(rank(b.kind), &b.name)));
        out.extend(found);
        out
    }
}
