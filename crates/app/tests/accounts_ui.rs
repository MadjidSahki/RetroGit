#![allow(clippy::unwrap_used)]
//! Several accounts in the UI (headless, with a mock worker).

use std::path::PathBuf;
use std::sync::Arc;

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use gitcore::{Head, RepoSummary};
use github::{AccountStatus, Client, MemoryAccounts, RepoInfo, TokenProvider, User};
use retrogit::config::Config;
use retrogit::protocol::Event;
use retrogit::state::AppState;
use retrogit::strings as s;
use retrogit::ui::Ctx;
use retrogit::ui::accounts::{folder_from_url, who_text};
use retrogit::worker::{WorkerDeps, WorkerHandle, spawn};

struct World {
    state: AppState,
    worker: WorkerHandle,
    highlighter: retrogit::highlight::Service,
    notices: std::sync::mpsc::Sender<retrogit::protocol::AppError>,
}

fn status(login: &str, valid: bool) -> AccountStatus {
    AccountStatus {
        login: login.into(),
        name: None,
        valid,
    }
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
    state.apply(Event::SignedIn(User {
        login: "perso".into(),
        name: None,
    }));
    state.apply(Event::AccountsChanged(vec![
        status("perso", true),
        status("pro", false),
    ]));
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
                retrogit::ui::accounts::show(&ctx, &mut cx);
                retrogit::ui::clone_dialog::show(&ctx, &mut cx);
                retrogit::ui::sign_in::show(&ctx, &mut cx);
            },
            w,
        )
}

fn open_repo(st: &mut AppState, origin: &str) {
    st.apply(Event::RepoOpened(RepoSummary {
        name: "app".into(),
        path: PathBuf::from("/tmp/app"),
        head: Head::Branch("main".into()),
        origin_url: Some(origin.into()),
        last_commit: None,
    }));
}

#[test]
fn the_status_bar_names_the_account_of_the_repository() {
    let mut st = world().state;
    assert_eq!(who_text(&st), "2 accounts");
    open_repo(&mut st, "https://github.com/corp/app.git");
    assert_eq!(who_text(&st), s::CHECKING_ACCOUNT);
    st.apply(Event::RepoAccount {
        slug: ("Corp".into(), "App".into()),
        login: Some("pro".into()),
    });
    assert_eq!(who_text(&st), "Account: @pro");
    st.apply(Event::RepoAccount {
        slug: ("corp".into(), "app".into()),
        login: None,
    });
    assert_eq!(who_text(&st), s::GIT_CREDENTIALS);
    open_repo(&mut st, "https://gitlab.com/x/y.git");
    assert_eq!(who_text(&st), s::GIT_CREDENTIALS, "not on GitHub");
}

#[test]
fn folders_are_named_after_the_url() {
    assert_eq!(
        folder_from_url("https://github.com/o/my-app.git").as_deref(),
        Some("my-app")
    );
    assert_eq!(
        folder_from_url("git@github.com:o/repo").as_deref(),
        Some("repo")
    );
    assert_eq!(
        folder_from_url("ssh://git@host:2222/team/x.git/").as_deref(),
        Some("x")
    );
    assert_eq!(
        folder_from_url("  https://gitlab.com/g/sub/proj  ").as_deref(),
        Some("proj")
    );
    assert_eq!(folder_from_url(""), None);
    assert_eq!(folder_from_url("https://github.com/"), None);
}

#[test]
fn ssh_urls_of_github_are_cloned_over_https_and_others_are_refused() {
    use retrogit::ui::accounts::clone_url_for;
    assert_eq!(
        clone_url_for("git@github.com:o/r.git").as_deref(),
        Ok("https://github.com/o/r.git")
    );
    assert_eq!(
        clone_url_for("ssh://git@github.com/o/r").as_deref(),
        Ok("https://github.com/o/r.git")
    );
    assert_eq!(
        clone_url_for(" https://gitlab.com/g/p.git ").as_deref(),
        Ok("https://gitlab.com/g/p.git")
    );
    assert_eq!(
        clone_url_for("git@gitlab.com:g/p.git"),
        Err(s::ERR_SSH_CLONE)
    );
    assert_eq!(clone_url_for("not a url"), Err(s::ERR_CLONE_URL));
}

#[test]
fn the_accounts_window_lists_accounts_and_adds_one() {
    let mut w = world();
    w.state.accounts_dialog = true;
    let mut h = harness(w);
    h.run();
    assert!(h.query_by_label("@perso").is_some());
    assert!(
        h.query_by_label(s::SIGN_IN_AGAIN).is_some(),
        "pro is invalid"
    );
    h.get_by_label(s::ADD_ACCOUNT).click();
    h.run();
    assert!(
        h.state().state.sign_in.is_some(),
        "the sign-in window opens"
    );
}

#[test]
fn the_clone_window_shows_accounts_and_takes_a_url() {
    let mut w = world();
    w.state.apply(Event::ReposLoaded(vec![RepoInfo {
        full_name: "corp/app".into(),
        name: "app".into(),
        owner: "corp".into(),
        private: true,
        clone_url: "https://github.com/corp/app.git".into(),
        updated_at: "2026-09-30T10:00:00Z".into(),
        accounts: vec!["perso".into(), "pro".into()],
    }]));
    w.state.clone = Some(retrogit::state::CloneDialog {
        dest_parent: "/tmp".into(),
        ..Default::default()
    });
    let mut h = harness(w);
    h.run();
    assert!(h.query_by_label("app").is_some(), "listed");
    assert_eq!(
        retrogit::ui::clone_dialog::accounts_cell(&["perso".into(), "pro".into()]),
        "perso, pro"
    );
    h.state_mut().state.clone.as_mut().unwrap().url = "git@github.com:o/tool.git".into();
    h.run();
    assert!(
        h.query_by_label_contains("tool").is_some(),
        "destination from the URL"
    );
}
