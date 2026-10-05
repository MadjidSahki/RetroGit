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
        message: format!("commit {id}"),
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
        h.query_by_label(s::todo_error(gitcore::TodoError::NoKeptAbove))
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
        overwrites: Vec::new(),
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
        status: None,
    });
    let mut h = harness(w);
    h.run();
    assert!(h.query_by_label_contains("v1.0").is_some());
    assert!(h.query_by_label(s::PUSH_ALL_TAGS).is_some());
}

#[test]
fn the_stash_and_retry_dialog_lists_the_files_in_the_way() {
    let mut w = world();
    w.state.git_dialog = Some(GitDialog::StashRetry {
        retry: Box::new(retrogit::protocol::Command::CherryPick {
            id: "x".into(),
            mainline: None,
        }),
        files: vec!["src/in_the_way.rs".into()],
    });
    let mut h = harness(w);
    h.run();
    assert!(h.query_by_label_contains("src/in_the_way.rs").is_some());
    h.get_by_label(s::STASH_AND_RETRY).click();
    h.run();
    assert!(h.state().state.git_dialog.is_none());
}

#[test]
fn a_hard_reset_names_the_untracked_files_it_replaces() {
    let mut w = world();
    w.state.git_dialog = Some(GitDialog::Reset {
        id: "abc".into(),
        mode: gitcore::ResetMode::Hard,
        hard_confirmed: false,
        drops_pushed: false,
        overwrites: vec!["notes.txt".into()],
    });
    let mut h = harness(w);
    h.run();
    assert!(h.query_by_label_contains("notes.txt").is_some());
    assert!(
        h.query_by_label(s::RESET_HARD_UNTRACKED).is_none(),
        "not 'kept'"
    );
}

#[test]
fn new_tag_from_the_tags_window_targets_head() {
    let mut w = world();
    w.state.history.entries = vec![gitcore::LogEntry {
        id: "fetched0tip".into(),
        short_id: "fetched".into(),
        parents: Vec::new(),
        author: "a".into(),
        email: "a@b".into(),
        time: 0,
        summary: "origin/main tip".into(),
        refs: Vec::new(),
    }];
    w.state.git_dialog = Some(GitDialog::Tags {
        filter: String::new(),
        selected: None,
        status: None,
    });
    let mut h = harness(w);
    h.run();
    h.get_by_label(s::NEW_TAG).click();
    h.run();
    assert!(
        matches!(&h.state().state.git_dialog, Some(GitDialog::CreateTag { id, .. }) if id == "HEAD")
    );
    assert!(
        h.query_by_label_contains("On: HEAD").is_some(),
        "target shown"
    );
}

#[test]
fn skip_asks_before_dropping_the_commit() {
    let mut w = world();
    w.state.apply(Event::OperationChanged(Some(
        gitcore::Operation::CherryPick,
    )));
    let mut h = harness(w);
    h.run();
    h.get_by_label(s::SKIP).click();
    h.run();
    assert!(matches!(
        h.state().state.git_dialog,
        Some(GitDialog::ConfirmSkip)
    ));
    assert!(h.query_by_label_contains("left out").is_some());
}

fn tags_world() -> World {
    let mut w = world();
    w.state.apply(Event::TagsLoaded(vec![gitcore::Tag {
        name: "v1.0".into(),
        commit: "0123456789".into(),
        annotated: false,
        message: String::new(),
    }]));
    w
}

#[test]
fn creating_or_deleting_a_tag_from_the_tags_window_goes_back_to_it() {
    let mut w = tags_world();
    w.state.git_dialog = Some(GitDialog::CreateTag {
        id: "HEAD".into(),
        name: "v2.0".into(),
        message: String::new(),
        annotated: false,
        back_to_tags: true,
    });
    let mut h = harness(w);
    h.run();
    h.get_by_label(s::CREATE).click();
    h.run();
    assert!(matches!(
        h.state().state.git_dialog,
        Some(GitDialog::Tags { .. })
    ));
    h.state_mut().state.git_dialog = Some(GitDialog::DeleteTag {
        name: "v1.0".into(),
        remote: false,
        back_to_tags: true,
    });
    h.run();
    h.get_by_label(s::CANCEL).click();
    h.run();
    assert!(matches!(
        h.state().state.git_dialog,
        Some(GitDialog::Tags { .. })
    ));
}

