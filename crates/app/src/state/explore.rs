//! Explore tab: the version explored, its files, the open file (content, blame, history),
//! and the search in files. Requests are made by `needs` and answered by the explore service.

use std::collections::BTreeSet;
use std::path::PathBuf;

use gitcore::{
    BlameBlock, EntryKind, ExploreRef, FileCommit, FileContent, FileDiff, GrepResult, LogEntry,
    LogSearch, TreeEntry,
};

use super::AppState;
use crate::highlight::Colors;
use crate::protocol::{AppError, ExploreRequest, ExploreResult, Severity};
use crate::strings as s;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FileView {
    #[default]
    Content,
    Blame,
    History,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SearchView {
    pub text: String,
    pub match_case: bool,
    pub paths: String,
    pub running: bool,
    pub result: Option<GrepResult>,
}

/// The Search in files window (what is typed, before Search).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SearchForm {
    pub text: String,
    pub match_case: bool,
    pub paths: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExploreView {
    /// Version explored ("HEAD", a branch, a tag or a commit id) and its commit.
    pub rev: String,
    pub commit: Option<String>,
    pub refs: Vec<ExploreRef>,
    pub entries: Vec<TreeEntry>,
    pub open_dirs: BTreeSet<String>,
    pub filter: String,
    pub file: Option<String>,
    pub view: FileView,
    pub content: Option<FileContent>,
    /// The content as a one-hunk diff, for syntax colors.
    pub code: Option<FileDiff>,
    pub colors: Colors,
    /// Line to show once the content is there (1-based, from a search result).
    pub goto_line: Option<usize>,
    pub blame: Option<Vec<BlameBlock>>,
    /// The blamed text as a diff (for colors) and its colors.
    pub blame_code: Option<FileDiff>,
    pub blame_colors: Colors,
    /// (rev, path) of earlier blames: "Blame the parent" pushes, Back pops.
    pub blame_stack: Vec<(String, String)>,
    pub history: Option<Vec<FileCommit>>,
    /// Commit selected in the file history, and its diff of the file.
    pub history_selected: Option<String>,
    pub history_diff: Option<FileDiff>,
    pub history_colors: Colors,
    pub search: Option<SearchView>,
    /// The Search in files window, when open.
    pub search_form: Option<SearchForm>,
    /// Why the view shown could not be loaded (in place of "Loading...").
    pub load_error: Option<String>,
    /// The version changed: the open file waits for the new tree.
    tree_pending: bool,
    /// Ask for the tree again (the repository changed: HEAD or a branch may have moved).
    reload_tree: bool,
    asked: Vec<ExploreRequest>,
    /// Rows of the tree for (commit, open folders, filter).
    rows_cache: Option<(RowsKey, Vec<TreeRow>)>,
}

type RowsKey = (Option<String>, BTreeSet<String>, String);

impl Default for ExploreView {
    fn default() -> Self {
        ExploreView {
            rev: "HEAD".into(),
            commit: None,
            refs: Vec::new(),
            entries: Vec::new(),
            open_dirs: BTreeSet::new(),
            filter: String::new(),
            file: None,
            view: FileView::Content,
            content: None,
            code: None,
            colors: Colors::NotRequested,
            goto_line: None,
            blame: None,
            blame_code: None,
            blame_colors: Colors::NotRequested,
            blame_stack: Vec::new(),
            history: None,
            history_selected: None,
            history_diff: None,
            history_colors: Colors::NotRequested,
            search: None,
            search_form: None,
            load_error: None,
            tree_pending: false,
            reload_tree: false,
            asked: Vec::new(),
            rows_cache: None,
        }
    }
}

/// One row of the file tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeRow {
    pub path: String,
    pub name: String,
    pub depth: usize,
    pub kind: EntryKind,
    pub open: bool,
}

fn parent_of(path: &str) -> &str {
    path.rsplit_once('/').map(|(p, _)| p).unwrap_or("")
}

fn name_of(path: &str) -> &str {
    path.rsplit_once('/').map(|(_, n)| n).unwrap_or(path)
}

