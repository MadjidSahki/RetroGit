#![allow(clippy::unwrap_used)]
//! Pull request additions in the UI (headless, mock worker).

use std::path::PathBuf;
use std::sync::Arc;

use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use gitcore::{Head, RepoSummary};
use github::{
    ChecksState, Client, DiffSide, MemoryAccounts, MergeMethod, Mergeable, PrDetail, PrFile,
    PrState, PrSummary, ReviewDecision, ReviewThread, Reviewer, ThreadComment, TokenProvider,
};
use retrogit::config::Config;
use retrogit::protocol::Event;
use retrogit::state::{AppState, LineSelection, PeopleKind, PullDialog, PullTab, Tab};
use retrogit::strings as s;
use retrogit::ui::Ctx;
use retrogit::worker::{WorkerDeps, WorkerHandle, spawn};

struct World {
    state: AppState,
    worker: WorkerHandle,
    highlighter: retrogit::highlight::Service,
    notices: std::sync::mpsc::Sender<retrogit::protocol::AppError>,
    /// The mock GitHub of the worker.
    server: mockito::ServerGuard,
}

fn slug() -> (String, String) {
    ("o".into(), "r".into())
}

fn world(draft: bool) -> World {
    world_with(draft, |_| {})
}

/// `world`, with the pull request detail changed by `edit` first.
fn world_with(draft: bool, edit: impl FnOnce(&mut PrDetail)) -> World {
    let server = mockito::Server::new();
    let worker = spawn(
        WorkerDeps {
            client: Client::with_bases(&server.url(), &server.url()),
            store: Arc::new(MemoryAccounts::default()),
            client_id: String::new(),
            commit_backend: gitcore::CommitBackend::Git2,
            tokens: TokenProvider::without_gh(),
            known_accounts: Vec::new(),
            repo_accounts: Default::default(),
        },
        || {},
    );
    let mut state = AppState::new(Config::default());
    state.apply(Event::RepoOpened(RepoSummary {
        name: "r".into(),
        path: PathBuf::from("/tmp/r"),
        head: Head::Branch("main".into()),
        origin_url: Some("https://github.com/o/r.git".into()),
        last_commit: None,
    }));
    state.tab = Tab::PullRequests;
    state.pulls.stale = false;
    let summary = PrSummary {
        number: 7,
        title: "Fix login".into(),
        url: String::new(),
        author: "bob".into(),
        head: "feat/login".into(),
        base: "main".into(),
        draft,
        state: PrState::Open,
        labels: vec![],
        checks: ChecksState::None,
        review_decision: ReviewDecision::None,
        updated_at: String::new(),
    };
    state.apply(Event::PullsLoaded {
        slug: slug(),
        filter: Default::default(),
        list: vec![summary.clone()],
        total: 1,
    });
    state.pulls.select(7);
    let thread = ReviewThread {
        id: "T1".into(),
        can_resolve: true,
        can_unresolve: false,
        path: "a.rs".into(),
        line: Some(3),
        original_line: Some(3),
        side: DiffSide::Right,
        start_line: Some(2),
        start_side: Some(DiffSide::Right),
        outdated: false,
        resolved: false,
        comments: vec![ThreadComment {
            id: 1,
            author: "carol".into(),
            author_id: Some(43),
            body: "Simpler:\n```suggestion\nlet bc = 5;\n```".into(),
            at: String::new(),
        }],
    };
    let mut detail = PrDetail {
        summary,
        body: String::new(),
        head_sha: "abc".into(),
        head_repo: Some(("o".into(), "r".into())),
        cross_repository: false,
        commits: vec![],
        commit_count: 0,
        comments_total: 0,
        reviews_total: 0,
        threads_total: 0,
        check_runs: vec![],
        timeline: vec![],
        threads: vec![thread],
        mergeable: Mergeable::Mergeable,
        merge_state: "CLEAN".into(),
        allowed_methods: vec![MergeMethod::Squash],
        viewer: "ada".into(),
        viewer_is_author: false,
        viewer_can_write: true,
        repo_labels: vec![],
        id: "PR_7".into(),
        viewer_can_update: true,
        reviewers: vec![
            Reviewer {
                login: "carol".into(),
                state: None,
            },
            Reviewer {
                login: "dan".into(),
                state: Some(github::ReviewState::Approved),
            },
        ],
        assignees: vec!["bob".into()],
        viewer_can_triage: true,
        team_reviewers: vec!["o/core".into()],
    };
    edit(&mut detail);
    state.apply(Event::PullLoaded {
        slug: slug(),
        detail: Box::new(detail),
    });
    state.apply(Event::PullFilesLoaded {
        slug: slug(),
        number: 7,
        files: vec![PrFile {
            path: "a.rs".into(),
            previous_path: None,
            status: "modified".into(),
            additions: 2,
            deletions: 1,
            patch: Some("@@ -1,3 +1,4 @@\n a\n-b\n+let b = 2;\n+let c = 3;\n d".into()),
        }],
    });
    let (notices, _rx) = std::sync::mpsc::channel();
    World {
        state,
        worker,
        highlighter: retrogit::highlight::Service::start(|_| {}),
        notices,
        server,
    }
}

