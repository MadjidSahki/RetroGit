#![allow(clippy::unwrap_used)]
//! The conflict editor drawn headless (egui_kittest), with a mock worker.

use std::path::PathBuf;
use std::sync::Arc;

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use gitcore::{Change, ConflictFile, ConflictKind, FileStatus, Head, Operation, RepoSummary};
use github::{Client, MemoryAccounts, TokenProvider};
use retrogit::config::Config;
use retrogit::protocol::Event;
use retrogit::state::{AppState, ConflictConfirm};
use retrogit::strings as s;
use retrogit::ui::Ctx;
use retrogit::worker::{WorkerDeps, WorkerHandle, spawn};

pub struct World {
    pub state: AppState,
    worker: WorkerHandle,
    highlighter: retrogit::highlight::Service,
    notices: std::sync::mpsc::Sender<retrogit::protocol::AppError>,
}

const MARKED: &str =
    "fn login() {\n<<<<<<< HEAD\n    check(a);\n=======\n    verify(a, b);\n>>>>>>> feat/x\n}\n";

pub fn world(kind: ConflictKind) -> World {
    world_in(kind, Operation::Merge)
}

pub fn world_in(kind: ConflictKind, op: Operation) -> World {
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
    state.apply(Event::OperationChanged(Some(op)));
    state.apply(Event::StatusLoaded(
        ["src/login.rs", "README.md"]
            .iter()
            .map(|p| FileStatus {
                path: p.to_string(),
                staged: None,
                unstaged: Some(Change::Conflicted),
            })
            .collect(),
    ));
    assert!(state.changes.open_conflict("src/login.rs"));
    state.apply(Event::ConflictLoaded(Box::new(ConflictFile {
        path: "src/login.rs".into(),
        kind,
        mine: Some("fn login() {\n    check(a);\n}\n".into()),
        theirs: Some("fn login() {\n    verify(a, b);\n}\n".into()),
        working: Some(MARKED.into()),
        operation: Some(op),
    })));
    let (notices, _rx) = std::sync::mpsc::channel();
    World {
        state,
        worker,
        highlighter: retrogit::highlight::Service::start(|_| {}),
        notices,
    }
}

pub fn harness(w: World) -> Harness<'static, World> {
    Harness::builder()
        .with_size(egui::vec2(1200.0, 700.0))
        .build_ui_state(
            |ui, w: &mut World| {
                let mut cx = Ctx {
                    state: &mut w.state,
                    worker: &w.worker,
                    highlighter: &w.highlighter,
                    notices: &w.notices,
                };
                retrogit::ui::main_window::show(ui, &mut cx);
            },
            w,
        )
}

fn result(h: &Harness<'static, World>) -> String {
    h.state()
        .state
        .changes
        .conflict
        .as_ref()
        .unwrap()
        .result
        .clone()
}

#[test]
fn the_editor_shows_three_panes_and_applies_choices() {
    let mut h = harness(world(ConflictKind::Content));
    h.run();
    assert!(h.query_by_label("Conflicts (2)").is_some());
    assert!(h.query_by_label("Mine (HEAD)").is_some());
    assert!(h.query_by_label("Theirs (feat/x)").is_some());
    assert!(
        h.query_by_label(&retrogit::ui::conflict_view::conflicts_left_text(1))
            .is_some()
    );
    h.get_by_label(s::USE_BOTH).click();
    h.run();
    assert_eq!(
        result(&h),
        "fn login() {\n    check(a);\n    verify(a, b);\n}\n"
    );
    assert!(h.query_by_label("0 conflicts left").is_some());
}

#[test]
fn marking_resolved_with_markers_left_asks_first() {
    let mut h = harness(world(ConflictKind::Content));
    h.run();
    h.get_by_label(s::MARK_RESOLVED).click();
    h.run();
    assert_eq!(
        h.state().state.changes.conflict.as_ref().unwrap().confirm,
        Some(ConflictConfirm::ResolveWithMarkers)
    );
    assert!(h.query_by_label(s::CONFIRM_MARKERS_LEFT).is_some());
    h.get_by_label(s::CANCEL).click();
    h.run();
    assert_eq!(
        h.state().state.changes.conflict.as_ref().unwrap().confirm,
        None
    );
}

#[test]
fn the_whole_file_asks_before_dropping_the_other_side() {
    let mut h = harness(world(ConflictKind::Content));
    h.run();
    h.get_by_label("Whole file: theirs").click();
    h.run();
    assert!(
        h.query_by_label_contains("Keep the other version")
            .is_some()
    );
}

#[test]
fn files_deleted_on_one_side_offer_keep_or_delete() {
    let mut h = harness(world(ConflictKind::DeletedByThem));
    h.run();
    assert!(
        h.query_by_label_contains("deleted in the other version")
            .is_some()
    );
    assert!(h.query_by_label(s::KEEP_FILE).is_some());
    assert!(h.query_by_label(s::DELETE_FILE).is_some());
}

#[test]
fn during_a_rebase_every_button_names_the_right_side() {
    let mut h = harness(world_in(ConflictKind::Content, Operation::Rebase));
    h.run();
    assert!(h.query_by_label("Use upstream").is_some());
    assert!(h.query_by_label("Use my commit").is_some());
    assert!(
        h.query_by_label("Use mine").is_none(),
        "no 'mine' during a rebase"
    );
    h.get_by_label("Whole file: my commit").click();
    h.run();
    assert!(
        h.query_by_label_contains("your commit").is_some(),
        "confirmation says what is kept"
    );
}

/// The result pane (the commit description is another multiline input).
fn result_input<'a>(h: &'a Harness<'static, World>, containing: &str) -> egui_kittest::Node<'a> {
    h.get_all_by_role(egui::accesskit::Role::MultilineTextInput)
        .find(|n| n.value().unwrap_or_default().contains(containing))
        .unwrap()
}

#[test]
fn undo_does_not_bring_back_another_files_text() {
    let mut h = harness(world(ConflictKind::Content));
    h.run();
    result_input(&h, "login").focus();
    h.run();
    result_input(&h, "login").type_text("typed in login.rs ");
    h.run();
    // Move to README.md (edits discarded), then press undo there.
    h.state_mut()
        .state
        .changes
        .conflict
        .as_mut()
        .unwrap()
        .edited = false;
    assert!(h.state_mut().state.changes.open_conflict("README.md"));
    h.state_mut()
        .state
        .apply(Event::ConflictLoaded(Box::new(ConflictFile {
            path: "README.md".into(),
            kind: ConflictKind::Content,
            mine: Some("readme mine\n".into()),
            theirs: Some("readme theirs\n".into()),
            working: Some("<<<<<<< HEAD\nreadme mine\n=======\nreadme theirs\n>>>>>>> x\n".into()),
            operation: Some(Operation::Merge),
        })));
    h.run();
    result_input(&h, "readme").focus();
    h.run();
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Z);
    h.run();
    assert!(!result(&h).contains("login"), "{}", result(&h));
}
