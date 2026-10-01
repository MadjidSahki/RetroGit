//! Which token a GitHub call uses: RetroGit's own, or the GitHub CLI's (`gh`) for
//! organizations that block RetroGit's OAuth App but have approved `gh`.

use std::collections::HashSet;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

use crate::GithubError;

/// Gives the `gh` token of a GitHub login on demand (`None`: `gh` missing, or it does not
/// know that account).
pub type GhTokenSource = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// Picks the token of a call: the account's RetroGit token, or the GitHub CLI's token of
/// the same account for organizations that restrict OAuth Apps. Restricted owners are
/// remembered per account for the session.
#[derive(Clone)]
pub struct TokenProvider {
    gh: GhTokenSource,
    /// `(login, owner)`, lowercase.
    restricted: Arc<Mutex<HashSet<(String, String)>>>,
}

impl std::fmt::Debug for TokenProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokenProvider").finish_non_exhaustive()
    }
}

impl TokenProvider {
    pub fn new(gh: GhTokenSource) -> TokenProvider {
        TokenProvider {
            gh,
            restricted: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    /// No `gh` fallback (tests, or when the CLI must not be used).
    pub fn without_gh() -> TokenProvider {
        TokenProvider::new(Arc::new(|_| None))
    }

    /// The `gh` token of account `login`, if `gh` has it.
    pub fn gh_token_for(&self, login: &str) -> Option<String> {
        (self.gh)(login)
    }

    fn key(login: &str, owner: &str) -> (String, String) {
        (login.to_lowercase(), owner.to_lowercase())
    }

    /// `owner` refused `login`'s RetroGit token (OAuth App restriction) this session.
    pub fn is_restricted(&self, login: &str, owner: &str) -> bool {
        self.restricted
            .lock()
            .map(|s| s.contains(&Self::key(login, owner)))
            .unwrap_or(false)
    }

    fn remember(&self, login: &str, owner: &str) {
        if let Ok(mut s) = self.restricted.lock() {
            s.insert(Self::key(login, owner));
        }
    }

    /// Forget what was learned about `login` (account removed or replaced).
    pub fn forget_account(&self, login: &str) {
        let login = login.to_lowercase();
        if let Ok(mut s) = self.restricted.lock() {
            s.retain(|(l, _)| *l != login);
        }
    }

    /// Run `call` for a repository of `owner` as account `login`: with its RetroGit
    /// `token` first; if the organization restricts OAuth Apps (or hides the repository),
    /// once more with `gh`'s token of the same account, and use it directly for that
    /// owner from then on. Without a usable `gh` token, the first error is returned. A
    /// refused `gh` token is reported as the restriction, never as `Unauthorized` (callers
    /// sign the account out on `Unauthorized`).
    pub fn with_token<T>(
        &self,
        login: &str,
        token: &str,
        owner: &str,
        call: impl Fn(&str) -> Result<T, GithubError>,
    ) -> Result<T, GithubError> {
        if self.is_restricted(login, owner)
            && let Some(gh) = self.gh_token_for(login)
        {
            return call(&gh).map_err(|e| match e {
                GithubError::Unauthorized => GithubError::OAuthRestricted {
                    org: Some(owner.to_string()),
                },
                other => other,
            });
        }
        match call(token) {
            Err(first) if hidden_by_restriction(&first) => match self.gh_token_for(login) {
                Some(gh) => {
                    let r = call(&gh);
                    if r.is_ok() {
                        self.remember(login, owner);
                    }
                    // gh cannot see it either: RetroGit's answer is the one to explain.
                    r.map_err(|e| match e {
                        GithubError::Unauthorized => first,
                        e if hidden_by_restriction(&e) => first,
                        other => other,
                    })
                }
                None => Err(first),
            },
            other => other,
        }
    }
}

/// Errors an organization restricting OAuth Apps produces: the explicit restriction, or
/// the repository looking missing (GraphQL `NOT_FOUND`, REST 404).
fn hidden_by_restriction(e: &GithubError) -> bool {
    matches!(
        e,
        GithubError::OAuthRestricted { .. }
            | GithubError::NotFound(_)
            | GithubError::Http(404)
            | GithubError::Rejected { status: 404, .. }
    )
}

/// First non-empty line of `gh auth token`'s output.
pub fn parse_gh_token(stdout: &str) -> Option<String> {
    stdout
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.contains(' '))
        .map(str::to_string)
}

/// Ask the GitHub CLI for its github.com token of account `user` (RetroGit never acts as
/// another account), or of its active account when no `user` is given. `path` replaces PATH (apps started from the Finder get a
/// minimal one). The caller's `GH_TOKEN` / `GITHUB_TOKEN` are not passed on, so `gh`
/// answers from its own login. Blocking (runs a process).
pub fn gh_auth_token(path: Option<&str>, user: Option<&str>) -> Option<String> {
    let run = |user: Option<&str>| -> Option<String> {
        let mut cmd = Command::new("gh");
        cmd.args(["auth", "token", "--hostname", "github.com"]);
        if let Some(u) = user {
            cmd.args(["--user", u]);
        }
        if let Some(p) = path {
            cmd.env("PATH", p);
        }
        cmd.env_remove("GH_TOKEN")
            .env_remove("GITHUB_TOKEN")
            .env_remove("GH_ENTERPRISE_TOKEN")
            .stdin(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        let out = cmd.output().ok()?;
        if !out.status.success() {
            return None;
        }
        parse_gh_token(&String::from_utf8_lossy(&out.stdout))
    };
    run(user)
}