/// Visible rows: folders first then files, alphabetical; children of open folders.
/// With a `filter`, files whose name contains it (any case) and their folders, all open.
pub fn tree_rows(entries: &[TreeEntry], open: &BTreeSet<String>, filter: &str) -> Vec<TreeRow> {
    let f = filter.trim().to_lowercase();
    let mut shown: Option<BTreeSet<&str>> = None;
    if !f.is_empty() {
        let mut keep = BTreeSet::new();
        for e in entries {
            if e.kind != EntryKind::Dir && name_of(&e.path).to_lowercase().contains(&f) {
                keep.insert(e.path.as_str());
                let mut p = parent_of(&e.path);
                while !p.is_empty() {
                    keep.insert(p);
                    p = parent_of(p);
                }
            }
        }
        shown = Some(keep);
    }
    let mut children: std::collections::BTreeMap<&str, Vec<&TreeEntry>> = Default::default();
    for e in entries {
        if shown.as_ref().is_none_or(|k| k.contains(e.path.as_str())) {
            children.entry(parent_of(&e.path)).or_default().push(e);
        }
    }
    for list in children.values_mut() {
        list.sort_by_cached_key(|e| (e.kind != EntryKind::Dir, name_of(&e.path).to_lowercase()));
    }
    let mut out = Vec::new();
    fn walk(
        dir: &str,
        depth: usize,
        children: &std::collections::BTreeMap<&str, Vec<&TreeEntry>>,
        open: &BTreeSet<String>,
        all_open: bool,
        out: &mut Vec<TreeRow>,
    ) {
        for e in children.get(dir).into_iter().flatten() {
            let is_open = e.kind == EntryKind::Dir && (all_open || open.contains(&e.path));
            out.push(TreeRow {
                path: e.path.clone(),
                name: name_of(&e.path).to_string(),
                depth,
                kind: e.kind,
                open: is_open,
            });
            if is_open {
                walk(&e.path, depth + 1, children, open, all_open, out);
            }
        }
    }
    walk("", 0, &children, open, shown.is_some(), &mut out);
    out
}

/// Age of each blame block from 0 (oldest commit) to 1 (newest), by rank of distinct times.
pub fn age_ranks(blocks: &[BlameBlock]) -> Vec<f32> {
    let times: BTreeSet<i64> = blocks.iter().map(|b| b.time).collect();
    let times: Vec<i64> = times.into_iter().collect();
    let n = times.len();
    blocks
        .iter()
        .map(|b| {
            if n < 2 {
                return 1.0;
            }
            let i = times.binary_search(&b.time).unwrap_or(0);
            i as f32 / (n - 1) as f32
        })
        .collect()
}

impl ExploreView {
    /// Commit the requests about files use (a name could be taken for an option by git,
    /// and the tree shown must match the files read).
    fn shown_commit(&self) -> Option<String> {
        self.commit.clone().filter(|c| !c.is_empty())
    }

    /// Requests for what is shown and not asked yet (marked as asked).
    pub fn needs(&mut self) -> Vec<ExploreRequest> {
        let mut want = Vec::new();
        if self.refs.is_empty() {
            want.push(ExploreRequest::Refs);
        }
        if self.commit.is_none() || self.reload_tree {
            want.push(ExploreRequest::Tree {
                rev: self.rev.clone(),
            });
        }
        if let (Some(path), Some(commit)) = (self.file.clone(), self.shown_commit())
            && !self.tree_pending
        {
            let text = matches!(self.content, Some(FileContent::Text(_)));
            match self.view {
                FileView::Content | FileView::Blame if self.content.is_none() => {
                    want.push(ExploreRequest::File {
                        rev: commit.clone(),
                        path: path.clone(),
                    })
                }
                FileView::Blame if self.blame.is_none() && text => {
                    if let Some((rev, path)) = self.blame_target() {
                        want.push(ExploreRequest::Blame { rev, path });
                    }
                }
                FileView::History if self.history.is_none() => {
                    want.push(ExploreRequest::FileHistory { rev: commit, path })
                }
                _ => {}
            }
        }
        if let (Some(commit), Some(path), None) = (
            self.history_selected.clone(),
            self.history_path(),
            &self.history_diff,
        ) && self.view == FileView::History
        {
            want.push(ExploreRequest::FileDiff { commit, path });
        }
        want.retain(|r| !self.asked.contains(r));
        self.asked.extend(want.iter().cloned());
        want
    }

