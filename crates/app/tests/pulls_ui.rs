#![allow(clippy::unwrap_used)]
//! The Pull Requests tab drawn headless (egui_kittest), with a mock worker.

use std::path::PathBuf;
use std::sync::Arc;

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use gitcore::{Head, RepoSummary};
use github::{
    ChecksState, Client, Label, MemoryAccounts, MergeMethod, Mergeable, PrDetail, PrState,
    PrSummary, ReviewDecision, TokenProvider,
};
use retrogit::config::Config;
use retrogit::protocol::Event;
use retrogit::state::{AppState, PullDialog, Tab};
use retrogit::strings as s;
use retrogit::ui::Ctx;
use retrogit::worker::{WorkerDeps, WorkerHandle, spawn};

struct World {
    state: AppState,
    worker: WorkerHandle,
    highlighter: retrogit::highlight::Service,
    notices: std::sync::mpsc::Sender<retrogit::protocol::AppError>,
}

fn summary(draft: bool) -> PrSummary {
    PrSummary {
        number: 7,
        title: "Fix login".into(),
        url: "https://github.com/o/r/pull/7".into(),
        author: "bob".into(),
        head: "feat/login".into(),
        base: "main".into(),
        draft,
        state: PrState::Open,
        labels: vec![Label {
            name: "bug".into(),
            color: [0xd7, 0x3a, 0x4a],
            description: None,
        }],
        checks: ChecksState::Success,
        review_decision: ReviewDecision::Approved,
        updated_at: "2026-10-01T08:00:00Z".into(),
    }
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
    let slug = ("o".to_string(), "r".to_string());
    state.pulls.stale = false;
    state.apply(Event::PullsLoaded {
        slug: slug.clone(),
        filter: Default::default(),
        list: vec![summary(draft)],
    });
    state.pulls.select(7);
    state.apply(Event::PullLoaded {
        slug,
        detail: Box::new(PrDetail {
            summary: summary(draft),
            body: "Fixes the **login** page.".into(),
            head_sha: "abc".into(),
            head_repo: Some(("o".into(), "r".into())),
            cross_repository: false,
            commits: vec![],
            commit_count: 0,
            check_runs: vec![],
            timeline: vec![],
            threads: vec![],
            mergeable: Mergeable::Mergeable,
            merge_state: "CLEAN".into(),
            allowed_methods: vec![MergeMethod::Squash],
            viewer: "ada".into(),
            viewer_is_author: false,
            viewer_can_write: true,
            repo_labels: vec![Label {
                name: "docs".into(),
                color: [0x00, 0x75, 0xca],
                description: Some("Documentation".into()),
            }],
            id: "PR_7".into(),
            viewer_can_update: true,
            reviewers: vec![github::Reviewer {
                login: "carol".into(),
                state: None,
            }],
            assignees: vec!["ada".into()],
        }),
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
    harness_sized(w, 1100.0)
}

fn harness_sized(w: World, width: f32) -> Harness<'static, World> {
    Harness::builder()
        .with_size(egui::vec2(width, 700.0))
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
fn the_tab_shows_the_list_and_the_selected_pull_request() {
    let mut h = harness(world(false));
    h.run();
    assert!(h.query_by_label(s::TAB_PULLS).is_some());
    assert!(h.query_by_label("#7 Fix login").is_some(), "detail title");
    assert!(h.query_by_label("bug").is_some(), "label chip");
    assert!(h.query_by_label("login").is_some(), "Markdown description");
    h.get_by_label(s::MERGE_PULL).click();
    h.run();
    assert!(
        matches!(&h.state().state.pulls.dialog, Some(PullDialog::Merge { method: MergeMethod::Squash, title, .. }) if title == "Fix login (#7)")
    );
    assert!(h.query_by_label(s::CONFIRM_MERGE).is_some());
}

#[test]
fn a_draft_explains_why_it_cannot_be_merged() {
    let mut h = harness(world(true));
    h.run();
    assert!(h.query_by_label(s::WHY_DRAFT).is_some());
    h.get_by_label(s::MERGE_PULL).click();
    h.run();
    assert!(h.state().state.pulls.dialog.is_none());
}

#[test]
fn labels_are_edited_in_a_dialog() {
    let mut h = harness(world(false));
    h.run();
    h.get_by_label(s::EDIT_LABELS).click();
    h.run();
    assert!(h.query_by_label("Documentation").is_some());
    assert!(
        matches!(&h.state().state.pulls.dialog, Some(PullDialog::Labels { checked }) if checked == &["bug".to_string()])
    );
}

#[test]
fn a_repository_outside_github_explains_it() {
    let mut w = world(false);
    w.state.apply(Event::RepoOpened(RepoSummary {
        name: "other".into(),
        path: PathBuf::from("/tmp/other"),
        head: Head::Branch("main".into()),
        origin_url: Some("https://gitlab.com/o/r.git".into()),
        last_commit: None,
    }));
    w.state.tab = Tab::PullRequests;
    let mut h = harness(w);
    h.run();
    assert!(h.query_by_label(s::NOT_GITHUB).is_some());
}

#[test]
fn a_hovered_commit_stays_readable_in_high_contrast_white() {
    let mut w = world(false);
    {
        let pulls = &mut w.state.pulls;
        let mut d = (**pulls.detail.as_ref().unwrap()).clone();
        d.commits = vec![github::PrCommit {
            oid: "a".repeat(40),
            short_oid: "aaaaaaa".into(),
            headline: "Fix the login".into(),
            author: "bob".into(),
            date: "2026-10-01T08:00:00Z".into(),
        }];
        pulls.detail = Some(Arc::new(d));
        pulls.sub_tab = retrogit::state::PullTab::Commits;
    }
    let mut h = harness(w);
    win95::theme::install(&h.ctx);
    let scheme = win95::Scheme::HighContrastWhite;
    win95::theme::apply(
        &h.ctx,
        win95::theme::Appearance {
            scheme,
            font: win95::theme::Font::W95fa,
        },
    );
    h.run();
    h.get_by_label_contains("Fix the login").hover();
    h.run();
    let p = scheme.palette();
    let colors: Vec<egui::Color32> = h
        .output()
        .shapes
        .iter()
        .filter_map(|s| match &s.shape {
            egui::Shape::Text(t) if t.galley.job.text.contains("Fix the login") => Some(
                t.galley
                    .job
                    .sections
                    .first()
                    .map(|s| s.format.color)
                    .filter(|c| *c != egui::Color32::PLACEHOLDER)
                    .unwrap_or(t.fallback_color),
            ),
            _ => None,
        })
        .collect();
    assert!(!colors.is_empty());
    for c in colors {
        assert!(
            win95::palette::contrast(c, p.selection) >= 4.5,
            "hovered row text {c:?} on {:?}",
            p.selection
        );
    }
}

#[test]
fn a_long_title_keeps_edit_and_the_actions_on_screen() {
    let mut w = world(false);
    {
        let pulls = &mut w.state.pulls;
        let mut d = (**pulls.detail.as_ref().unwrap()).clone();
        d.summary.title = "A very long pull request title that goes on and on ".repeat(6);
        pulls.detail = Some(Arc::new(d));
    }
    // Narrow window (or Large size): the minimum window is 520 points wide.
    let mut h = harness_sized(w, 760.0);
    h.run();
    let screen = h.ctx.content_rect();
    for label in [s::EDIT_PULL, s::CHECKOUT, s::MERGE_PULL] {
        let r = h.get_by_label(label).rect();
        assert!(
            r.right() <= screen.right() && r.left() >= screen.left(),
            "{label} at {r:?}, screen {screen:?}"
        );
    }
}
