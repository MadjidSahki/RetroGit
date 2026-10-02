#![allow(clippy::unwrap_used)]

use std::path::PathBuf;

use gitcore::{Head, RepoSummary};
use github::{
    ChecksState, Label, MergeMethod, Mergeable, PrCommit, PrDetail, PrFile, PrFilter, PrState,
    PrSummary, ReviewDecision, ReviewEvent,
};
use retrogit::config::Config;
use retrogit::highlight::{Colors, Target};
use retrogit::protocol::{AppError, Event, Op, Severity};
use retrogit::state::{
    AppState, PullTab, default_merge_method, merge_defaults, merge_disabled_reason,
    review_events_allowed,
};
use retrogit::strings as s;

fn slug() -> (String, String) {
    ("o".into(), "r".into())
}

fn opened(origin: Option<&str>) -> AppState {
    let mut st = AppState::new(Config::default());
    st.apply(Event::RepoOpened(RepoSummary {
        name: "r".into(),
        path: PathBuf::from("/tmp/r"),
        head: Head::Branch("main".into()),
        origin_url: origin.map(str::to_string),
        last_commit: None,
    }));
    st
}

fn pr(number: u64) -> PrSummary {
    PrSummary {
        number,
        title: format!("PR {number}"),
        url: String::new(),
        author: "bob".into(),
        head: "feat/x".into(),
        base: "main".into(),
        draft: false,
        state: PrState::Open,
        labels: vec![Label {
            name: "bug".into(),
            color: [0xd7, 0x3a, 0x4a],
            description: None,
        }],
        checks: ChecksState::Success,
        review_decision: ReviewDecision::None,
        updated_at: String::new(),
    }
}

fn detail(number: u64) -> PrDetail {
    PrDetail {
        summary: pr(number),
        body: String::new(),
        head_sha: "abc".into(),
        head_repo: Some(("o".into(), "r".into())),
        cross_repository: false,
        commits: vec![
            PrCommit {
                oid: "1".into(),
                short_oid: "1".into(),
                headline: "First".into(),
                author: "bob".into(),
                date: String::new(),
            },
            PrCommit {
                oid: "2".into(),
                short_oid: "2".into(),
                headline: "Second".into(),
                author: "bob".into(),
                date: String::new(),
            },
        ],
        commit_count: 2,
        check_runs: vec![],
        timeline: vec![],
        threads: vec![],
        mergeable: Mergeable::Mergeable,
        merge_state: "CLEAN".into(),
        allowed_methods: vec![MergeMethod::Merge, MergeMethod::Squash],
        viewer: "ada".into(),
        viewer_is_author: false,
        viewer_can_write: true,
        repo_labels: vec![],
        id: "PR_1".into(),
        viewer_can_update: true,
        reviewers: vec![],
        assignees: vec![],
    }
}

fn file(path: &str, patch: &str) -> PrFile {
    PrFile {
        path: path.into(),
        previous_path: None,
        status: "modified".into(),
        additions: 1,
        deletions: 0,
        patch: Some(patch.into()),
    }
}

#[test]
fn opening_a_github_repo_prepares_the_pull_requests_tab() {
    let st = opened(Some("git@github.com:o/r.git"));
    assert_eq!(st.github_slug(), Some(slug()));
    assert_eq!(st.pulls.slug, Some(slug()));
    assert!(st.pulls.stale, "loads when the tab is shown");
    let other = opened(Some("https://gitlab.com/o/r.git"));
    assert_eq!(other.github_slug(), None);
    assert_eq!(other.pulls.slug, None);
}

#[test]
fn results_for_another_repo_filter_or_selection_are_ignored() {
    let mut st = opened(Some("https://github.com/o/r"));
    st.pulls.loading = true;
    st.apply(Event::PullsLoaded {
        slug: ("x".into(), "y".into()),
        filter: PrFilter::Open,
        list: vec![pr(1)],
    });
    st.apply(Event::PullsLoaded {
        slug: slug(),
        filter: PrFilter::Closed,
        list: vec![pr(2)],
    });
    assert!(st.pulls.list.is_empty() && st.pulls.loading);
    st.apply(Event::PullsLoaded {
        slug: slug(),
        filter: PrFilter::Open,
        list: vec![pr(3)],
    });
    assert_eq!(st.pulls.list[0].number, 3);
    assert!(!st.pulls.loading);
    st.pulls.select(3);
    st.apply(Event::PullLoaded {
        slug: slug(),
        detail: Box::new(detail(9)),
    });
    assert!(st.pulls.detail.is_none());
    let mut fresh = detail(3);
    fresh.summary.title = "Renamed".into();
    st.apply(Event::PullLoaded {
        slug: slug(),
        detail: Box::new(fresh),
    });
    assert_eq!(st.pulls.detail.as_ref().unwrap().summary.number, 3);
    assert_eq!(
        st.pulls.list[0].title, "Renamed",
        "list row follows the detail"
    );
}

