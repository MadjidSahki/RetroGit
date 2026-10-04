#![allow(clippy::unwrap_used)]
//! Pull request additions in the UI (headless, mock worker).

use std::path::PathBuf;
use std::sync::Arc;

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
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
}

fn slug() -> (String, String) {
    ("o".into(), "r".into())
}

fn world(draft: bool) -> World {
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
            body: "Simpler:\n```suggestion\nlet bc = 5;\n```".into(),
            at: String::new(),
        }],
    };
    state.apply(Event::PullLoaded {
        slug: slug(),
        detail: Box::new(PrDetail {
            summary,
            body: String::new(),
            head_sha: "abc".into(),
            head_repo: Some(("o".into(), "r".into())),
            cross_repository: false,
            commits: vec![],
            commit_count: 0,
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
        }),
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
    assert!(h.query_by_label(s::READY_FOR_REVIEW).is_some());
    h.get_by_label(s::EDIT_PULL).click();
    h.run();
    assert!(
        matches!(&h.state().state.pulls.dialog, Some(PullDialog::EditPull { title, .. }) if title == "Fix login")
    );
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

#[test]
fn hovering_a_multi_line_comment_shows_all_of_it() {
    let mut h = files_world();
    assert!(h.query_by_label_contains("```suggestion").is_none());
    let rect = h.get_by_label_contains("> carol: Simpler:").rect();
    // Right of the text (the row ends near the window edge).
    h.hover_at(egui::pos2(rect.right() + 30.0, rect.center().y));
    for _ in 0..10 {
        h.run();
    }
    assert!(
        h.query_by_label_contains("```suggestion").is_some(),
        "full comment in a tooltip"
    );
}

#[test]
fn hovering_a_multi_line_pending_comment_shows_all_of_it() {
    let mut w = world(false);
    w.state.pulls.sub_tab = PullTab::Files;
    w.state.pulls.open_file("a.rs");
    w.state.queue_line_comment(github::LineComment {
        path: "a.rs".into(),
        line: 4,
        side: DiffSide::Right,
        start: None,
        body: "First line\nsecond line here".into(),
    });
    let mut h = harness(w);
    h.run();
    assert!(h.query_by_label_contains("second line here").is_none());
    let rect = h.get_by_label_contains("First line").rect();
    h.hover_at(egui::pos2(rect.right() + 100.0, rect.center().y));
    for _ in 0..10 {
        h.run();
    }
    assert!(h.query_by_label_contains("second line here").is_some());
    assert_eq!(retrogit::ui::pull_detail::comment_hover("one line\n"), None);
}
