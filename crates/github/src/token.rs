//! Which token a GitHub call uses: RetroGit's own, or the GitHub CLI's (`gh`) for
//! organizations that block RetroGit's OAuth App but have approved `gh`.

use std::collections::HashSet;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

use crate::GithubError;

/// Gives the `gh` token on demand, preferably for the given login (`None`: `gh` missing
/// or not signed in).
pub type GhTokenSource = Arc<dyn Fn(Option<&str>) -> Option<String> + Send + Sync>;

/// Picks the token per repository owner and remembers, for the session, the owners for
/// which RetroGit's token was refused because of OAuth App restrictions.
#[derive(Clone)]
pub struct TokenProvider {
    gh: GhTokenSource,
    restricted: Arc<Mutex<HashSet<String>>>,
    /// Signed-in GitHub login: `gh` is asked for that account first.
    login: Arc<Mutex<Option<String>>>,
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
            login: Arc::new(Mutex::new(None)),
        }
    }

    /// No `gh` fallback (tests, or when the CLI must not be used).
    pub fn without_gh() -> TokenProvider {
        TokenProvider::new(Arc::new(|_| None))
    }

    /// Remember who is signed in (`None` after signing out).
    pub fn set_login(&self, login: Option<&str>) {
        if let Ok(mut l) = self.login.lock() {
            *l = login.map(str::to_string);
        }
    }

    /// The `gh` token, if any.
    pub fn gh_token(&self) -> Option<String> {
        let login = self.login.lock().ok().and_then(|l| l.clone());
        (self.gh)(login.as_deref())
    }

    fn is_restricted(&self, owner: &str) -> bool {
        self.restricted
            .lock()
            .map(|s| s.contains(&owner.to_lowercase()))
            .unwrap_or(false)
    }

    fn remember(&self, owner: &str) {
        if let Ok(mut s) = self.restricted.lock() {
            s.insert(owner.to_lowercase());
        }
    }

    /// Run `call` for a repository of `owner`: with `primary` (RetroGit's token) first; if
    /// the organization restricts OAuth Apps, once more with the `gh` token, and use `gh`
    /// directly for that owner from then on. Without a usable `gh` token, the first error
    /// is returned. Other errors are never retried.
    pub fn with_token<T>(
        &self,
        primary: Option<&str>,
        owner: &str,
        call: impl Fn(&str) -> Result<T, GithubError>,
    ) -> Result<T, GithubError> {
        if (primary.is_none() || self.is_restricted(owner))
            && let Some(gh) = self.gh_token()
        {
            return call(&gh);
        }
        let Some(primary) = primary else {
            return Err(GithubError::Unauthorized);
        };
        match call(primary) {
            Err(first @ GithubError::OAuthRestricted { .. }) => match self.gh_token() {
                Some(gh) => {
                    let r = call(&gh);
                    if r.is_ok() {
                        self.remember(owner);
                    }
                    r.map_err(|e| match e {
                        GithubError::Unauthorized | GithubError::OAuthRestricted { .. } => first,
                        other => other,
                    })
                }
                None => Err(first),
            },
            other => other,
        }
    }
}

/// First non-empty line of `gh auth token`'s output.
pub fn parse_gh_token(stdout: &str) -> Option<String> {
    stdout
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.contains(' '))
        .map(str::to_string)
}

/// Ask the GitHub CLI for its github.com token: for account `user` if `gh` knows it, else
/// for its active account. `path` replaces PATH (apps started from the Finder get a
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
    user.and_then(|u| run(Some(u))).or_else(|| run(None))
}