    /// The repository changed (commit, checkout, pull...): ask again for the refs and the
    /// tree of the version; the open file is reloaded if the version moved.
    pub fn refresh(&mut self) {
        self.reload_refs();
        self.reload_tree = true;
        self.asked
            .retain(|r| !matches!(r, ExploreRequest::Tree { .. }));
    }

    fn reload_refs(&mut self) {
        self.refs.clear();
        self.asked.retain(|r| *r != ExploreRequest::Refs);
    }

    /// Rows of the file tree (rebuilt only when the version, the open folders or the filter
    /// change).
    pub fn rows(&mut self) -> &[TreeRow] {
        let key = (
            self.commit.clone(),
            self.open_dirs.clone(),
            self.filter.clone(),
        );
        if self.rows_cache.as_ref().is_none_or(|(k, _)| *k != key) {
            let rows = tree_rows(&self.entries, &self.open_dirs, &self.filter);
            self.rows_cache = Some((key, rows));
        }
        self.rows_cache
            .as_ref()
            .map(|(_, r)| r.as_slice())
            .unwrap_or(&[])
    }

    /// Path of the open file in the commit selected in its history (it may have been renamed).
    fn history_path(&self) -> Option<String> {
        let id = self.history_selected.as_ref()?;
        self.history
            .as_ref()?
            .iter()
            .find(|c| &c.entry.id == id)
            .map(|c| c.path.clone())
    }

    /// Explore another version (the open file stays if the new version has it).
    pub fn set_rev(&mut self, rev: &str) {
        if self.rev == rev {
            return;
        }
        let asked_refs = self.asked.contains(&ExploreRequest::Refs);
        let keep = ExploreView {
            rev: rev.to_string(),
            refs: std::mem::take(&mut self.refs),
            open_dirs: std::mem::take(&mut self.open_dirs),
            filter: std::mem::take(&mut self.filter),
            file: self.file.take(),
            view: self.view,
            tree_pending: true,
            asked: if asked_refs {
                vec![ExploreRequest::Refs]
            } else {
                Vec::new()
            },
            ..ExploreView::default()
        };
        *self = keep;
    }

    pub fn open_file(&mut self, path: &str) {
        if self.file.as_deref() == Some(path) {
            return;
        }
        self.file = Some(path.to_string());
        self.clear_file();
        let mut p = parent_of(path);
        while !p.is_empty() {
            self.open_dirs.insert(p.to_string());
            p = parent_of(p);
        }
    }

    fn clear_file(&mut self) {
        self.load_error = None;
        self.content = None;
        self.code = None;
        self.colors = Colors::NotRequested;
        self.goto_line = None;
        self.blame = None;
        self.blame_code = None;
        self.blame_colors = Colors::NotRequested;
        self.blame_stack.clear();
        self.history = None;
        self.history_selected = None;
        self.history_diff = None;
        self.history_colors = Colors::NotRequested;
        self.asked.retain(|r| {
            matches!(
                r,
                ExploreRequest::Refs | ExploreRequest::Tree { .. } | ExploreRequest::Grep { .. }
            )
        });
    }

    pub fn toggle_dir(&mut self, path: &str) {
        if !self.open_dirs.remove(path) {
            self.open_dirs.insert(path.to_string());
        }
    }

    /// What the Blame view shows: the top of the stack, else the open file.
    pub fn blame_target(&self) -> Option<(String, String)> {
        self.blame_stack
            .last()
            .cloned()
            .or_else(|| Some((self.shown_commit()?, self.file.clone()?)))
    }

    /// Blame the file as it was just before `block`'s commit (the parent and path git
    /// names, which follows a rename made by that commit).
    pub fn blame_parent(&mut self, block: &BlameBlock) {
        let Some(prev) = block.previous.clone() else {
            return;
        };
        self.blame_stack.push(prev);
        self.reset_blame();
    }

    pub fn can_go_back(&self) -> bool {
        !self.blame_stack.is_empty()
    }

