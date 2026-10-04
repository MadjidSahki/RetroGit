//! State of the Pull Requests tab (sub-project 4).

use gitcore::FileDiff;
use github::{
    LineComment, MergeMethod, Mergeable, PrDetail, PrFile, PrFilter, PrState, PrSummary, RepoMeta,
    ReviewEvent,
};

use std::collections::HashMap;
use std::sync::Arc;

use super::AppState;
use crate::protocol::{Event, Slug};
use crate::strings as s;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PullTab {
    #[default]
    Conversation,
    Commits,
    Files,
    Checks,
}

/// Dialogs of the Pull Requests tab (at most one at a time).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PullDialog {
    Review {
        event: ReviewEvent,
        body: String,
    },
    Merge {
        method: MergeMethod,
        title: String,
        message: String,
        delete_branch: bool,
    },
    Create {
        title: String,
        body: String,
        base: String,
        draft: bool,
        labels: Vec<String>,
        publish: bool,
    },
    Labels {
        /// Labels when the window opened.
        old: Vec<String>,
        checked: Vec<String>,
    },
    /// New line comment, queued in the pending review.
    LineComment {
        path: String,
        line: u32,
        side: github::DiffSide,
        /// First line of a comment on several lines (same side).
        start: Option<u32>,
        /// The commented line, shown for context.
        quote: String,
        body: String,
    },
    Reply {
        comment_id: u64,
        body: String,
    },
    /// Title and description of the pull request.
    EditPull {
        title: String,
        body: String,
    },
    /// Reviewers or assignees: who is checked, and the search filter.
    People {
        kind: super::PeopleKind,
        checked: Vec<String>,
        filter: String,
    },
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct PullsView {
    /// Repository the list and detail belong to.
    pub slug: Option<Slug>,
    pub filter: PrFilter,
    pub list: Vec<PrSummary>,
    /// Pull requests matching the filter (GitHub sends at most 50 of them).
    pub total: u32,
    pub loading: bool,
    /// The list must be (re)loaded when the tab is shown.
    pub stale: bool,
    pub selected: Option<u64>,
    /// Shared: the UI takes a cheap handle every frame instead of copying it.
    pub detail: Option<Arc<PrDetail>>,
    pub files: Option<Arc<Vec<PrFile>>>,
    pub sub_tab: PullTab,
    /// File shown in the Files tab, its diff and syntax colors.
    pub file: Option<String>,
    pub file_diff: Option<FileDiff>,
    pub file_colors: crate::highlight::Colors,
    /// Line comments of the reviews being written, by pull request number.
    pub pending: HashMap<u64, Vec<LineComment>>,
    /// Conversation comment being typed.
    pub comment: String,
    /// `comment` was sent: cleared once GitHub accepted it, kept after an error.
    pub comment_sent: bool,
    /// A change is being sent.
    pub busy: bool,
    pub dialog: Option<PullDialog>,
    pub meta: Option<RepoMeta>,
    /// Status bar note after the last change.
    pub note: Option<String>,
    /// The selected pull request must be loaded (selected from outside the list).
    pub load_selected: bool,
    /// Pull request to show once this repository is open (from a notification).
    pub open_after_switch: Option<(Slug, u64)>,
    /// Lines selected in the Files tab (comment on a range, suggest a change).
    pub selection: Option<super::LineSelection>,
    /// People who can be reviewers or assignees (for the People dialog).
    pub assignable: Vec<String>,
    /// When the shown detail was loaded (pull requests with running checks are reloaded).
    pub loaded_at: Option<std::time::Instant>,
    /// Search sent for `assignable` (GitHub returns at most 100 people per search).
    pub assignable_query: Option<String>,
    /// Pull request created in this session, until GitHub's list has it (its search lags).
    pub created: Option<u64>,
    /// The detail of this pull request failed to load, and why.
    pub load_error: Option<(u64, String)>,
}

impl PullsView {
    /// Fresh view for `slug` (or none): the list loads when the tab is shown.
    pub fn for_repo(slug: Option<Slug>) -> PullsView {
        PullsView {
            slug,
            stale: true,
            ..PullsView::default()
        }
    }

    pub fn select(&mut self, number: u64) {
        if self.selected != Some(number) {
            self.selected = Some(number);
            self.detail = None;
            self.files = None;
            self.file = None;
            self.file_diff = None;
            self.file_colors = crate::highlight::Colors::NotRequested;
            self.selection = None;
            self.load_error = None;
            self.comment.clear();
            self.comment_sent = false;
            self.sub_tab = PullTab::Conversation;
        }
    }

    /// The comment to send, if any; it stays in the box until GitHub accepted it.
    pub fn send_comment(&mut self) -> Option<String> {
        if self.comment.trim().is_empty() {
            return None;
        }
        self.busy = true;
        self.comment_sent = true;
        Some(self.comment.clone())
    }

