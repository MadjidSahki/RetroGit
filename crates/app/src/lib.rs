//! RetroGit application: state, background worker and screens.

pub mod app;
pub mod config;
pub mod format;
pub mod logging;
pub mod protocol;
pub mod state;
pub mod strings;
pub mod ui;
pub mod worker;

/// OAuth App client ID (public, no secret). Paste yours here, or build with
/// `RETROGIT_GITHUB_CLIENT_ID=Ov23li... cargo build`.
pub const GITHUB_CLIENT_ID: &str = match option_env!("RETROGIT_GITHUB_CLIENT_ID") {
    Some(id) => id,
    None => "Ov23liy76k7uO4WBEEci",
};