    pub fn blame_back(&mut self) {
        if self.blame_stack.pop().is_some() {
            self.reset_blame();
        }
    }

    fn reset_blame(&mut self) {
        self.load_error = None;
        self.blame = None;
        self.blame_code = None;
        self.blame_colors = Colors::NotRequested;
        self.asked
            .retain(|r| !matches!(r, ExploreRequest::Blame { .. }));
    }

    pub fn select_history(&mut self, id: &str) {
        if self.history_selected.as_deref() != Some(id) {
            self.history_selected = Some(id.to_string());
            self.load_error = None;
            self.history_diff = None;
            self.history_colors = Colors::NotRequested;
            self.asked
                .retain(|r| !matches!(r, ExploreRequest::FileDiff { .. }));
        }
    }

    /// Start a search in the files of the explored version; returns the request.
    pub fn start_search(&mut self, text: &str, match_case: bool, paths: &str) -> ExploreRequest {
        self.search = Some(SearchView {
            text: text.to_string(),
            match_case,
            paths: paths.to_string(),
            running: true,
            result: None,
        });
        ExploreRequest::Grep {
            rev: self.rev.clone(),
            text: text.to_string(),
            match_case,
            paths: paths.to_string(),
        }
    }

    /// Open a search result: the file, scrolled to its line.
    pub fn open_match(&mut self, path: &str, line: usize) {
        self.open_file(path);
        self.view = FileView::Content;
        self.goto_line = Some(line);
    }

    /// Apply a result; `Err` is a message for the user.
    fn loaded(&mut self, result: ExploreResult) -> Result<(), String> {
        match result {
            ExploreResult::Refs(refs) => self.refs = refs,
            ExploreResult::Tree {
                rev,
                commit,
                entries,
            } if rev == self.rev => {
                let moved = self.commit.as_ref().is_some_and(|c| *c != commit);
                if moved {
                    // Same version name, new commit: what was read is out of date.
                    let file = self.file.take();
                    self.clear_file();
                    self.file = file;
                }
                if self.tree_pending || moved {
                    self.tree_pending = false;
                    if let Some(f) = &self.file
                        && !entries.iter().any(|e| &e.path == f)
                    {
                        self.file = None;
                    }
                }
                self.reload_tree = false;
                self.commit = Some(commit);
                self.entries = entries;
            }
            ExploreResult::File { rev, path, content }
                if Some(&rev) == self.commit.as_ref()
                    && self.file.as_deref() == Some(path.as_str()) =>
            {
                self.code = match &content {
                    FileContent::Text(t) => Some(super::text_as_diff(&path, t)),
                    _ => None,
                };
                self.colors = Colors::NotRequested;
                self.content = Some(content);
            }
            ExploreResult::Blame { rev, path, blocks }
                if self.blame_target() == Some((rev.clone(), path.clone())) =>
            {
                let text: String = blocks
                    .iter()
                    .flat_map(|b| b.lines.iter())
                    .map(|l| format!("{l}\n"))
                    .collect();
                self.blame_code = Some(super::text_as_diff(&path, &text));
                self.blame_colors = Colors::NotRequested;
                self.blame = Some(blocks);
            }
            ExploreResult::FileHistory { rev, path, commits }
                if Some(&rev) == self.commit.as_ref()
                    && self.file.as_deref() == Some(path.as_str()) =>
            {
                self.history = Some(commits);
            }
            ExploreResult::FileDiff { commit, path, diff }
                if self.history_selected.as_deref() == Some(commit.as_str())
                    && self.history_path().as_deref() == Some(path.as_str()) =>
            {
                self.history_colors = Colors::NotRequested;
                self.history_diff = Some(diff);
            }
            ExploreResult::Grep { rev, text, result } => {
                if let Some(sv) = self.search.as_mut()
                    && sv.text == text
                    && rev == self.rev
                {
                    sv.running = false;
                    sv.result = Some(result);
                }
            }
            ExploreResult::Failed { request, message } => {
                return self.failed(request, message).map_or(Ok(()), Err);
            }
            _ => {}
        }
        Ok(())
    }

