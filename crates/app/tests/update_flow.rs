#![allow(clippy::unwrap_used)]
//! Updates: checking GitHub (mock server), what the state does with the answer, the window.

use std::sync::Arc;

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use github::{Client, MemoryAccounts, TokenProvider};
use retrogit::config::Config;
use retrogit::state::AppState;
use retrogit::strings as s;
use retrogit::ui::Ctx;
use retrogit::update::{Release, fetch_latest};
use retrogit::worker::{WorkerDeps, WorkerHandle, spawn};

fn release(version: &str) -> Release {
    Release {
        version: version.into(),
        tag: format!("v{version}"),
        notes: "## What's new\n* Faster".into(),
        url: format!("https://github.com/MadjidSahki/RetroGit/releases/tag/v{version}"),
        assets: Vec::new(),
    }
}

#[test]
fn the_latest_release_is_fetched_without_a_token() {
    let mut server = mockito::Server::new();
    let m = server
        .mock("GET", "/repos/MadjidSahki/RetroGit/releases/latest")
        .match_header("authorization", mockito::Matcher::Missing)
        .with_body(r#"{"tag_name":"v0.1.50","body":"notes","html_url":"https://x","assets":[]}"#)
        .create();
    let r = fetch_latest(&server.url()).unwrap();
    m.assert();
    assert_eq!(r.version, "0.1.50");
    server
        .mock("GET", "/repos/MadjidSahki/RetroGit/releases/latest")
        .with_status(403)
        .with_body(r#"{"message":"API rate limit exceeded"}"#)
        .create();
    assert!(
        fetch_latest(&server.url())
            .unwrap_err()
            .contains("rate limit")
    );
}

#[test]
fn a_newer_release_is_offered_and_can_be_skipped() {
    let mut st = AppState::new(Config::default());
    st.update_checked("0.1.39", Ok(release("0.1.42")), false);
    assert_eq!(st.update.available.as_ref().unwrap().version, "0.1.42");
    assert!(!st.update.open, "automatic: only the toolbar button");
    assert!(st.messages.is_empty());
    st.skip_update();
    assert_eq!(st.config.updates.skipped.as_deref(), Some("0.1.42"));
    assert!(st.update.available.is_none());
    assert!(st.config_dirty);
    st.update_checked("0.1.39", Ok(release("0.1.42")), false);
    assert!(st.update.available.is_none(), "skipped stays skipped");
}

#[test]
fn a_manual_check_always_answers() {
    let mut st = AppState::new(Config::default());
    st.request_update_check();
    assert!(st.update.checking);
    st.update_checked("0.1.42", Ok(release("0.1.42")), true);
    assert!(!st.update.checking);
    assert!(st.messages.back().unwrap().message.contains("up to date"));
    st.update_checked("0.1.42", Err("no network".into()), true);
    assert_eq!(
        st.messages.back().unwrap().detail.as_deref(),
        Some("no network")
    );
    st.update_checked("0.1.42", Err("no network".into()), false);
    assert_eq!(st.messages.len(), 2, "automatic failures are silent");
    st.update_checked("0.1.39", Ok(release("0.1.42")), true);
    assert!(st.update.open, "manual: the window opens");
    // A manual check also offers a skipped version again.
    st.config.updates.skipped = Some("0.1.43".into());
    st.update_checked("0.1.39", Ok(release("0.1.43")), true);
    assert_eq!(st.update.available.as_ref().unwrap().version, "0.1.43");
}

struct World {
    state: AppState,
    worker: WorkerHandle,
    highlighter: retrogit::highlight::Service,
    notices: std::sync::mpsc::Sender<retrogit::protocol::AppError>,
}

fn harness(state: AppState) -> Harness<'static, World> {
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
    let (notices, _rx) = std::sync::mpsc::channel();
    let w = World {
        state,
        worker,
        highlighter: retrogit::highlight::Service::start(|_| {}),
        notices,
    };
    Harness::builder()
        .with_size(egui::vec2(1000.0, 650.0))
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
                retrogit::ui::update::show(&ctx, &mut cx);
            },
            w,
        )
}

#[test]
fn the_toolbar_button_opens_the_update_window() {
    let mut st = AppState::new(Config::default());
    st.update_checked("0.1.39", Ok(release("0.1.42")), false);
    let mut h = harness(st);
    h.run();
    h.get_by_label("Update available (0.1.42)").click();
    h.run();
    assert!(h.state().state.update.open);
    assert!(
        h.query_by_label_contains("Faster").is_some(),
        "release notes"
    );
    assert!(h.query_by_label(s::SKIP_VERSION).is_some());
    assert!(h.query_by_label(s::CHECK_AUTOMATICALLY).is_some());
    // Not installable here (no file for this installation): Download instead.
    assert!(h.query_by_label(s::DOWNLOAD_UPDATE).is_some());
    h.get_by_label(s::LATER).click();
    h.run();
    assert!(!h.state().state.update.open);
    assert!(
        h.state().state.update.available.is_some(),
        "Later keeps the button"
    );
}