fn harness(w: World) -> Harness<'static, World> {
    Harness::builder()
        .with_size(egui::vec2(1200.0, 750.0))
        .build_ui_state(
            |ui, w: &mut World| {
                let ctx = ui.ctx().clone();
                let mut cx = Ctx {
                    state: &mut w.state,
                    worker: &w.worker,
                    highlighter: &w.highlighter,
                    notices: &w.notices,
                };
                retrogit::ui::main_window::show(ui, &mut cx);
                retrogit::ui::pull_dialogs::show(&ctx, &mut cx);
            },
            w,
        )
}

#[test]
fn the_header_shows_people_and_offers_edits() {
    let mut h = harness(world(true));
    h.run();
    assert!(h.query_by_label("@carol").is_some(), "requested reviewer");
    assert!(h.query_by_label_contains("@dan").is_some(), "reviewed");
    assert!(h.query_by_label("@bob").is_some(), "assignee");
    assert!(h.query_by_label("@o/core").is_some(), "requested team");
    assert!(h.query_by_label(s::EDIT_REVIEWERS).is_some());
    assert!(h.query_by_label(s::EDIT_ASSIGNEES).is_some());
    assert!(h.query_by_label(s::READY_FOR_REVIEW).is_some());
    h.get_by_label(s::EDIT_PULL).click();
    h.run();
    assert!(
        matches!(&h.state().state.pulls.dialog, Some(PullDialog::EditPull { title, .. }) if title == "Fix login")
    );
}

#[test]
fn without_triage_people_cannot_be_edited() {
    let mut h = harness(world_with(true, |d| d.viewer_can_triage = false));
    h.run();
    assert!(
        h.query_by_label(s::EDIT_PULL).is_some(),
        "the author may edit"
    );
    assert!(h.query_by_label(s::EDIT_REVIEWERS).is_none());
    assert!(h.query_by_label(s::EDIT_ASSIGNEES).is_none());
}

#[test]
fn only_teams_requested_are_shown_and_kept_out_of_the_people_dialog() {
    let mut h = harness(world_with(false, |d| d.reviewers.clear()));
    h.run();
    assert!(h.query_by_label("@o/core").is_some());
    assert!(h.query_by_label(s::NOBODY).is_none(), "a team is somebody");
    h.get_by_label(s::EDIT_REVIEWERS).click();
    h.run();
    assert!(matches!(
        &h.state().state.pulls.dialog,
        Some(PullDialog::People { kind: PeopleKind::Reviewers, checked, .. }) if checked.is_empty()
    ));
}

#[test]
fn a_suggestion_across_both_sides_cannot_be_applied() {
    let mut w = world_with(false, |d| d.threads[0].start_side = Some(DiffSide::Left));
    w.state.pulls.sub_tab = PullTab::Files;
    w.state.pulls.open_file("a.rs");
    let mut h = harness(w);
    h.run();
    assert!(
        h.query_by_label_contains("Simpler:").is_some(),
        "the comment"
    );
    assert!(h.query_by_label(s::APPLY_SUGGESTION).is_none());
}

#[test]
fn editing_reviewers_opens_the_people_dialog() {
    let mut h = harness(world(false));
    h.run();
    assert!(h.query_by_label(s::CONVERT_TO_DRAFT).is_some());
    h.get_by_label(s::EDIT_REVIEWERS).click();
    h.run();
    assert!(matches!(
        &h.state().state.pulls.dialog,
        Some(PullDialog::People { kind: PeopleKind::Reviewers, checked, .. }) if checked == &["carol".to_string()]
    ));
}

