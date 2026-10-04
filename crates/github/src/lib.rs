//! GitHub access for RetroGit: OAuth Device Flow, REST API, token storage.

mod accounts;
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

pub use accounts::{
    Account, AccountStatus, AccountStore, Accounts, KeyringAccounts, MemoryAccounts, RepoAccount,
    choose_account,
};
pub use client::{Client, RepoInfo, RepoListing, User};
pub use device_flow::{DeviceCode, DeviceFlow, DeviceFlowFailure, PollResponse, Step};
pub use error::{GithubError, repository_missing};
pub use graphql::GraphqlResponse;
pub use link::next_link;
pub use pulls::{
    CheckRun, CheckStatus, ChecksState, DETAIL_PAGE, DiffSide, Label, MergeMethod, Mergeable,
    PrCommit, PrDetail, PrFile, PrFilter, PrState, PrSummary, ReviewDecision, ReviewState,
    ReviewThread, Reviewer, ThreadComment, TimelineItem, parse_color, pulls_web_url, search_query,
};
pub use pulls_write::{
    LineComment, Merge, NewPull, RepoMeta, Review, ReviewEvent, diff_lists, encode_segment,
    suggestion_block, suggestions,
};
pub use token::{
    GhTokenSource, TokenProvider, gh_auth_token, hidden_by_restriction, parse_gh_token,
};
pub use token_store::{
    KeyringStore, MemoryStore, SECURITY_MARKER, TokenStore, TokenStoreError, keep_after_migration,
    parse_security_comment, security_add_command,
};
pub use watch::{PrEvent, PrEventKind, PrSnapshot, date_days_before, diff_snapshots};

/// OAuth scopes requested by RetroGit.
/// `workflow`: pushes that change `.github/workflows` are refused without it.
pub const SCOPES: &[&str] = &["repo", "read:org", "workflow"];
