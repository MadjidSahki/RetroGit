#![allow(clippy::unwrap_used)]
//! History menu, dialogs, Stashes tab and Tags window (headless, mock worker).

use std::path::PathBuf;
use std::sync::Arc;

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use gitcore::{Head, RepoSummary, StashEntry, TodoAction, TodoItem};
use github::{Client, MemoryAccounts, TokenProvider};
use retrogit::config::Config;
use retrogit::protocol::Event;
use retrogit::state::{AppState, GitDialog, Tab};
use retrogit::strings as s;
use retrogit::ui::Ctx;
use retrogit::worker::{WorkerDeps, WorkerHandle, spawn};

struct World {
    state: AppState,
    worker: WorkerHandle,
    highlighter: retrogit::highlight::Service,
    notices: std::sync::mpsc::Sender<retrogit::protocol::AppError>,
}

fn world() -> World {
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
        origin_url: None,
        last_commit: None,
    }));
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
        .with_size(egui::vec2(1100.0, 700.0))
        .build_ui_state(
            |ui, w: &mut World| {
                let ctx = ui.ctx().clone();
                win95::theme::install(&ctx);
                let mut cx = Ctx {
                    state: &mut w.state,
                    worker: &w.worker,
                    highlighter: &w.highlighter,
                    notices: &w.notices,
                };
                retrogit::ui::main_window::show(ui, &mut cx);
                retrogit::ui::git_dialogs::show(&ctx, &mut cx);
            },
            w,
        )
}

fn item(id: &str, action: TodoAction) -> TodoItem {
    TodoItem {
        action,
        id: id.into(),
        summary: format!("commit {id}"),
    }
}

#[test]
fn the_rebase_dialog_explains_why_it_cannot_start() {
    let mut w = world();
    w.state.git_dialog = Some(GitDialog::Rebase {
        base: "b".into(),
        items: vec![
            item("1", TodoAction::Squash(None)),
            item("2", TodoAction::Pick),
        ],
        pushed: 1,
    });
    let mut h = harness(w);
    h.run();
    assert!(h.query_by_label("commit 1").is_some());
    assert!(
        h.query_by_label_contains("A squash or fixup needs")
            .is_some()
    );
    assert!(
        h.query_by_label_contains("force push").is_some(),
        "pushed warning"
    );
}

#[test]
fn hard_reset_needs_its_confirmation() {
    let mut w = world();
    w.state.git_dialog = Some(GitDialog::Reset {
        id: "abc".into(),
        mode: gitcore::ResetMode::Hard,
        hard_confirmed: false,
        drops_pushed: true,
    });
    let mut h = harness(w);
    h.run();
    assert!(h.query_by_label(s::RESET_HARD_CONFIRM).is_some());
    assert!(h.query_by_label_contains("already pushed").is_some());
}

#[test]
fn the_stashes_tab_lists_stashes_and_offers_actions() {
    let mut w = world();
    w.state.tab = Tab::Stashes;
    w.state.apply(Event::StashesLoaded(vec![StashEntry {
        index: 0,
        id: "id0".into(),
        message: "wip login".into(),
        branch: "main".into(),
        time: 0,
    }]));
    let mut h = harness(w);
    h.run();
    assert!(h.query_by_label(s::TAB_STASHES).is_some());
    assert!(h.query_by_label_contains("wip login").is_some());
    assert!(h.query_by_label(s::STASH_CHANGES).is_some());
    h.get_by_label(s::STASH_CHANGES).click();
    h.run();
    assert!(matches!(
        h.state().state.git_dialog,
        Some(GitDialog::StashSave { .. })
    ));
}

#[test]
fn the_tags_window_lists_tags() {
    let mut w = world();
    w.state.apply(Event::TagsLoaded(vec![gitcore::Tag {
        name: "v1.0".into(),
        commit: "0123456789".into(),
        annotated: true,
        message: "Release".into(),
    }]));
    w.state.git_dialog = Some(GitDialog::Tags {
        filter: String::new(),
        selected: None,
    });
    let mut h = harness(w);
    h.run();
    assert!(h.query_by_label_contains("v1.0").is_some());
    assert!(h.query_by_label(s::PUSH_ALL_TAGS).is_some());
}