    /// A request failed: what it was for gets the error if it is still shown. Returns the
    /// message to show in a box, if any.
    fn failed(&mut self, request: ExploreRequest, message: String) -> Option<String> {
        let commit = self.commit.clone();
        let file = self.file.clone();
        match request {
            ExploreRequest::Tree { rev } if rev == self.rev && rev != "HEAD" => {
                let gone = rev.clone();
                self.set_rev("HEAD");
                self.reload_refs();
                Some(s::ERR_EXPLORE_REF_GONE.replace("{rev}", &gone))
            }
            ExploreRequest::Tree { rev } if rev == self.rev => {
                self.reload_tree = false;
                self.load_error = Some(message);
                None
            }
            ExploreRequest::Grep { text, .. } => {
                let mine = self.search.as_ref().is_some_and(|sv| sv.text == text);
                if let Some(sv) = self.search.as_mut().filter(|_| mine) {
                    sv.running = false;
                }
                mine.then_some(message)
            }
            ExploreRequest::File { rev, path } | ExploreRequest::FileHistory { rev, path }
                if Some(&rev) == commit.as_ref() && Some(&path) == file.as_ref() =>
            {
                self.load_error = Some(message);
                None
            }
            ExploreRequest::Blame { rev, path }
                if self.blame_target() == Some((rev.clone(), path.clone())) =>
            {
                self.load_error = Some(message);
                None
            }
            ExploreRequest::FileDiff { commit, .. }
                if self.history_selected.as_deref() == Some(commit.as_str()) =>
            {
                self.load_error = Some(message);
                None
            }
            _ => None,
        }
    }
}

/// Filter of the History tab: commits found by message, author, hash or changed text.
#[derive(Debug, Clone, PartialEq)]
pub struct HistoryFilter {
    pub kind: LogSearch,
    pub query: String,
    pub running: bool,
    /// Commits found, and whether there are more than shown.
    pub results: Option<(Vec<LogEntry>, bool)>,
    /// The search running (the text field may be edited meanwhile).
    pub in_flight: Option<(LogSearch, String)>,
}

impl Default for HistoryFilter {
    fn default() -> Self {
        HistoryFilter {
            kind: LogSearch::Message,
            query: String::new(),
            running: false,
            results: None,
            in_flight: None,
        }
    }
}

impl super::HistoryView {
    /// The search to run (`None` when the text is empty).
    pub fn start_filter(&mut self) -> Option<ExploreRequest> {
        let f = &mut self.filter;
        let query = f.query.trim().to_string();
        if query.is_empty() {
            return None;
        }
        f.running = true;
        f.in_flight = Some((f.kind, query.clone()));
        Some(ExploreRequest::LogSearch {
            kind: f.kind,
            query,
        })
    }

    pub fn clear_filter(&mut self) {
        self.filter.running = false;
        self.filter.in_flight = None;
        self.filter.results = None;
    }

    /// Commits shown in the list: the filter's results, else the history.
    pub fn shown(&self) -> &[LogEntry] {
        match &self.filter.results {
            Some((found, _)) => found,
            None => &self.entries,
        }
    }
}

impl AppState {
    pub(super) fn explore_loaded(&mut self, repo: PathBuf, result: ExploreResult) {
        if self.current.as_ref().map(|c| &c.path) != Some(&repo) {
            return;
        }
        if let ExploreResult::LogSearch {
            kind,
            query,
            entries,
            truncated,
        } = result
        {
            let f = &mut self.history.filter;
            if f.in_flight.as_ref() == Some(&(kind, query)) {
                f.running = false;
                f.in_flight = None;
                f.results = Some((entries, truncated));
            }
            return;
        }
        if let ExploreResult::Failed {
            request: ExploreRequest::LogSearch { .. },
            message,
        } = &result
        {
            self.history.filter.running = false;
            self.history.filter.in_flight = None;
            self.messages
                .push_back(AppError::new(Severity::Warning, message));
            return;
        }
        if let Err(message) = self.explore.loaded(result) {
            self.messages
                .push_back(AppError::new(Severity::Warning, &message));
        }
    }

    /// Explore a version picked in History.
    pub fn browse_at(&mut self, commit: &str) {
        self.explore.set_rev(commit);
        self.tab = super::Tab::Explore;
    }
}
