//! RetroGit application: state, background worker and screens.

pub mod app;
pub mod cli;
pub mod config;
pub mod env_path;
pub mod format;
pub mod highlight;
pub mod ide;
pub mod instance;
pub mod logging;
pub mod protocol;
pub mod state;
pub mod strings;
pub mod ui;
pub mod watch;
pub mod worker;

/// OAuth App client ID (public, no secret). Paste yours here, or build with
/// `RETROGIT_GITHUB_CLIENT_ID=Ov23li... cargo build`.
pub const GITHUB_CLIENT_ID: &str = match option_env!("RETROGIT_GITHUB_CLIENT_ID") {
    Some(id) => id,
    None => "Ov23liN6gBDPqziTh3qi",
};
