//! GitHub specifics: which github.com repository `origin` is, and pull request heads.

use std::sync::atomic::AtomicBool;

use crate::net::NetAuth;
use crate::remote::retry_without_token;
use crate::{GitError, Repo};

/// `(owner, repo)` of a github.com remote URL (HTTPS, `git@github.com:`, `ssh://`).
pub fn parse_github_slug(url: &str) -> Option<(String, String)> {
    let url = url.trim();
    let lower = url.to_ascii_lowercase();
    let rest = ["https://", "http://", "ssh://", "git://"]
        .iter()
        .find_map(|scheme| lower.strip_prefix(scheme).map(|_| &url[scheme.len()..]))
        .map(|after| {
            // Drop credentials (`user@`) and the port.
            let after = after.rsplit_once('@').map_or(after, |(_, h)| h);
            let (host, path) = after.split_once('/')?;
            let host = host.split(':').next().unwrap_or(host);
            host.eq_ignore_ascii_case("github.com").then_some(path)
        })
        .unwrap_or_else(|| {
            // scp-like: git@github.com:owner/repo.git
            let (user_host, path) = url.split_once(':')?;
            let host = user_host.rsplit_once('@').map_or(user_host, |(_, h)| h);
            host.eq_ignore_ascii_case("github.com").then_some(path)
        })?;
    let mut parts = rest.trim_matches('/').split('/');
    let owner = parts.next()?.to_string();
    let repo = parts.next()?.trim_end_matches(".git").to_string();
    if parts.next().is_some() || owner.is_empty() || repo.is_empty() {
        return None;
    }
    Some((owner, repo))
}

impl Repo {
    /// `(owner, repo)` if `origin` is a github.com repository.
    pub fn github_slug(&self) -> Option<(String, String)> {
        parse_github_slug(&self.origin_url()?)
    }

    fn rev(&self, name: &str) -> Option<String> {
        self.git()
            .revparse_single(name)
            .ok()
            .map(|o| o.id().to_string())
    }

    /// HEAD is `commit` or a descendant of it (local commits on top).
    pub fn head_descends_from(&self, commit: &str) -> bool {
        self.rev("HEAD")
            .is_some_and(|h| h == commit || self.is_ancestor(commit, &h))
    }

    fn is_ancestor(&self, ancestor: &str, of: &str) -> bool {
        self.run_git(&["merge-base", "--is-ancestor", ancestor, of])
            .is_ok_and(|o| o.success)
    }

    /// Fast-forward the current branch to `onto` (e.g. `origin/feature`) when it has no
    /// commits of its own. Returns whether it moved.
    pub fn fast_forward(&self, onto: &str) -> Result<bool, GitError> {
        let (Some(head), Some(target)) = (self.rev("HEAD"), self.rev(onto)) else {
            return Ok(false);
        };
        if head == target || !self.is_ancestor(&head, &target) {
            return Ok(false);
        }
        self.git_ok(&["merge", "--ff-only", onto])?;
        Ok(true)
    }

    /// Fetch the head of pull request `number` (works for forks: `refs/pull/N/head`) into
    /// the local branch `pr/N`, without switching to it. An existing `pr/N` moves forward;
    /// after a force-push it follows the pull request only if it has no commits of its own.
    /// Returns the branch name.
    pub fn fetch_pull(
        &self,
        number: u64,
        auth: &NetAuth,
        cancel: &AtomicBool,
    ) -> Result<String, GitError> {
        let branch = format!("pr/{number}");
        let fetched = format!("refs/retrogit/pull/{number}");
        let before = self.rev(&fetched);
        let spec = format!("+refs/pull/{number}/head:{fetched}");
        let args = ["fetch", "--no-tags", "--progress", "origin", spec.as_str()];
        retry_without_token(auth, |a| self.run_net(a, &args, |_| {}, cancel))?;
        let new = self
            .rev(&fetched)
            .ok_or_else(|| GitError::Other(format!("pull request #{number} was not fetched")))?;
        let local_ref = format!("refs/heads/{branch}");
        let Some(local) = self.rev(&local_ref) else {
            self.git_ok(&["branch", &branch, &new])?;
            return Ok(branch);
        };
        if local == new {
            return Ok(branch);
        }
        let forward = self.is_ancestor(&local, &new);
        let untouched = before.as_deref() == Some(local.as_str());
        if !forward && !untouched {
            // Local commits on pr/N: never drop them.
            return Ok(branch);
        }
        let checked_out = self.current_branch().is_some_and(|b| b.name == branch);
        if !checked_out {
            self.git_ok(&["branch", "-f", &branch, &new])?;
        } else if forward {
            self.git_ok(&["merge", "--ff-only", &new])?;
        } else {
            // Force-pushed pull request, no local work: follow it, keeping local changes.
            self.git_ok(&["reset", "--keep", &new])?;
        }
        Ok(branch)
    }
}

#[cfg(test)]
mod tests {
    use super::parse_github_slug;

    #[test]
    fn slugs_from_every_url_form() {
        let ok = Some(("Owner".to_string(), "Repo".to_string()));
        for url in [
            "https://github.com/Owner/Repo.git",
            "https://github.com/Owner/Repo",
            "https://github.com/Owner/Repo/",
            "https://x-access-token@github.com/Owner/Repo.git",
            "HTTPS://GitHub.com/Owner/Repo",
            "git@github.com:Owner/Repo.git",
            "git@github.com:Owner/Repo",
            "ssh://git@github.com/Owner/Repo.git",
            "ssh://git@github.com:22/Owner/Repo",
            "  https://github.com/Owner/Repo.git\n",
        ] {
            assert_eq!(parse_github_slug(url), ok, "{url}");
        }
        for url in [
            "https://gitlab.com/Owner/Repo.git",
            "https://github.company.com/Owner/Repo",
            "git@gitlab.com:Owner/Repo.git",
            "https://github.com/Owner",
            "https://github.com/Owner/Repo/tree/main",
            "/local/path/repo",
            "",
        ] {
            assert_eq!(parse_github_slug(url), None, "{url}");
        }
    }
}