#[test]
fn suggestions_show_a_mini_diff_and_apply_needs_a_checkout() {
    let mut w = world(false);
    w.state.pulls.sub_tab = PullTab::Files;
    w.state.pulls.open_file("a.rs");
    let mut h = harness(w);
    h.run();
    assert!(
        h.query_by_label_contains("let bc = 5;").is_some(),
        "proposed line"
    );
    assert!(h.query_by_label(s::APPLY_SUGGESTION).is_some());
    assert!(
        h.query_by_label(s::WHY_CHECKOUT_FIRST).is_some(),
        "on main, not feat/login"
    );
    assert!(
        h.query_by_label_contains("lines 2-3").is_some(),
        "range thread"
    );
}

#[test]
fn typing_in_the_people_filter_searches_github() {
    let mut h = harness(world(false));
    h.run();
    h.get_by_label(s::EDIT_ASSIGNEES).click();
    h.run();
    h.get_by_role(egui::accesskit::Role::TextInput).focus();
    h.run();
    h.get_by_role(egui::accesskit::Role::TextInput)
        .type_text("car");
    h.run();
    h.run();
    assert_eq!(
        h.state().state.pulls.assignable_query.as_deref(),
        Some("car")
    );
}

fn click_right_of(h: &Harness<'static, World>, text: &str, button: egui::PointerButton) {
    let rect = h.get_by_label_contains(text).rect();
    let pos = egui::pos2(rect.right() + 100.0, rect.center().y);
    h.hover_at(pos);
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton {
            pos,
            button,
            pressed,
            modifiers: egui::Modifiers::NONE,
        });
    }
}

fn files_world() -> Harness<'static, World> {
    let mut w = world(false);
    w.state.pulls.sub_tab = PullTab::Files;
    w.state.pulls.open_file("a.rs");
    let mut h = harness(w);
    h.run();
    h
}

#[test]
fn clicking_right_of_a_diff_line_text_selects_the_line() {
    let mut h = files_world();
    click_right_of(&h, "3 + let c = 3;", egui::PointerButton::Primary);
    h.run();
    assert_eq!(
        h.state().state.pulls.selection,
        Some(LineSelection {
            hunk: 0,
            from: 3,
            to: 3
        })
    );
}

#[test]
fn right_clicking_right_of_a_diff_line_text_offers_a_comment() {
    let mut h = files_world();
    assert!(h.query_by_label(s::ADD_COMMENT).is_none());
    click_right_of(&h, "3 + let c = 3;", egui::PointerButton::Secondary);
    h.run();
    assert!(h.query_by_label(s::ADD_COMMENT).is_some());
}

/// Width of the test window: comment rows must fit in it.
const WINDOW_RIGHT: f32 = 1200.0;

/// A comment far wider than the window, on one line, ending with `ENDWORD`.
fn long_comment() -> String {
    let mut body = "lorem ipsum dolor sit amet ".repeat(8);
    body.push_str("ENDWORD");
    body
}

/// The Files tab of `a.rs`, its comment (on line 3 only) replaced by `body`.
fn files_world_with_comment(body: &str) -> Harness<'static, World> {
    let body = body.to_string();
    let mut w = world_with(false, move |d| {
        d.threads[0].start_line = None;
        d.threads[0].comments[0].body = body;
    });
    w.state.pulls.sub_tab = PullTab::Files;
    w.state.pulls.open_file("a.rs");
    let mut h = harness(w);
    h.run();
    h
}

fn text_of(h: &Harness<'static, World>, containing: &str) -> String {
    let node = h.get_by_label_contains(containing);
    let node = node.accesskit_node();
    node.value().or_else(|| node.label()).unwrap_or_default()
}

/// Rects of the labels containing `text`.
fn rects_of(h: &Harness<'static, World>, text: &str) -> Vec<egui::Rect> {
    h.query_all_by_label_contains(text)
        .map(|n| n.rect())
        .collect()
}

fn click_at(h: &mut Harness<'static, World>, pos: egui::Pos2, button: egui::PointerButton) {
    h.hover_at(pos);
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton {
            pos,
            button,
            pressed,
            modifiers: egui::Modifiers::NONE,
        });
    }
    h.run();
}

