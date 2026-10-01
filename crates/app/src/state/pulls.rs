//! State of the Pull Requests tab (sub-project 4).

use gitcore::FileDiff;
use github::{
    LineComment, MergeMethod, Mergeable, PrDetail, PrFile, PrFilter, PrState, PrSummary, RepoMeta,
    ReviewEvent,
};

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
        checked: Vec<String>,
    },
    /// New line comment, queued in the pending review.
    LineComment {
        path: String,
        line: u32,
        side: github::DiffSide,
        /// The commented line, shown for context.
        quote: String,
        body: String,
    },
    Reply {
        comment_id: u64,
        body: String,
    },
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct PullsView {
    /// Repository the list and detail belong to.
    pub slug: Option<Slug>,
    pub filter: PrFilter,
    pub list: Vec<PrSummary>,
    pub loading: bool,
    /// The list must be (re)loaded when the tab is shown.
    pub stale: bool,
    pub selected: Option<u64>,
    pub detail: Option<PrDetail>,
    pub files: Option<Vec<PrFile>>,
    pub sub_tab: PullTab,
    /// File shown in the Files tab, its diff and syntax colors.
    pub file: Option<String>,
    pub file_diff: Option<FileDiff>,
    pub file_colors: crate::highlight::Colors,
    /// Line comments of the review being written (for `selected`).
    pub pending: Vec<LineComment>,
    /// Conversation comment being typed.
    pub comment: String,
    /// A change is being sent.
    pub busy: bool,
    pub dialog: Option<PullDialog>,
    pub meta: Option<RepoMeta>,
    /// Status bar note after the last change.
    pub note: Option<String>,
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
            self.pending.clear();
            self.comment.clear();
            self.sub_tab = PullTab::Conversation;
        }
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
        self.file_diff = Some(crate::pr_diff::parse_patch(&f.path, f.patch.as_deref()));
        self.file_colors = crate::highlight::Colors::NotRequested;
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
        "BLOCKED" => return Some(s::WHY_BLOCKED),
        "BEHIND" => return Some(s::WHY_BEHIND),
        _ => {}
    }
    if d.allowed_methods.is_empty() {
        return Some(s::WHY_NO_METHOD);
    }
    None
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
            Event::PullsLoaded { slug, filter, list } => {
                if p.slug.as_ref() == Some(&slug) && p.filter == filter {
                    p.list = list;
                    p.loading = false;
                }
            }
            Event::PullLoaded { slug, detail } => {
                if p.slug.as_ref() == Some(&slug) && p.selected == Some(detail.summary.number) {
                    // Keep the list in step with what the detail says.
                    if let Some(row) = p
                        .list
                        .iter_mut()
                        .find(|r| r.number == detail.summary.number)
                    {
                        *row = detail.summary.clone();
                    }
                    p.detail = Some(*detail);
                }
            }
            Event::PullFilesLoaded {
                slug,
                number,
                files,
            } => {
                if p.slug.as_ref() == Some(&slug) && p.selected == Some(number) {
                    p.files = Some(files);
                    // Same file still there: refresh its diff (new commits), else clear.
                    match p.file.clone() {
                        Some(path) if p.files.iter().flatten().any(|f| f.path == path) => {
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
                    p.stale = true;
                    p.note = Some(s::NOTE_PULL_CREATED.to_string());
                }
            }
            Event::PullActionDone { number, note } => {
                p.busy = false;
                p.note = Some(note);
                p.stale = true;
                if p.selected == Some(number) {
                    // The review went through with its line comments.
                    if matches!(p.dialog, Some(PullDialog::Review { .. })) {
                        p.pending.clear();
                    }
                    p.dialog = None;
                }
            }
            _ => {}
        }
    }

    /// Queue a line comment in the pending review (from the line comment dialog).
    pub fn queue_line_comment(&mut self, comment: LineComment) {
        self.pulls.pending.push(comment);
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
