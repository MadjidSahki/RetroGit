//! GitHub access for RetroGit: OAuth Device Flow, REST API, token storage.

mod client;
mod device_flow;
mod error;
mod graphql;
mod link;
mod pulls;
mod pulls_write;
mod token;
mod token_store;
mod watch;

pub use client::{Client, RepoInfo, RepoListing, User};
pub use device_flow::{DeviceCode, DeviceFlow, DeviceFlowFailure, PollResponse, Step};
pub use error::{GithubError, repository_missing};
pub use graphql::GraphqlResponse;
pub use link::next_link;
pub use pulls::{
    CheckRun, CheckStatus, ChecksState, DiffSide, Label, MergeMethod, Mergeable, PrCommit,
    PrDetail, PrFile, PrFilter, PrState, PrSummary, ReviewDecision, ReviewState, ReviewThread,
    ThreadComment, TimelineItem, parse_color, search_query,
};
pub use pulls_write::{LineComment, Merge, NewPull, RepoMeta, Review, ReviewEvent, encode_segment};
pub use token::{GhTokenSource, TokenProvider, gh_auth_token, parse_gh_token};
pub use token_store::{KeyringStore, MemoryStore, TokenStore, TokenStoreError};
pub use watch::{PrEvent, PrEventKind, PrSnapshot, date_days_before, diff_snapshots};

/// OAuth scopes requested by RetroGit.
pub const SCOPES: &[&str] = &["repo", "read:org"];
