//! RetroGit's internal Git API. No `git2` type is ever exposed publicly.

mod clone;
mod error;
mod repo;

pub use clone::{CloneProgress, CloneRequest, Credentials, clone};
pub use error::GitError;
pub use repo::{CommitInfo, Head, Repo, RepoSummary};
