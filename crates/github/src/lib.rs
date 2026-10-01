//! GitHub access for RetroGit: OAuth Device Flow, REST API, token storage.

mod client;
mod device_flow;
mod error;
mod graphql;
mod link;
mod token;
mod token_store;

pub use client::{Client, RepoInfo, RepoListing, User};
pub use device_flow::{DeviceCode, DeviceFlow, DeviceFlowFailure, PollResponse, Step};
pub use error::GithubError;
pub use graphql::GraphqlResponse;
pub use link::next_link;
pub use token::{GhTokenSource, TokenProvider, gh_auth_token, parse_gh_token};
pub use token_store::{KeyringStore, MemoryStore, TokenStore, TokenStoreError};

/// OAuth scopes requested by RetroGit.
pub const SCOPES: &[&str] = &["repo", "read:org"];
