//! RetroGit's internal Git API. No `git2` type is ever exposed publicly.

mod error;
mod repo;

pub use error::GitError;
pub use repo::{CommitInfo, Head, Repo, RepoSummary};