/// Left-click the comment row containing `text`.
fn click_comment(h: &mut Harness<'static, World>, text: &str) {
    let pos = h.get_by_label_contains(text).rect().center();
    click_at(h, pos, egui::PointerButton::Primary);
}

#[test]
fn a_long_comment_is_cut_to_the_visible_width() {
    let h = files_world_with_comment(&long_comment());
    let text = text_of(&h, "> carol:");
    assert!(text.trim_end().ends_with(s::ELLIPSIS), "{text:?}");
    assert!(text.starts_with(s::EXPAND_MARK), "{text:?}");
    assert!(h.query_by_label_contains("ENDWORD").is_none(), "cut");
    let rect = h.get_by_label_contains("> carol:").rect();
    assert!(rect.right() <= WINDOW_RIGHT, "{rect:?}");
}

#[test]
fn a_short_comment_has_no_marker() {
    let h = files_world_with_comment("Looks good");
    let text = text_of(&h, "> carol: Looks good");
    assert!(!text.contains(s::EXPAND_MARK), "{text:?}");
    assert!(!text.contains(s::ELLIPSIS), "{text:?}");
}

#[test]
fn clicking_a_long_comment_shows_it_whole_then_cuts_it_again() {
    let mut h = files_world_with_comment(&long_comment());
    click_comment(&mut h, "> carol:");
    let text = text_of(&h, "> carol:");
    assert!(text.starts_with(s::COLLAPSE_MARK), "{text:?}");
    assert!(!text.contains(s::ELLIPSIS), "{text:?}");
    assert!(h.query_by_label_contains("ENDWORD").is_some(), "last word");
    let rows = rects_of(&h, "lorem");
    assert!(rows.len() >= 3, "wrapped on several rows: {rows:?}");
    for r in &rows {
        assert!(r.right() <= WINDOW_RIGHT, "{r:?}");
    }
    click_comment(&mut h, "> carol:");
    assert!(h.query_by_label_contains("ENDWORD").is_none(), "cut again");
    assert_eq!(rects_of(&h, "lorem").len(), 1, "continuation rows gone");
    assert!(text_of(&h, "> carol:").starts_with(s::EXPAND_MARK));
}

#[test]
fn clicking_a_multi_line_comment_shows_all_of_it() {
    let mut h = files_world();
    assert!(h.query_by_label_contains("```suggestion").is_none());
    assert!(text_of(&h, "> carol: Simpler:").starts_with(s::EXPAND_MARK));
    click_comment(&mut h, "> carol: Simpler:");
    assert!(
        h.query_by_label_contains("```suggestion").is_some(),
        "full comment under its first line"
    );
}

#[test]
fn selecting_another_pull_request_forgets_expanded_comments() {
    let mut h = files_world_with_comment(&long_comment());
    click_comment(&mut h, "> carol:");
    assert!(!h.state().state.pulls.expanded.is_empty());
    h.state_mut().state.pulls.select(8);
    assert!(h.state().state.pulls.expanded.is_empty());
}

#[test]
fn an_expanded_comment_still_offers_reply() {
    let mut h = files_world_with_comment(&long_comment());
    click_comment(&mut h, "> carol:");
    let pos = h.get_by_label_contains("ENDWORD").rect().center();
    click_at(&mut h, pos, egui::PointerButton::Secondary);
    assert!(
        h.query_by_label(s::REPLY).is_some(),
        "on a continuation row"
    );
    h.key_press(egui::Key::Escape);
    h.run();
    let pos = h.get_by_label_contains("> carol:").rect().center();
    click_at(&mut h, pos, egui::PointerButton::Secondary);
    assert!(h.query_by_label(s::REPLY).is_some(), "on the first row");
    assert!(
        h.query_by_label_contains("ENDWORD").is_some(),
        "a right-click does not collapse"
    );
}

