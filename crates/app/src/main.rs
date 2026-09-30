#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::sync::Arc;

use retrogit::app::RetroGitApp;
use retrogit::config::Config;
use retrogit::state::AppState;
use retrogit::worker::{WorkerDeps, spawn};
use retrogit::{GITHUB_CLIENT_ID, logging, strings};

fn main() -> eframe::Result {
    if let Some(dir) = dirs::data_local_dir() {
        logging::init(&dir.join("RetroGit").join("retrogit.log"));
    }
    log::info!("RetroGit {} starting", env!("CARGO_PKG_VERSION"));

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
            };
            let worker = spawn(deps, move || repaint.request_repaint());
            Ok(Box::new(RetroGitApp::new(
                AppState::new(config),
                worker,
                config_path,
            )))
        }),
    )
}
