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
    // RETROGIT_DATA_DIR overrides the data folder (separate profile, tests).
    let data_dir = std::env::var_os("RETROGIT_DATA_DIR")
        .map(std::path::PathBuf::from)
        .or_else(|| dirs::data_local_dir().map(|d| d.join("RetroGit")));
    let initial = match retrogit::cli::parse(&std::env::args().collect::<Vec<_>>()) {
        retrogit::cli::Launch::Cli { target } => return run_cli(&target, data_dir.as_deref()),
        retrogit::cli::Launch::Gui { open } => open,
        // A clicked notification link goes the way of a folder: to the open window if any.
        retrogit::cli::Launch::Link { link } => Some(std::path::PathBuf::from(link)),
    };
    // `--open` (also used by the command when it starts a window): join a window that is
    // already open instead of starting a second one.
    if let (Some(path), Some(dir)) = (&initial, &data_dir)
        && retrogit::instance::send(dir, path).is_ok()
    {
        return Ok(());
    }
    if let Some(dir) = &data_dir {
        logging::init(&dir.join("retrogit.log"));
    }
    logging::install_panic_hook();
    // Before the window: the click that launched the app must find its handler.
    #[cfg(target_os = "macos")]
    retrogit::notify::macos::init();
    #[cfg(windows)]
    std::thread::spawn(retrogit::notify::winreg::register);
    log::info!("RetroGit {} starting", retrogit::version::version());
    if let Ok(exe) = std::env::current_exe() {
        gitcore::set_askpass_program(exe);
    }
    // Resolved in the background so a slow shell never delays the window.
    std::thread::spawn(|| {
        if let Some(path) = retrogit::env_path::login_shell_path(std::time::Duration::from_secs(10))
        {
            retrogit::env_path::set_tool_path(path.clone());
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
        .with_icon(std::sync::Arc::new(retrogit::ui::logo::window_icon()))
        .with_min_inner_size(retrogit::config::MIN_WINDOW)
        .with_inner_size([900.0, 600.0]);
    if let Some(g) = config.window {
        viewport = viewport.with_inner_size([
            g.width.max(retrogit::config::MIN_WINDOW[0]),
            g.height.max(retrogit::config::MIN_WINDOW[1]),
        ]);
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
            let tokens = github::TokenProvider::from_gh(std::sync::Arc::new(|login| {
                let token = github::gh_auth_token_checked(
                    retrogit::env_path::tool_path().as_deref(),
                    Some(login),
                );
                if let github::GhToken::Token(t) = &token {
                    logging::add_secret(t);
                }
                token
            }));
            let deps = WorkerDeps {
                client: github::Client::github_com(),
                store: Arc::new(github::KeyringAccounts::new("RetroGit")),
                known_accounts: config.accounts.clone(),
                repo_accounts: config.repo_accounts.clone(),
                client_id: GITHUB_CLIENT_ID.to_string(),
                commit_backend: gitcore::CommitBackend::PreferCli,
                tokens: tokens.clone(),
            };
            let worker = spawn(deps, move || repaint.request_repaint());
            let state = AppState::new(config);
            let app = RetroGitApp::new(state, worker, config_path, cc.egui_ctx.clone());
            let app = app
                .with_instance(data_dir.as_deref(), cc.egui_ctx.clone(), initial)
                .with_pr_watch(github::Client::github_com(), tokens, cc.egui_ctx.clone())
                .with_updates("https://api.github.com", cc.egui_ctx.clone());
            Ok(Box::new(app))
        }),
    )
}

/// `retrogit [path]` from a terminal: hand the repository to the running window, or start
/// one (detached, so the terminal gets its prompt back).
fn run_cli(target: &std::path::Path, data_dir: Option<&std::path::Path>) -> eframe::Result {
    let folder = match retrogit::cli::repo_root(target) {
        Ok(root) => root,
        Err(e) => {
            eprintln!("retrogit: {e}");
            // Windows: no console to print to; let the window show the error instead.
            if cfg!(windows) {
                target.to_path_buf()
            } else {
                std::process::exit(1);
            }
        }
    };
    if let Some(dir) = data_dir
        && retrogit::instance::send(dir, &folder).is_ok()
    {
        return Ok(());
    }
    let Ok(exe) = std::env::current_exe() else {
        std::process::exit(1)
    };
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("--open")
        .arg(&folder)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0); // not killed with the terminal
    }
    if let Err(e) = cmd.spawn() {
        eprintln!("{} {e}", strings::ERR_CLI_START);
        std::process::exit(1);
    }
    Ok(())
}