    /// Show `path` of the selected pull request in the Files tab.
    pub fn open_file(&mut self, path: &str) {
        let Some(f) = self
            .files
            .as_ref()
            .and_then(|fs| fs.iter().find(|f| f.path == path))
        else {
            return;
        };
        self.file = Some(path.to_string());
        self.selection = None;
        self.file_diff = Some(crate::pr_diff::parse_patch(&f.path, f.patch.as_deref()));
        self.file_colors = crate::highlight::Colors::NotRequested;
    }

    /// Line comments waiting in the review of the selected pull request.
    pub fn selected_pending(&self) -> &[LineComment] {
        self.selected
            .and_then(|n| self.pending.get(&n))
            .map_or(&[], Vec::as_slice)
    }

    /// Line comments waiting in every pull request of this repository.
    pub fn pending_total(&self) -> usize {
        self.pending.values().map(Vec::len).sum()
    }

    /// Drop the `i`th pending line comment of the selected pull request.
    pub fn drop_pending(&mut self, i: usize) {
        let Some(n) = self.selected else {
            return;
        };
        if let Some(list) = self.pending.get_mut(&n)
            && i < list.len()
        {
            list.remove(i);
            if list.is_empty() {
                self.pending.remove(&n);
            }
        }
    }

    /// The selected pull request is loaded again: forget why it failed last time.
    pub fn reload_selected(&mut self) -> Option<u64> {
        self.load_error = None;
        self.selected
    }

    pub fn summary(&self, number: u64) -> Option<&PrSummary> {
        self.list.iter().find(|p| p.number == number)
    }
}

/// Why the Merge button is disabled, if it is.
pub fn merge_disabled_reason(d: &PrDetail) -> Option<&'static str> {
    if d.summary.state != PrState::Open {
        return Some(s::WHY_NOT_OPEN);
    }
    if !d.viewer_can_write {
        return Some(s::WHY_NO_PERMISSION);
    }
    if d.summary.draft {
        return Some(s::WHY_DRAFT);
    }
    if d.mergeable == Mergeable::Conflicting || d.merge_state == "DIRTY" {
        return Some(s::WHY_CONFLICTS);
    }
    match d.merge_state.as_str() {
        // GitHub says BLOCKED while required checks are still running.
        "BLOCKED" if d.summary.checks == github::ChecksState::Pending => {
            return Some(s::WHY_CHECKS_RUNNING);
        }
        "BLOCKED" => return Some(s::WHY_BLOCKED),
        "BEHIND" => return Some(s::WHY_BEHIND),
        _ => {}
    }
    if d.allowed_methods.is_empty() {
        return Some(s::WHY_NO_METHOD);
    }
    None
}

/// How often a pull request whose checks are running is reloaded.
pub const CHECKS_REFRESH: std::time::Duration = std::time::Duration::from_secs(30);

/// The shown pull request has checks running and was loaded `CHECKS_REFRESH` ago or more:
/// reload it (its merge state follows the checks).
pub fn needs_auto_refresh(
    d: &PrDetail,
    loaded_at: std::time::Instant,
    now: std::time::Instant,
) -> bool {
    d.summary.state == PrState::Open
        && d.summary.checks == github::ChecksState::Pending
        && now.duration_since(loaded_at) >= CHECKS_REFRESH
}

/// Review kinds the viewer may submit (GitHub refuses approving one's own pull request).
pub fn review_events_allowed(d: &PrDetail) -> Vec<ReviewEvent> {
    if d.summary.state != PrState::Open {
        return Vec::new();
    }
    if d.viewer_is_author {
        vec![ReviewEvent::Comment]
    } else {
        vec![
            ReviewEvent::Comment,
            ReviewEvent::Approve,
            ReviewEvent::RequestChanges,
        ]
    }
}

/// Default title and message of the merge commit, like github.com.
pub fn merge_defaults(d: &PrDetail, method: MergeMethod) -> (String, String) {
    let n = d.summary.number;
    match method {
        MergeMethod::Squash => {
            let message = d
                .commits
                .iter()
                .map(|c| format!("* {}", c.headline))
                .collect::<Vec<_>>()
                .join("\n");
            (format!("{} (#{n})", d.summary.title), message)
        }
        MergeMethod::Merge => {
            let from = match &d.head_repo {
                Some((owner, _)) => format!("{owner}/{}", d.summary.head),
                None => d.summary.head.clone(),
            };
            (
                format!("Merge pull request #{n} from {from}"),
                d.summary.title.clone(),
            )
        }
        MergeMethod::Rebase => (String::new(), String::new()),
    }
}

/// The method preselected in the merge dialog: squash, else merge, else rebase.
pub fn default_merge_method(d: &PrDetail) -> Option<MergeMethod> {
    [MergeMethod::Squash, MergeMethod::Merge, MergeMethod::Rebase]
        .into_iter()
        .find(|m| d.allowed_methods.contains(m))
}

