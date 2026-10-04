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

fn disabled(h: &Harness<'static, World>, label: &str) -> bool {
    use egui_kittest::kittest::NodeT;
    h.get_by_label(label).accesskit_node().is_disabled()
}

#[test]
fn mark_resolved_is_greyed_until_git_answers() {
    let mut w = world(ConflictKind::Content);
    w.state
        .changes
        .conflict
        .as_mut()
        .unwrap()
        .edit("fn login() {\n    check(a);\n}\n".into());
    let mut h = harness(w);
    h.run();
    assert!(!disabled(&h, s::MARK_RESOLVED));
    h.get_by_label(s::MARK_RESOLVED).click();
    h.run();
    assert!(h.state().state.changes.conflict.as_ref().unwrap().resolving);
    assert!(disabled(&h, s::MARK_RESOLVED));
    assert!(disabled(&h, "Whole file: theirs"));
}

#[test]
fn a_failed_conflict_load_says_why() {
    let mut w = world(ConflictKind::Content);
    assert!(w.state.changes.open_conflict("README.md"));
    w.state.apply(Event::Error {
        during: retrogit::protocol::Op::Conflict("README.md".into()),
        error: retrogit::protocol::AppError::new(
            retrogit::protocol::Severity::Warning,
            "cannot read README.md: Is a directory",
        ),
    });
    let mut h = harness(w);
    h.run();
    assert!(
        h.query_by_label("cannot read README.md: Is a directory")
            .is_some()
    );
    assert!(h.query_by_label(s::LOADING_CONFLICT).is_none());
}

#[test]
fn next_scrolls_the_result_to_the_current_block() {
    let mut w = world(ConflictKind::Content);
    let lines = |n: usize, t: &str| (0..n).map(|i| format!("{t}{i}\n")).collect::<String>();
    let block = |m: &str| format!("<<<<<<< HEAD\n{m}\n=======\nother\n>>>>>>> x\n");
    let working = format!(
        "{}{}{}{}{}",
        lines(100, "top"),
        block("one"),
        lines(300, "middle"),
        block("two"),
        lines(100, "end")
    );
    assert!(w.state.changes.open_conflict("README.md"));
    w.state.apply(Event::ConflictLoaded(Box::new(ConflictFile {
        path: "README.md".into(),
        kind: ConflictKind::Content,
        mine: Some(format!(
            "{}one\n{}two\n",
            lines(100, "top"),
            lines(300, "middle")
        )),
        theirs: Some(format!(
            "{}other\n{}other\n",
            lines(100, "top"),
            lines(300, "middle")
        )),
        working: Some(working),
        operation: Some(Operation::Merge),
    })));
    let mut h = harness(w);
    h.run();
    h.run();
    let top = |h: &Harness<'static, World>| result_input(h, "middle299").rect().top();
    let first = top(&h);
    h.get_by_label(s::NEXT_CONFLICT).click();
    h.run();
    h.run();
    let second = top(&h);
    // 305 lines further down: the result moved up by far more than a screen.
    assert!(second < first - 2000.0, "{first} -> {second}");
}

#[test]
fn typing_above_the_current_block_does_not_scroll_the_result() {
    let mut w = world(ConflictKind::Content);
    let lines = |n: usize, t: &str| (0..n).map(|i| format!("{t}{i}\n")).collect::<String>();
    let block = |m: &str| format!("<<<<<<< HEAD\n{m}\n=======\nother\n>>>>>>> x\n");
    let working = format!(
        "{}{}{}{}{}",
        lines(100, "top"),
        block("one"),
        lines(300, "middle"),
        block("two"),
        lines(100, "end")
    );
    assert!(w.state.changes.open_conflict("README.md"));
    w.state.apply(Event::ConflictLoaded(Box::new(ConflictFile {
        path: "README.md".into(),
        kind: ConflictKind::Content,
        mine: Some(format!(
            "{}one\n{}two\n",
            lines(100, "top"),
            lines(300, "middle")
        )),
        theirs: Some(format!(
            "{}other\n{}other\n",
            lines(100, "top"),
            lines(300, "middle")
        )),
        working: Some(working),
        operation: Some(Operation::Merge),
    })));
    let mut h = harness(w);
    h.run();
    h.run();
    // The user scrolls back to the top, then adds a line there.
    h.event(egui::Event::PointerMoved(egui::pos2(900.0, 500.0)));
    h.event(egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: egui::vec2(0.0, 100_000.0),
        phase: egui::TouchPhase::Move,
        modifiers: egui::Modifiers::NONE,
    });
    h.run_steps(30);
    let top = |h: &Harness<'static, World>| result_input(h, "middle299").rect().top();
    let before = top(&h);
    let typed = format!("new line\n{}", result(&h));
    h.state_mut()
        .state
        .changes
        .conflict
        .as_mut()
        .unwrap()
        .typed(typed);
    h.run();
    h.run();
    let after = top(&h);
    assert!((after - before).abs() < 1.0, "{before} -> {after}");
}

#[test]
fn enter_and_backspace_in_a_crlf_file_keep_crlf() {
    let mut w = world(ConflictKind::Content);
    assert!(w.state.changes.open_conflict("README.md"));
    w.state.apply(Event::ConflictLoaded(Box::new(ConflictFile {
        path: "README.md".into(),
        kind: ConflictKind::Content,
        mine: Some("readme mine\r\n".into()),
        theirs: Some("readme theirs\r\n".into()),
        working: Some(
            "<<<<<<< HEAD\r\nreadme mine\r\n=======\r\nreadme theirs\r\n>>>>>>> x\r\n".into(),
        ),
        operation: Some(Operation::Merge),
    })));
    let mut h = harness(w);
    h.run();
    result_input(&h, "readme").focus();
    h.run();
    result_input(&h, "readme").type_text("x");
    h.run();
    h.key_press(egui::Key::Enter);
    h.run();
    result_input(&h, "readme").type_text("y");
    h.run();
    let content =
        |h: &Harness<'static, World>| h.state().state.changes.conflict.as_ref().unwrap().content();
    let r = content(&h);
    assert!(r.contains("x\r\ny"), "{r:?}");
    assert_eq!(r.matches('\n').count(), r.matches("\r\n").count(), "{r:?}");
    // Backspace at the start of a line joins it with the line above, leaving no lone '\r'.
    h.key_press(egui::Key::ArrowLeft);
    h.run();
    h.key_press(egui::Key::Backspace);
    h.run();
    let r = content(&h);
    assert!(r.contains("xy"), "{r:?}");
    assert_eq!(r.matches('\r').count(), r.matches("\r\n").count(), "{r:?}");
}
