#![allow(clippy::unwrap_used)]
//! The Repositories panel drawn headless: arrow, branch folders, double-click checkout,
//! "New branch from here...". Real repositories and a real worker (needs `git`).

use std::path::{Path, PathBuf};
use std::process::Command as Cmd;
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use github::{Client, MemoryAccounts, TokenProvider};
use retrogit::config::Config;
use retrogit::protocol::Command;
use retrogit::state::{AppState, PendingDialog};
use retrogit::strings as s;
use retrogit::ui::Ctx;
use retrogit::worker::{WorkerDeps, WorkerHandle, spawn};

struct World {
    state: AppState,
    worker: WorkerHandle,
    highlighter: retrogit::highlight::Service,
    notices: std::sync::mpsc::Sender<retrogit::protocol::AppError>,
    _server: mockito::ServerGuard,
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Cmd::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A repository on `main` with `branches` created at its only commit.
fn repo(root: &Path, name: &str, branches: &[&str]) -> PathBuf {
    let dir = root.join(name);
    std::fs::create_dir(&dir).unwrap();
    git(&dir, &["init", "-q", "-b", "main"]);
    for (k, v) in [
        ("user.name", "Ada"),
        ("user.email", "ada@example.com"),
        ("commit.gpgsign", "false"),
    ] {
        git(&dir, &["config", k, v]);
    }
    std::fs::write(dir.join("README.md"), "hello\n").unwrap();
    git(&dir, &["add", "."]);
    git(&dir, &["commit", "-q", "-m", "init"]);
    for b in branches {
        git(&dir, &["branch", b]);
    }
    dir
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
    let (notices, _rx) = std::sync::mpsc::channel();
    World {
        state: AppState::new(Config::default()),
        worker,
        highlighter: retrogit::highlight::Service::start(|_| {}),
        notices,
        _server: server,
    }
}

fn harness(w: World) -> Harness<'static, World> {
    Harness::builder()
        .with_size(egui::vec2(1200.0, 700.0))
        // Short steps: two clicks a step apart make a double-click (0.3 s at most).
        .with_step_dt(0.05)
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
                retrogit::ui::sync_dialogs::show(&ctx, &mut cx);
            },
            w,
        )
}

/// Apply worker events until `done` holds for the state (10 s at most).
fn until(h: &mut Harness<'static, World>, done: impl Fn(&AppState) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !done(&h.state().state) {
        assert!(Instant::now() < deadline, "timed out");
        let w = h.state_mut();
        if let Ok(ev) = w.worker.events.recv_timeout(Duration::from_millis(50)) {
            w.state.apply(ev);
        }
    }
    h.run();
}

/// Two clicks one step apart. Right after another click (the arrow), as users do: egui
/// then counts a triple-click.
fn double_click(h: &mut Harness<'static, World>, label: &str) {
    h.get_by_label(label).click();
    h.step();
    h.get_by_label(label).click();
    h.step();
}

fn head_is(st: &AppState, branch: &str) -> bool {
    st.current
        .as_ref()
        .is_some_and(|c| c.head == gitcore::Head::Branch(branch.into()))
}

/// Two repositories, `alpha` open (with its branches loaded).
fn opened() -> (tempfile::TempDir, PathBuf, PathBuf, Harness<'static, World>) {
    let root = tempfile::tempdir().unwrap();
    let alpha = repo(root.path(), "alpha", &["feat/login", "feat/api", "dev"]);
    let beta = repo(root.path(), "beta", &["fix/bug"]);
    let mut w = world();
    w.state.config.add_recent("beta", &beta);
    w.worker.send(Command::OpenRepo(alpha.clone()));
    let mut h = harness(w);
    until(&mut h, |st| head_is(st, "main") && !st.branches.is_empty());
    (root, alpha, beta, h)
}