/// Title proposed for a new pull request: the last commit's summary, else the branch name.
pub fn prefill_title(last_commit: Option<&str>, branch: &str) -> String {
    match last_commit.map(str::trim).filter(|t| !t.is_empty()) {
        Some(t) => t.to_string(),
        None => {
            let name = branch.rsplit('/').next().unwrap_or(branch);
            let words = name.replace(['-', '_'], " ");
            let mut c = words.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                None => String::new(),
            }
        }
    }
}

impl AppState {
    /// github.com repository of the open repo, if any.
    pub fn github_slug(&self) -> Option<Slug> {
        self.current
            .as_ref()?
            .origin_url
            .as_deref()
            .and_then(gitcore::parse_github_slug)
    }

    pub(super) fn apply_pulls(&mut self, event: Event) {
        let p = &mut self.pulls;
        match event {
            Event::PullsLoaded {
                slug,
                filter,
                list,
                total,
            } => {
                if p.slug.as_ref() == Some(&slug) && p.filter == filter {
                    let old = std::mem::replace(&mut p.list, list);
                    p.total = total;
                    p.loading = false;
                    if let Some(n) = p.created {
                        if p.list.iter().any(|r| r.number == n) {
                            p.created = None;
                        } else if p.selected == Some(n)
                            && let Some(row) = old.into_iter().find(|r| r.number == n)
                        {
                            // GitHub's search does not list it yet: keep it.
                            p.list.insert(0, row);
                        }
                    }
                }
            }
            Event::PullLoaded { slug, detail } => {
                if p.slug.as_ref() == Some(&slug) && p.selected == Some(detail.summary.number) {
                    if p.created == Some(detail.summary.number)
                        && detail.summary.state != github::PrState::Open
                    {
                        // Merged or closed since: lists no longer keep it.
                        p.created = None;
                    }
                    // Keep the list in step with what the detail says.
                    if let Some(row) = p
                        .list
                        .iter_mut()
                        .find(|r| r.number == detail.summary.number)
                    {
                        *row = detail.summary.clone();
                    } else if p.created == Some(detail.summary.number)
                        && matches!(p.filter, PrFilter::Open | PrFilter::Mine)
                    {
                        // Just created: open and ours, before GitHub's search lists it.
                        p.list.insert(0, detail.summary.clone());
                    }
                    p.load_error = None;
                    p.detail = Some(Arc::new(*detail));
                    p.loaded_at = Some(std::time::Instant::now());
                }
            }
            Event::PullFilesLoaded {
                slug,
                number,
                files,
            } => {
                if p.slug.as_ref() == Some(&slug) && p.selected == Some(number) {
                    p.files = Some(Arc::new(files));
                    p.selection = None;
                    // Same file still there: refresh its diff (new commits), else clear.
                    match p.file.clone() {
                        Some(path)
                            if p.files
                                .iter()
                                .flat_map(|f| f.iter())
                                .any(|f| f.path == path) =>
                        {
                            p.open_file(&path)
                        }
                        _ => {
                            p.file = None;
                            p.file_diff = None;
                        }
                    }
                }
            }
            Event::RepoMetaLoaded { slug, meta } => {
                if p.slug.as_ref() == Some(&slug) {
                    if let Some(PullDialog::Create { base, .. }) = p.dialog.as_mut()
                        && base.is_empty()
                    {
                        *base = meta.default_branch.clone();
                    }
                    p.meta = Some(meta);
                }
            }
            Event::PullCreated { slug, number } => {
                if p.slug.as_ref() == Some(&slug) {
                    p.busy = false;
                    p.dialog = None;
                    p.select(number);
                    p.created = Some(number);
                    p.stale = true;
                    p.note = Some(s::NOTE_PULL_CREATED.to_string());
                }
            }
            Event::AssignableLoaded { slug, users } => {
                if p.slug.as_ref() == Some(&slug) {
                    p.assignable = users;
                }
            }
            Event::PullActionDone { number, note } => {
                p.busy = false;
                p.note = Some(note);
                p.stale = true;
                if p.selected == Some(number) {
                    // The review went through with its line comments.
                    if matches!(p.dialog, Some(PullDialog::Review { .. })) {
                        p.pending.remove(&number);
                    }
                    if p.comment_sent {
                        p.comment.clear();
                        p.comment_sent = false;
                    }
                    p.dialog = None;
                }
            }
            _ => {}
        }
    }

    /// Queue a line comment in the pending review (from the line comment dialog).
    pub fn queue_line_comment(&mut self, comment: LineComment) {
        if let Some(n) = self.pulls.selected {
            self.pulls.pending.entry(n).or_default().push(comment);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_from_commits_or_branch_names() {
        assert_eq!(prefill_title(Some("Fix login\n"), "x"), "Fix login");
        assert_eq!(prefill_title(None, "feat/add-tarif_v2"), "Add tarif v2");
        assert_eq!(prefill_title(Some("  "), "main"), "Main");
        assert_eq!(prefill_title(None, ""), "");
    }
}