#[test]
fn a_multi_line_pending_comment_expands_on_click() {
    let mut w = world(false);
    w.state.pulls.sub_tab = PullTab::Files;
    w.state.pulls.open_file("a.rs");
    w.state.queue_line_comment(
        7,
        github::LineComment {
            path: "a.rs".into(),
            line: 4,
            side: DiffSide::Right,
            start: None,
            body: "First line\nsecond line here".into(),
        },
    );
    let mut h = harness(w);
    h.run();
    assert!(h.query_by_label_contains("second line here").is_none());
    assert!(text_of(&h, "First line").starts_with(s::EXPAND_MARK));
    click_comment(&mut h, "First line");
    assert!(h.query_by_label_contains("second line here").is_some());
    assert!(text_of(&h, "First line").starts_with(s::COLLAPSE_MARK));
    click_comment(&mut h, "First line");
    assert!(h.query_by_label_contains("second line here").is_none());
}

#[test]
fn comments_wrap_on_words_and_break_long_ones() {
    use retrogit::ui::pull_detail::wrap_text;
    assert_eq!(wrap_text("aaa bbb ccc", 7), ["aaa bbb", "ccc"]);
    assert_eq!(wrap_text("abcdefg", 7), ["abcdefg"], "exactly the limit");
    assert_eq!(wrap_text("abcdefgh", 7), ["abcdefg", "h"]);
    assert_eq!(wrap_text("abcdefghij", 4), ["abcd", "efgh", "ij"]);
    assert_eq!(wrap_text("aa bbbbbbbbbb", 4), ["aa", "bbbb", "bbbb", "bb"]);
    assert_eq!(wrap_text("one\n\ntwo\n", 10), ["one", "", "two"]);
    assert_eq!(wrap_text("  indented", 20), ["  indented"], "kept");
    for row in wrap_text(&long_comment(), 33) {
        assert!(row.chars().count() <= 33, "{row:?}");
    }
}

#[test]
fn a_cut_line_ends_with_an_ellipsis_within_the_limit() {
    use retrogit::ui::pull_detail::clip_line;
    assert_eq!(clip_line("short", 10), ("short".to_string(), false));
    assert_eq!(
        clip_line("abcdefghij", 10),
        ("abcdefghij".to_string(), false)
    );
    assert_eq!(
        clip_line("abcdefghijk", 10),
        ("abcdefg...".to_string(), true)
    );
}

#[test]
fn expanded_comments_get_continuation_rows() {
    use retrogit::state::CommentKey;
    use retrogit::ui::pull_detail::{FileRow, file_rows};
    let w = world(false);
    let p = &w.state.pulls;
    let d = p.detail.as_ref().unwrap();
    let diff = retrogit::pr_diff::parse_patch(
        "a.rs",
        Some("@@ -1,3 +1,4 @@\n a\n-b\n+let b = 2;\n+let c = 3;\n d"),
    );
    let pending = vec![github::LineComment {
        path: "a.rs".into(),
        line: 4,
        side: DiffSide::Right,
        start: None,
        body: "one\ntwo\nthree".into(),
    }];
    let none = std::collections::HashSet::new();
    let collapsed = file_rows(&diff, &d.threads, &pending, &none, 200);
    assert!(
        !collapsed
            .iter()
            .any(|r| matches!(r, FileRow::CommentMore(..) | FileRow::PendingMore(..)))
    );
    let expanded = [CommentKey::Thread("T1".into(), 0), CommentKey::Pending(0)]
        .into_iter()
        .collect();
    let rows = file_rows(&diff, &d.threads, &pending, &expanded, 200);
    let more = |r: &&FileRow| matches!(r, FileRow::CommentMore(0, 0, _));
    // "Simpler:", "```suggestion", "let bc = 5;", "```": 3 rows under the first.
    assert_eq!(rows.iter().filter(more).count(), 3, "{rows:?}");
    let at = rows
        .iter()
        .position(|r| *r == FileRow::Comment(0, 0))
        .unwrap();
    assert_eq!(rows[at + 1], FileRow::CommentMore(0, 0, 1));
    let at = rows.iter().position(|r| *r == FileRow::Pending(0)).unwrap();
    assert_eq!(
        rows[at + 1..at + 3],
        [FileRow::PendingMore(0, 1), FileRow::PendingMore(0, 2)]
    );
}

fn titled(line: Option<u32>, start: Option<u32>, side: DiffSide) -> ReviewThread {
    ReviewThread {
        id: "T".into(),
        can_resolve: false,
        can_unresolve: false,
        path: "src/a.rs".into(),
        line,
        original_line: Some(9),
        side,
        start_line: start,
        start_side: start.map(|_| side),
        outdated: false,
        resolved: false,
        comments: Vec::new(),
    }
}