#[test]
fn the_arrow_shows_the_branches_in_folders() {
    let (_root, _alpha, _beta, mut h) = opened();
    assert!(h.query_by_label("feat/login").is_none(), "folded at first");
    h.get_by_label(&s::show_branches("alpha")).click();
    h.run();
    assert!(h.query_by_label(&s::folder_label("feat", false)).is_some());
    assert!(h.query_by_label("dev").is_some());
    assert!(
        h.query_by_label("feat/login").is_none(),
        "the folder is closed"
    );
    h.get_by_label(&s::folder_label("feat", false)).click();
    h.run();
    assert!(h.query_by_label("feat/login").is_some());
    assert!(h.query_by_label("feat/api").is_some());
    h.get_by_label(&s::hide_branches("alpha")).click();
    h.run();
    assert!(h.query_by_label("dev").is_none());
}

#[test]
fn the_current_branch_has_its_own_colour_and_double_click_checks_out() {
    let (_root, alpha, _beta, mut h) = opened();
    h.get_by_label(&s::show_branches("alpha")).click();
    h.run();
    assert!(
        h.query_by_label(&s::current_branch_label("main")).is_some(),
        "the current branch is told apart"
    );
    double_click(&mut h, "dev");
    until(&mut h, |st| {
        head_is(st, "dev") && st.branches.iter().any(|b| b.name == "dev" && b.is_head)
    });
    assert_eq!(git(&alpha, &["branch", "--show-current"]), "dev");
    assert!(h.query_by_label(&s::current_branch_label("dev")).is_some());
}

#[test]
fn a_single_click_on_a_branch_does_not_check_it_out() {
    let (_root, alpha, _beta, mut h) = opened();
    h.get_by_label(&s::show_branches("alpha")).click();
    h.run();
    h.get_by_label("dev").click();
    h.run();
    assert_eq!(h.state().state.repo_tree.selected.as_deref(), Some("dev"));
    assert_eq!(git(&alpha, &["branch", "--show-current"]), "main");
}

#[test]
fn right_click_starts_a_new_branch_from_the_branch() {
    let (_root, _alpha, _beta, mut h) = opened();
    h.get_by_label(&s::show_branches("alpha")).click();
    h.run();
    h.get_by_label("dev").click_secondary();
    h.run();
    h.get_by_label(s::NEW_BRANCH_FROM_MENU).click();
    h.run();
    assert_eq!(
        h.state().state.dialog,
        Some(PendingDialog::NewBranch {
            name: String::new(),
            from: Some("dev".into()),
            switch: true,
        })
    );
    assert!(h.query_by_label(&s::new_branch_from("dev")).is_some());
}

#[test]
fn the_arrow_of_another_repository_opens_and_unfolds_it() {
    let (_root, _alpha, _beta, mut h) = opened();
    h.get_by_label(&s::show_branches("beta")).click();
    h.run();
    until(&mut h, |st| {
        st.current.as_ref().is_some_and(|c| c.name == "beta") && !st.branches.is_empty()
    });
    assert!(h.query_by_label(&s::folder_label("fix", false)).is_some());
    assert!(h.state().state.repo_tree.expanded);
}

#[test]
fn branches_are_indented_under_their_folder() {
    let (_root, _alpha, _beta, mut h) = opened();
    h.get_by_label(&s::show_branches("alpha")).click();
    h.run();
    h.get_by_label(&s::folder_label("feat", false)).click();
    h.run();
    let label_x = |h: &Harness<'static, World>, label: &str| {
        let r = h.get_by_label(label).rect();
        (r.left(), r.top())
    };
    let (_, folder_y) = label_x(&h, &s::folder_label("feat", false));
    let (_, login_y) = label_x(&h, "feat/login");
    assert!(login_y > folder_y, "below its folder");
    // Rows span the panel; the indent is in the painted text, so check the model depth.
    let rows = retrogit::state::repo_tree::sidebar_rows(
        &h.state().state.recents_sorted(),
        h.state().state.current.as_ref().map(|c| c.path.as_path()),
        &h.state().state.repo_tree,
        &h.state().state.branches,
    );
    let depth = |label: &str| rows.iter().find(|r| r.label == label).unwrap().depth;
    assert_eq!((depth("feat"), depth("login")), (1, 2));
}

