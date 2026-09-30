#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::sync::Arc;

use retrogit::app::RetroGitApp;
use retrogit::config::Config;
use retrogit::state::AppState;
use retrogit::worker::{WorkerDeps, spawn};
use retrogit::{GITHUB_CLIENT_ID, logging, strings};

fn main() -> eframe::Result {
    // `git` runs us as GIT_ASKPASS with the prompt as the only argument; the token variable
    // is only ever set in the environment of the git processes we start. Answer and exit
    // before any GUI setup.
    if let Ok(token) = std::env::var(gitcore::ASKPASS_TOKEN_VAR) {
        let prompt = std::env::args().nth(1).unwrap_or_default();
        println!("{}", gitcore::askpass_answer(&prompt, &token));
        return Ok(());
    }
    if let Some(dir) = dirs::data_local_dir() {
        logging::init(&dir.join("RetroGit").join("retrogit.log"));
    }
    logging::install_panic_hook();
    log::info!("RetroGit {} starting", env!("CARGO_PKG_VERSION"));
    if let Ok(exe) = std::env::current_exe() {
        gitcore::set_askpass_program(exe);
    }
    // Resolved in the background so a slow shell never delays the window.
    std::thread::spawn(|| {
        if let Some(path) = retrogit::env_path::login_shell_path(std::time::Duration::from_secs(10))
        {
            gitcore::set_git_search_path(path);
        }
    });
    if let Err(e) = gitcore::configure_network_timeouts() {
        log::warn!("could not set git network timeouts: {e}");
    }

    let config_path = Config::default_path();
    let config = config_path
        .as_deref()
        .map(Config::load_from)
        .unwrap_or_default();

    let mut viewport = egui::ViewportBuilder::default()
        .with_title(strings::APP_NAME)
        .with_decorations(false)
        .with_resizable(true)
        .with_min_inner_size([520.0, 360.0])
        .with_inner_size([900.0, 600.0]);
    if let Some(g) = config.window {
        viewport = viewport.with_inner_size([g.width.max(520.0), g.height.max(360.0)]);
        if let (Some(x), Some(y)) = (g.x, g.y) {
            viewport = viewport.with_position([x, y]);
        }
    }
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    eframe::run_native(
        strings::APP_NAME,
        options,
        Box::new(move |cc| {
            win95::theme::install(&cc.egui_ctx);
            let repaint = cc.egui_ctx.clone();
            let deps = WorkerDeps {
                client: github::Client::github_com(),
                store: Arc::new(github::KeyringStore::new("RetroGit", "github.com")),
                client_id: GITHUB_CLIENT_ID.to_string(),
                commit_backend: gitcore::CommitBackend::PreferCli,
            };
            let worker = spawn(deps, move || repaint.request_repaint());
            Ok(Box::new(RetroGitApp::new(
                AppState::new(config),
                worker,
                config_path,
                cc.egui_ctx.clone(),
            )))
        }),
    )
}