#[test]
fn conversation_titles_show_line_ranges_old_side_and_outdated() {
    use retrogit::ui::pull_detail::thread_title;
    assert_eq!(
        thread_title(&titled(Some(3), None, DiffSide::Right)),
        "src/a.rs:3"
    );
    assert_eq!(
        thread_title(&titled(Some(3), Some(3), DiffSide::Right)),
        "src/a.rs:3",
        "a range of one line"
    );
    assert_eq!(
        thread_title(&titled(Some(5), Some(2), DiffSide::Right)),
        "src/a.rs lines 2-5"
    );
    assert_eq!(
        thread_title(&titled(Some(4), None, DiffSide::Left)),
        "src/a.rs (old):4"
    );
    assert_eq!(
        thread_title(&titled(Some(5), Some(2), DiffSide::Left)),
        "src/a.rs (old) lines 2-5"
    );
    let mut mixed = titled(Some(5), Some(2), DiffSide::Right);
    mixed.start_side = Some(DiffSide::Left);
    assert_eq!(thread_title(&mixed), "src/a.rs old 2 - new 5");
    assert_eq!(
        retrogit::ui::pull_detail::thread_range(&mixed),
        Some((5, 5)),
        "old and new line numbers do not make a range"
    );
    let mut gone = titled(None, None, DiffSide::Right);
    gone.outdated = true;
    assert_eq!(thread_title(&gone), "src/a.rs:9 (outdated)");
    let mut moved = titled(Some(3), None, DiffSide::Right);
    moved.outdated = true;
    assert_eq!(
        thread_title(&moved),
        "src/a.rs:3",
        "still placed: not outdated"
    );
    let mut done = titled(Some(3), None, DiffSide::Right);
    done.resolved = true;
    assert_eq!(thread_title(&done), "src/a.rs:3 (resolved)");
}

/// Right-click line 3 of `a.rs`, Add comment..., type "Nice".
fn line_comment_open(h: &mut Harness<'static, World>) {
    click_right_of(h, "3 + let c = 3;", egui::PointerButton::Secondary);
    h.run();
    h.get_by_label(s::ADD_COMMENT).click();
    h.run();
    match h.state_mut().state.pulls.dialog.as_mut() {
        Some(PullDialog::LineComment {
            number,
            head_sha,
            body,
            ..
        }) => {
            assert_eq!((*number, head_sha.as_str()), (7, "abc"), "kept at opening");
            *body = "Nice".into();
        }
        other => panic!("{other:?}"),
    }
    h.run();
}

/// Another pull request is selected while the window is open.
fn select_another(h: &mut Harness<'static, World>) {
    h.state_mut().state.pulls.select(8);
    assert!(h.state().state.pulls.detail.is_none());
    h.run();
}

