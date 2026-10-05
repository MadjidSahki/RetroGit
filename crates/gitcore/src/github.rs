//! GitHub specifics: which github.com repository `origin` is, and pull request heads.

use std::sync::atomic::AtomicBool;

use crate::net::NetAuth;
use crate::remote::retry_without_token;
use crate::{GitError, NetProgress, Refusal, Repo};

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

    /// Whether commit `id` is on the current branch's upstream (already pushed).
    pub fn is_pushed(&self, id: &str) -> bool {
        self.upstream_oid()
            .is_some_and(|up| up == id || self.is_ancestor(id, &up))
    }

    /// Whether moving the current branch to `id` drops commits already pushed: the pushed
    /// commits still on the branch end at the merge base of HEAD and its upstream.
    pub fn reset_drops_pushed(&self, id: &str) -> bool {
        self.upstream_oid()
            .and_then(|up| self.merge_base(&self.rev("HEAD")?, &up))
            .is_some_and(|base| base != id && !self.is_ancestor(&base, id))
    }

    fn merge_base(&self, a: &str, b: &str) -> Option<String> {
        let (a, b) = (git2::Oid::from_str(a).ok()?, git2::Oid::from_str(b).ok()?);
        self.git().merge_base(a, b).ok().map(|o| o.to_string())
    }

    /// Whether `ancestor` is `id` or one of its ancestors.
    pub fn is_ancestor_of(&self, ancestor: &str, id: &str) -> bool {
        ancestor == id || self.is_ancestor(ancestor, id)
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
    /// Returns the branch name, and whether commits of its own kept it from moving to
    /// the pull request's latest commit. A checked-out `pr/N` that local changes keep from
    /// moving: `WouldOverwrite` (or `Refused(LocalChangesFirst)`), after a successful fetch.
    pub fn fetch_pull(
        &self,
        number: u64,
        auth: &NetAuth,
        mut progress: impl FnMut(NetProgress),
        cancel: &AtomicBool,
    ) -> Result<(String, bool), GitError> {
        let branch = format!("pr/{number}");
        let fetched = format!("refs/retrogit/pull/{number}");
        let before = self.rev(&fetched);
        let spec = format!("+refs/pull/{number}/head:{fetched}");
        let args = ["fetch", "--no-tags", "--progress", "origin", spec.as_str()];
        retry_without_token(auth, |a| self.run_net(a, &args, &mut progress, cancel))?;
        let new = self
            .rev(&fetched)
            .ok_or(GitError::Refused(Refusal::PullNotFetched(number)))?;
        let local_ref = format!("refs/heads/{branch}");
        let Some(local) = self.rev(&local_ref) else {
            self.git_ok(&["branch", &branch, &new])?;
            return Ok((branch, false));
        };
        if local == new {
            return Ok((branch, false));
        }
        let forward = self.is_ancestor(&local, &new);
        let untouched = before.as_deref() == Some(local.as_str());
        if !forward && !untouched {
            // Local commits on pr/N: never drop them. Behind unless the pull request's
            // head is already part of them.
            return Ok((branch, !self.is_ancestor(&new, &local)));
        }
        let checked_out = self.current_branch().is_some_and(|b| b.name == branch);
        if !checked_out {
            self.git_ok(&["branch", "-f", &branch, &new])?;
            return Ok((branch, false));
        }
        let out = if forward {
            self.run_git(&["merge", "--ff-only", &new])?
        } else {
            // Force-pushed pull request, no local work: follow it, keeping local changes.
            self.run_git(&["reset", "--keep", &new])?
        };
        if !out.success {
            // Fetched, but local changes keep the checked-out branch from moving.
            let files = crate::parse_overwritten_files(&out.text);
            let lower = out.text.to_lowercase();
            return Err(if !files.is_empty() {
                GitError::WouldOverwrite { files }
            } else if lower.contains("not uptodate") || lower.contains("would be overwritten") {
                // `reset --keep`: "Entry 'f' not uptodate. Cannot merge."
                GitError::Refused(Refusal::LocalChangesFirst)
            } else {
                GitError::Other(out.text)
            });
        }
        Ok((branch, false))
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