#[test]
fn selecting_another_pull_request_resets_the_detail_and_pending_review() {
    let mut st = opened(Some("https://github.com/o/r"));
    st.pulls.select(1);
    st.apply(Event::PullLoaded {
        slug: slug(),
        detail: Box::new(detail(1)),
    });
    st.pulls.sub_tab = PullTab::Files;
    st.queue_line_comment(github::LineComment {
        path: "a".into(),
        line: 1,
        side: github::DiffSide::Right,
        start: None,
        body: "x".into(),
    });
    st.pulls.select(1);
    assert_eq!(st.pulls.pending.len(), 1, "same pull request: kept");
    st.pulls.select(2);
    assert!(st.pulls.detail.is_none());
    assert!(st.pulls.pending.is_empty());
    assert_eq!(st.pulls.sub_tab, PullTab::Conversation);
}

#[test]
fn files_open_as_diffs_and_follow_new_commits() {
    let mut st = opened(Some("https://github.com/o/r"));
    st.pulls.select(1);
    st.apply(Event::PullFilesLoaded {
        slug: slug(),
        number: 1,
        files: vec![
            file("a.rs", "@@ -1 +1 @@\n-a\n+b"),
            file("b.rs", "@@ -0,0 +1 @@\n+x"),
        ],
    });
    st.pulls.open_file("a.rs");
    let d = st.pulls.file_diff.clone().unwrap();
    assert_eq!(d.path, "a.rs");
    assert_eq!(d.line_count(), 2);
    st.apply(Event::ColorsLoaded {
        target: Target::Pull,
        diff: d.clone(),
        colors: None,
    });
    assert_eq!(st.pulls.file_colors, Colors::Plain);
    // New commit: same file, new patch.
    st.apply(Event::PullFilesLoaded {
        slug: slug(),
        number: 1,
        files: vec![file("a.rs", "@@ -1 +1,2 @@\n-a\n+b\n+c")],
    });
    assert_eq!(st.pulls.file_diff.as_ref().unwrap().line_count(), 3);
    assert_eq!(st.pulls.file_colors, Colors::NotRequested);
    // The file disappeared from the pull request.
    st.apply(Event::PullFilesLoaded {
        slug: slug(),
        number: 1,
        files: vec![file("b.rs", "@@ -0,0 +1 @@\n+x")],
    });
    assert!(st.pulls.file.is_none() && st.pulls.file_diff.is_none());
}

#[test]
fn actions_end_busy_and_mark_the_list_stale() {
    let mut st = opened(Some("https://github.com/o/r"));
    st.pulls.stale = false;
    st.pulls.select(4);
    st.pulls.busy = true;
    st.pulls.dialog = Some(retrogit::state::PullDialog::Labels { checked: vec![] });
    st.apply(Event::PullActionDone {
        number: 4,
        note: s::NOTE_LABELS.into(),
    });
    assert!(!st.pulls.busy && st.pulls.stale && st.pulls.dialog.is_none());
    assert_eq!(st.pulls.note.as_deref(), Some(s::NOTE_LABELS));
    st.pulls.busy = true;
    st.apply(Event::Error {
        during: Op::PullAction,
        error: AppError::new(Severity::Warning, "no"),
    });
    assert!(!st.pulls.busy);
    st.pulls.loading = true;
    st.apply(Event::Error {
        during: Op::Pulls,
        error: AppError::new(Severity::Warning, "no"),
    });
    assert!(!st.pulls.loading);
    st.pulls.busy = true;
    st.apply(Event::Error {
        during: Op::Sync,
        error: AppError::new(Severity::Warning, "push failed"),
    });
    assert!(!st.pulls.busy, "publishing the branch failed: not created");
}

#[test]
fn pending_comments_are_cleared_only_when_the_review_went_through() {
    let mut st = opened(Some("https://github.com/o/r"));
    st.pulls.select(4);
    st.queue_line_comment(github::LineComment {
        path: "a".into(),
        line: 1,
        side: github::DiffSide::Right,
        start: None,
        body: "x".into(),
    });
    st.pulls.dialog = Some(retrogit::state::PullDialog::Review {
        event: ReviewEvent::Comment,
        body: String::new(),
    });
    st.pulls.busy = true;
    st.apply(Event::Error {
        during: Op::PullAction,
        error: AppError::new(Severity::Warning, "422"),
    });
    assert_eq!(st.pulls.pending.len(), 1, "failed: kept, dialog still open");
    assert!(st.pulls.dialog.is_some());
    st.apply(Event::PullActionDone {
        number: 4,
        note: s::NOTE_REVIEW_SENT.into(),
    });
    assert!(st.pulls.pending.is_empty() && st.pulls.dialog.is_none());
}

#[test]
fn a_conversation_comment_is_kept_until_github_accepts_it() {
    let mut st = opened(Some("https://github.com/o/r"));
    st.pulls.select(4);
    st.pulls.comment = "Looks good".into();
    assert_eq!(st.pulls.send_comment().as_deref(), Some("Looks good"));
    assert!(st.pulls.busy);
    st.apply(Event::Error {
        during: Op::PullAction,
        error: AppError::new(Severity::Warning, "no network"),
    });
    assert_eq!(st.pulls.comment, "Looks good", "not lost on failure");
    st.pulls.send_comment();
    st.apply(Event::PullActionDone {
        number: 4,
        note: s::NOTE_COMMENTED.into(),
    });
    assert!(st.pulls.comment.is_empty());
    st.pulls.comment = "  ".into();
    assert_eq!(st.pulls.send_comment(), None, "nothing to send");
}