#[test]
fn pushing_tags_shows_progress_then_the_result_in_the_window() {
    let mut w = tags_world();
    w.state.git_dialog = Some(GitDialog::Tags {
        filter: String::new(),
        selected: None,
        status: None,
    });
    let mut h = harness(w);
    h.run();
    h.get_by_label(s::PUSH_ALL_TAGS).click();
    h.run();
    assert!(h.query_by_label(s::PUSHING_TAGS).is_some());
    h.state_mut()
        .state
        .apply(Event::TagsStatus("Pushed all tags to origin".into()));
    h.run();
    assert!(h.query_by_label("Pushed all tags to origin").is_some());
}

#[test]
fn changing_the_reset_mode_forgets_the_hard_confirmation() {
    let mut w = world();
    w.state.git_dialog = Some(GitDialog::Reset {
        id: "abc".into(),
        mode: gitcore::ResetMode::Hard,
        hard_confirmed: true,
        drops_pushed: false,
        overwrites: Vec::new(),
    });
    let mut h = harness(w);
    h.run();
    h.get_by_label(s::RESET_MIXED).click();
    h.run();
    h.get_by_label(s::RESET_HARD).click();
    h.run();
    let Some(GitDialog::Reset {
        mode,
        hard_confirmed,
        ..
    }) = &h.state().state.git_dialog
    else {
        panic!("{:?}", h.state().state.git_dialog)
    };
    assert_eq!(*mode, gitcore::ResetMode::Hard);
    assert!(!hard_confirmed, "Hard asks again");
}

/// Every button with this label is greyed (the toolbar has a Push button too).
fn disabled(h: &Harness<'static, World>, label: &str) -> bool {
    use egui_kittest::kittest::NodeT;
    h.get_all_by_label(label)
        .all(|n| n.accesskit_node().is_disabled())
}

#[test]
fn tag_buttons_are_greyed_while_a_network_operation_runs() {
    let mut w = tags_world();
    w.state.git_dialog = Some(GitDialog::Tags {
        filter: String::new(),
        selected: Some("v1.0".into()),
        status: None,
    });
    w.state.sync.running = Some(retrogit::protocol::SyncOp::Push);
    let mut h = harness(w);
    h.run();
    for label in [s::PUSH_TAG, s::PUSH_ALL_TAGS, s::DELETE] {
        assert!(disabled(&h, label), "{label}");
    }
    h.state_mut().state.git_dialog = Some(GitDialog::DeleteTag {
        name: "v1.0".into(),
        remote: true,
        back_to_tags: true,
    });
    h.run();
    assert!(disabled(&h, s::DELETE));
    h.state_mut().state.sync.running = None;
    h.run();
    assert!(!disabled(&h, s::DELETE), "usable again");
}

#[test]
fn a_merge_dialog_offers_every_parent() {
    let mut w = world();
    w.state.git_dialog = Some(GitDialog::RevertMerge {
        id: "abc".into(),
        parent: 1,
        parents: 3,
    });
    let mut h = harness(w);
    h.run();
    assert!(h.query_by_label(s::KEEP_PARENT_1).is_some());
    assert!(h.query_by_label(s::KEEP_PARENT_2).is_some());
    h.get_by_label(&s::keep_parent(3)).click();
    h.run();
    assert!(matches!(
        h.state().state.git_dialog,
        Some(GitDialog::RevertMerge { parent: 3, .. })
    ));
    h.get_by_label(s::REVERT).click();
    h.run();
    assert!(h.state().state.git_dialog.is_none(), "sent and closed");
}

#[test]
fn cherry_picking_a_merge_asks_for_the_parent() {
    let mut w = world();
    w.state.git_dialog = Some(GitDialog::CherryPickMerge {
        id: "abc".into(),
        parent: 1,
        parents: 3,
    });
    let mut h = harness(w);
    h.run();
    assert!(h.query_by_label(s::CHERRY_PICK_MERGE_HELP).is_some());
    h.get_by_label(&s::pick_parent(3)).click();
    h.run();
    assert!(matches!(
        h.state().state.git_dialog,
        Some(GitDialog::CherryPickMerge { parent: 3, .. })
    ));
    h.get_by_label(s::CHERRY_PICK).click();
    h.run();
    assert!(h.state().state.git_dialog.is_none(), "sent and closed");
}
