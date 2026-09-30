//! GitHub access for RetroGit: OAuth Device Flow, REST API, token storage.

mod device_flow;
mod error;
mod link;

pub use device_flow::{DeviceCode, DeviceFlow, DeviceFlowFailure, PollResponse, Step};
pub use error::GithubError;
pub use link::next_link;

/// OAuth scopes requested by RetroGit.
pub const SCOPES: &[&str] = &["repo", "read:org"];