#[test]
fn large_details_are_shared_not_copied() {
    let mut st = opened(Some("https://github.com/o/r"));
    st.pulls.select(3);
    st.apply(Event::PullLoaded {
        slug: slug(),
        detail: Box::new(detail(3)),
    });
    st.apply(Event::PullFilesLoaded {
        slug: slug(),
        number: 3,
        files: vec![file("a.rs", "@@ -1 +1 @@\n-a\n+b")],
    });
    let d = st.pulls.detail.clone().unwrap();
    let f = st.pulls.files.clone().unwrap();
    assert!(std::sync::Arc::ptr_eq(
        &d,
        st.pulls.detail.as_ref().unwrap()
    ));
    assert!(std::sync::Arc::ptr_eq(&f, st.pulls.files.as_ref().unwrap()));
}

#[test]
fn a_created_pull_request_is_selected() {
    let mut st = opened(Some("https://github.com/o/r"));
    st.pulls.busy = true;
    st.apply(Event::PullCreated {
        slug: slug(),
        number: 12,
    });
    assert_eq!(st.pulls.selected, Some(12));
    assert!(!st.pulls.busy && st.pulls.stale);
}

#[test]
fn the_create_dialog_gets_the_default_branch() {
    let mut st = opened(Some("https://github.com/o/r"));
    st.pulls.dialog = Some(retrogit::state::PullDialog::Create {
        title: "t".into(),
        body: String::new(),
        base: String::new(),
        draft: false,
        labels: vec![],
        publish: false,
    });
    st.apply(Event::RepoMetaLoaded {
        slug: slug(),
        meta: github::RepoMeta {
            default_branch: "develop".into(),
            labels: vec![],
        },
    });
    assert!(matches!(
        &st.pulls.dialog,
        Some(retrogit::state::PullDialog::Create { base, .. }) if base == "develop"
    ));
}

#[test]
fn merge_is_disabled_with_a_reason() {
    let ok = detail(1);
    assert_eq!(merge_disabled_reason(&ok), None);
    let with = |f: &dyn Fn(&mut PrDetail)| {
        let mut d = detail(1);
        f(&mut d);
        merge_disabled_reason(&d)
    };
    assert_eq!(with(&|d| d.summary.draft = true), Some(s::WHY_DRAFT));
    assert_eq!(
        with(&|d| d.summary.state = PrState::Merged),
        Some(s::WHY_NOT_OPEN)
    );
    assert_eq!(
        with(&|d| d.viewer_can_write = false),
        Some(s::WHY_NO_PERMISSION)
    );
    assert_eq!(
        with(&|d| d.mergeable = Mergeable::Conflicting),
        Some(s::WHY_CONFLICTS)
    );
    assert_eq!(
        with(&|d| d.merge_state = "BLOCKED".into()),
        Some(s::WHY_BLOCKED)
    );
    assert_eq!(
        with(&|d| d.merge_state = "BEHIND".into()),
        Some(s::WHY_BEHIND)
    );
    assert_eq!(with(&|d| d.allowed_methods.clear()), Some(s::WHY_NO_METHOD));
    assert_eq!(
        with(&|d| d.merge_state = "UNSTABLE".into()),
        None,
        "optional checks failing"
    );
    assert_eq!(
        with(&|d| d.mergeable = Mergeable::Unknown),
        None,
        "GitHub decides"
    );
}

#[test]
fn own_pull_requests_can_only_be_commented() {
    let mut d = detail(1);
    assert_eq!(
        review_events_allowed(&d),
        [
            ReviewEvent::Comment,
            ReviewEvent::Approve,
            ReviewEvent::RequestChanges
        ]
    );
    d.viewer_is_author = true;
    assert_eq!(review_events_allowed(&d), [ReviewEvent::Comment]);
    d.summary.state = PrState::Closed;
    assert!(review_events_allowed(&d).is_empty());
}

#[test]
fn merge_commit_defaults_follow_github() {
    let d = detail(7);
    assert_eq!(default_merge_method(&d), Some(MergeMethod::Squash));
    assert_eq!(
        merge_defaults(&d, MergeMethod::Squash),
        ("PR 7 (#7)".to_string(), "* First\n* Second".to_string())
    );
    assert_eq!(
        merge_defaults(&d, MergeMethod::Merge),
        (
            "Merge pull request #7 from o/feat/x".to_string(),
            "PR 7".to_string()
        )
    );
    let mut rebase_only = detail(7);
    rebase_only.allowed_methods = vec![MergeMethod::Rebase];
    assert_eq!(
        default_merge_method(&rebase_only),
        Some(MergeMethod::Rebase)
    );
}
