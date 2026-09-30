//! GitHub access for RetroGit: OAuth Device Flow, REST API, token storage.

mod client;
mod device_flow;
mod error;
mod link;
mod token_store;

pub use client::{Client, RepoInfo, User};
pub use device_flow::{DeviceCode, DeviceFlow, DeviceFlowFailure, PollResponse, Step};
pub use error::GithubError;
pub use link::next_link;
pub use token_store::{KeyringStore, MemoryStore, TokenStore, TokenStoreError};

/// OAuth scopes requested by RetroGit.
pub const SCOPES: &[&str] = &["repo", "read:org"];