#[test]
fn a_line_comment_goes_to_the_pull_request_it_was_opened_on() {
    let mut w = world(false);
    w.state.pulls.sub_tab = PullTab::Files;
    w.state.pulls.open_file("a.rs");
    w.server
        .mock("GET", "/user")
        .with_body(r#"{"login":"ada","name":null}"#)
        .create();
    w.server
        .mock("GET", "/user/orgs")
        .match_query(mockito::Matcher::Any)
        .with_body(r#"[{"login":"o"}]"#)
        .create();
    let posted = w
        .server
        .mock("POST", "/repos/o/r/pulls/7/comments")
        .match_body(mockito::Matcher::PartialJson(
            serde_json::json!({ "commit_id": "abc", "body": "Nice", "line": 3 }),
        ))
        .with_status(201)
        .with_body("{}")
        .create();
    let wrong = w
        .server
        .mock("POST", "/repos/o/r/pulls/8/comments")
        .expect(0)
        .create();
    w.worker
        .send(retrogit::protocol::Command::SavePat("ghp_pat".into()));
    let mut h = harness(w);
    h.run();
    line_comment_open(&mut h);
    select_another(&mut h);
    assert!(
        !h.get_by_label(s::ADD_SINGLE_COMMENT)
            .accesskit_node()
            .is_disabled()
    );
    h.get_by_label(s::ADD_SINGLE_COMMENT).click();
    h.run();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    let done = loop {
        assert!(std::time::Instant::now() < deadline, "comment not sent");
        if let Ok(ev @ Event::PullActionDone { number: 7, .. }) = h
            .state()
            .worker
            .events
            .recv_timeout(std::time::Duration::from_millis(100))
        {
            break ev;
        }
    };
    posted.assert();
    wrong.assert();
    h.state_mut().state.apply(done);
    assert!(h.state().state.pulls.dialog.is_none(), "closed once posted");
}

#[test]
fn a_line_comment_is_queued_on_the_pull_request_it_was_opened_on() {
    let mut h = files_world();
    line_comment_open(&mut h);
    select_another(&mut h);
    h.get_by_label(s::ADD_TO_REVIEW).click();
    h.run();
    let pending = &h.state().state.pulls.pending;
    assert_eq!(pending.get(&7).map(Vec::len), Some(1), "{pending:?}");
    assert!(!pending.contains_key(&8));
}

#[test]
fn add_single_comment_is_disabled_without_a_commit() {
    let mut h = files_world();
    line_comment_open(&mut h);
    if let Some(PullDialog::LineComment { head_sha, .. }) =
        h.state_mut().state.pulls.dialog.as_mut()
    {
        head_sha.clear();
    }
    h.run();
    assert!(
        h.get_by_label(s::ADD_SINGLE_COMMENT)
            .accesskit_node()
            .is_disabled()
    );
    assert!(
        !h.get_by_label(s::ADD_TO_REVIEW)
            .accesskit_node()
            .is_disabled()
    );
    h.get_by_label(s::ADD_SINGLE_COMMENT).click();
    h.run();
    assert!(
        h.state().state.pulls.pending.is_empty(),
        "never queued instead"
    );
    assert!(h.state().state.pulls.dialog.is_some());
}

#[test]
fn a_closed_pull_request_still_offers_resolve_in_files() {
    let mut w = world(false);
    {
        let p = &mut w.state.pulls;
        let mut d = (**p.detail.as_ref().unwrap()).clone();
        d.summary.state = PrState::Closed;
        p.detail = Some(Arc::new(d));
        p.sub_tab = PullTab::Files;
        p.open_file("a.rs");
    }
    let mut h = harness(w);
    h.run();
    // Right of the text (the row ends near the window edge).
    let rect = h.get_by_label_contains("> carol: Simpler:").rect();
    let pos = egui::pos2(rect.right() + 30.0, rect.center().y);
    h.hover_at(pos);
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Secondary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        });
    }
    h.run();
    assert!(h.query_by_label(s::RESOLVE).is_some());
    assert!(h.query_by_label(s::REPLY).is_none(), "reply: open only");
}

#[test]
fn an_edit_is_saved_into_the_pull_request_it_was_opened_on() {
    let mut w = world(false);
    w.server
        .mock("GET", "/user")
        .with_body(r#"{"login":"ada","name":null}"#)
        .create();
    w.server
        .mock("GET", "/user/orgs")
        .match_query(mockito::Matcher::Any)
        .with_body(r#"[{"login":"o"}]"#)
        .create();
    let saved = w
        .server
        .mock("PATCH", "/repos/o/r/pulls/7")
        .match_body(mockito::Matcher::PartialJson(
            serde_json::json!({ "title": "New title" }),
        ))
        .with_body("{}")
        .create();
    let wrong = w
        .server
        .mock("PATCH", "/repos/o/r/pulls/8")
        .expect(0)
        .create();
    w.worker
        .send(retrogit::protocol::Command::SavePat("ghp_pat".into()));
    let mut h = harness(w);
    h.run();
    h.get_by_label(s::EDIT_PULL).click();
    h.run();
    match h.state_mut().state.pulls.dialog.as_mut() {
        Some(PullDialog::EditPull { number, title, .. }) => {
            assert_eq!(*number, 7, "kept at opening");
            *title = "New title".into();
        }
        other => panic!("{other:?}"),
    }
    // A notification click selects another pull request while the window is open.
    h.state_mut().state.pulls.select(8);
    h.run();
    h.get_by_label(s::SAVE).click();
    h.run();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while !saved.matched() {
        assert!(std::time::Instant::now() < deadline, "edit not saved");
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    wrong.assert();
}