#[test]
fn a_long_branch_list_fills_the_panel_to_the_bottom() {
    let root = tempfile::tempdir().unwrap();
    let names: Vec<String> = (0..80).map(|i| format!("b{i:02}")).collect();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let alpha = repo(root.path(), "alpha", &refs);
    let w = world();
    w.worker.send(Command::OpenRepo(alpha));
    let mut h = harness(w);
    until(&mut h, |st| st.branches.len() > 80);
    h.get_by_label(&s::show_branches("alpha")).click();
    h.run();
    let shown: Vec<egui::Rect> = names
        .iter()
        .filter_map(|n| h.query_by_label(n).map(|node| node.rect()))
        .collect();
    let first = shown.first().unwrap();
    let last = shown.last().unwrap();
    // Rows touch each other, and the drawn ones reach the panel's bottom (status bar above
    // the 700 px window edge).
    for pair in shown.windows(2) {
        assert!((pair[1].top() - pair[0].bottom()).abs() < 0.5, "{pair:?}");
    }
    assert!(
        last.bottom() > 640.0,
        "rows {first:?} .. {last:?} leave a blank band"
    );
}

#[test]
fn double_clicking_a_folder_or_the_arrow_leaves_it_open() {
    let (_root, _alpha, _beta, mut h) = opened();
    double_click(&mut h, &s::show_branches("alpha"));
    assert!(
        h.state().state.repo_tree.expanded,
        "the arrow's double-click opens once"
    );
    let feat = s::folder_label("feat", false);
    double_click(&mut h, &feat);
    assert!(h.state().state.repo_tree.open.contains("feat"));
    assert!(h.query_by_label("feat/login").is_some());
}

#[test]
fn local_and_remote_folders_have_their_own_names() {
    let root = tempfile::tempdir().unwrap();
    let alpha = repo(root.path(), "alpha", &["feat/x"]);
    git(
        &alpha,
        &["update-ref", "refs/remotes/upstream/feat/y", "HEAD"],
    );
    let w = world();
    w.worker.send(Command::OpenRepo(alpha));
    let mut h = harness(w);
    until(&mut h, |st| {
        st.branches.iter().any(|b| b.name == "upstream/feat/y")
    });
    h.get_by_label(&s::show_branches("alpha")).click();
    h.run();
    h.get_by_label(&s::folder_label("upstream", true)).click();
    h.run();
    assert!(h.query_by_label(&s::folder_label("feat", false)).is_some());
    assert!(
        h.query_by_label(&s::folder_label("upstream/feat", true))
            .is_some()
    );
}

#[test]
fn no_checkout_while_a_merge_or_rebase_is_in_progress() {
    use egui_kittest::kittest::NodeT;
    let (_root, _alpha, _beta, mut h) = opened();
    h.state_mut().state.operation = Some(gitcore::Operation::Merge);
    h.get_by_label(&s::show_branches("alpha")).click();
    h.run();
    h.get_by_label("dev").click_secondary();
    h.run();
    assert!(
        h.get_by_label(s::CHECKOUT_BRANCH_MENU)
            .accesskit_node()
            .is_disabled()
    );
}

#[test]
fn checkout_from_the_right_click_menu() {
    let (_root, alpha, _beta, mut h) = opened();
    h.get_by_label(&s::show_branches("alpha")).click();
    h.run();
    h.get_by_label("dev").click_secondary();
    h.run();
    h.get_by_label(s::CHECKOUT_BRANCH_MENU).click();
    h.run();
    assert!(
        h.query_by_label(s::CHECKOUT_BRANCH_MENU).is_none(),
        "the menu closes"
    );
    until(&mut h, |st| head_is(st, "dev"));
    assert_eq!(git(&alpha, &["branch", "--show-current"]), "dev");
}
