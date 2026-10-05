//! Which token a GitHub call uses: RetroGit's own, or the GitHub CLI's (`gh`) for
//! organizations that block RetroGit's OAuth App but have approved `gh`.

use std::collections::HashMap;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::GithubError;

/// After this long, a restricted owner is tried again with RetroGit's token (it may have
/// approved RetroGit meanwhile).
pub const RETRY_RESTRICTED: Duration = Duration::from_secs(30 * 60);
/// How long a `gh` answer is reused before `gh` runs again.
pub const GH_CACHE: Duration = Duration::from_secs(5 * 60);

/// Gives the `gh` token of a GitHub login on demand (`None`: `gh` missing, or it does not
/// know that account).
pub type GhTokenSource = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// What `gh` answered for an account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GhToken {
    Token(String),
    /// `gh` missing, or it does not know that account.
    Missing,
    /// `gh` is too old to choose the account (`--user` is unknown before 2.40).
    TooOld,
}

/// Gives what `gh` answers for a GitHub login.
pub type GhSource = Arc<dyn Fn(&str) -> GhToken + Send + Sync>;

/// The current time (injected by tests).
pub type Clock = Arc<dyn Fn() -> Instant + Send + Sync>;

/// When `gh` answered, and its token.
type GhAnswer = (Instant, Option<String>);

/// Picks the token of a call: the account's RetroGit token, or the GitHub CLI's token of
/// the same account for organizations that restrict OAuth Apps. Restricted owners are
/// remembered per account, and tried again with RetroGit's token after
/// [`RETRY_RESTRICTED`].
#[derive(Clone)]
pub struct TokenProvider {
    gh: GhSource,
    clock: Clock,
    /// `(login, owner)`, lowercase, and when the restriction was learned.
    restricted: Arc<Mutex<HashMap<(String, String), Instant>>>,
    /// `gh`'s last answer per login (lowercase), and when.
    gh_cache: Arc<Mutex<HashMap<String, GhAnswer>>>,
    /// `gh` last said it is too old to choose the account.
    gh_too_old: Arc<AtomicBool>,
}

impl std::fmt::Debug for TokenProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokenProvider").finish_non_exhaustive()
    }
}

impl TokenProvider {
    pub fn new(gh: GhTokenSource) -> TokenProvider {
        TokenProvider::from_gh(Arc::new(move |login| match gh(login) {
            Some(t) => GhToken::Token(t),
            None => GhToken::Missing,
        }))
    }

    /// A provider whose `gh` source also tells when `gh` is too old.
    pub fn from_gh(gh: GhSource) -> TokenProvider {
        TokenProvider {
            gh,
            clock: Arc::new(Instant::now),
            restricted: Arc::new(Mutex::new(HashMap::new())),
            gh_cache: Arc::new(Mutex::new(HashMap::new())),
            gh_too_old: Arc::new(AtomicBool::new(false)),
        }
    }

    /// The same provider reading the time from `clock` (tests).
    pub fn with_clock(mut self, clock: Clock) -> TokenProvider {
        self.clock = clock;
        self
    }

    /// No `gh` fallback (tests, or when the CLI must not be used).
    pub fn without_gh() -> TokenProvider {
        TokenProvider::new(Arc::new(|_| None))
    }

    /// The `gh` token of account `login`, if `gh` has it (`gh`'s answer is reused for
    /// [`GH_CACHE`]).
    pub fn gh_token_for(&self, login: &str) -> Option<String> {
        let key = login.to_lowercase();
        let now = (self.clock)();
        if let Ok(cache) = self.gh_cache.lock()
            && let Some((at, token)) = cache.get(&key)
            && now.saturating_duration_since(*at) < GH_CACHE
        {
            return token.clone();
        }
        let answer = (self.gh)(login);
        let too_old = answer == GhToken::TooOld;
        self.gh_too_old.store(too_old, Ordering::Relaxed);
        let token = match answer {
            GhToken::Token(t) => Some(t),
            GhToken::Missing | GhToken::TooOld => None,
        };
        // A gh too old is not remembered: once the user updates it, the next try sees it.
        if !too_old && let Ok(mut cache) = self.gh_cache.lock() {
            cache.insert(key, (now, token.clone()));
        }
        token
    }

    /// `gh` last said it is too old to choose the account: the user must update it.
    pub fn gh_too_old(&self) -> bool {
        self.gh_too_old.load(Ordering::Relaxed)
    }

    /// GitHub refused `login`'s `gh` token: ask `gh` again next time.
    pub fn gh_rejected(&self, login: &str) {
        if let Ok(mut cache) = self.gh_cache.lock() {
            cache.remove(&login.to_lowercase());
        }
    }

    /// Forget every restricted owner and every `gh` answer (a new sign-in or a check of the
    /// accounts: RetroGit may have been approved, `gh` installed or signed in since).
    pub fn forget_restricted(&self) {
        if let Ok(mut s) = self.restricted.lock() {
            s.clear();
        }
        if let Ok(mut cache) = self.gh_cache.lock() {
            cache.clear();
        }
    }

    fn key(login: &str, owner: &str) -> (String, String) {
        (login.to_lowercase(), owner.to_lowercase())
    }

    /// `owner` refused `login`'s RetroGit token (OAuth App restriction) this session.
    pub fn is_restricted(&self, login: &str, owner: &str) -> bool {
        self.restricted
            .lock()
            .map(|s| s.contains_key(&Self::key(login, owner)))
            .unwrap_or(false)
    }

    /// Restricted, and learned less than [`RETRY_RESTRICTED`] ago.
    fn restricted_recently(&self, login: &str, owner: &str) -> bool {
        let now = (self.clock)();
        self.restricted
            .lock()
            .ok()
            .and_then(|s| s.get(&Self::key(login, owner)).copied())
            .is_some_and(|at| now.saturating_duration_since(at) < RETRY_RESTRICTED)
    }

    fn unrestrict(&self, login: &str, owner: &str) {
        if let Ok(mut s) = self.restricted.lock() {
            s.remove(&Self::key(login, owner));
        }
    }

    /// Remember that `owner` restricts `login`'s RetroGit token (also learned when only the
    /// GitHub CLI's token lists its repositories).
    pub fn remember(&self, login: &str, owner: &str) {
        if let Ok(mut s) = self.restricted.lock() {
            s.insert(Self::key(login, owner), (self.clock)());
        }
    }

    /// Forget what was learned about `login` (account removed or replaced).
    pub fn forget_account(&self, login: &str) {
        let login = login.to_lowercase();
        if let Ok(mut s) = self.restricted.lock() {
            s.retain(|(l, _), _| *l != login);
        }
        if let Ok(mut cache) = self.gh_cache.lock() {
            cache.remove(&login);
        }
    }

    /// Run `call` for a repository of `owner` as account `login`: with its RetroGit
    /// `token` first; if the organization restricts OAuth Apps (or hides the repository),
    /// once more with `gh`'s token of the same account, and use it directly for that
    /// owner from then on (RetroGit's token is tried again after [`RETRY_RESTRICTED`],
    /// and the restriction forgotten when it works). Without a usable `gh` token, the first error is returned. A
    /// refused `gh` token is reported as the restriction, never as `Unauthorized` (callers
    /// sign the account out on `Unauthorized`).
    pub fn with_token<T>(
        &self,
        login: &str,
        token: &str,
        owner: &str,
        call: impl Fn(&str) -> Result<T, GithubError>,
    ) -> Result<T, GithubError> {
        if self.restricted_recently(login, owner)
            && let Some(gh) = self.gh_token_for(login)
        {
            return call(&gh).map_err(|e| match e {
                GithubError::Unauthorized => {
                    self.gh_rejected(login);
                    GithubError::OAuthRestricted {
                        org: Some(owner.to_string()),
                    }
                }
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
                        GithubError::Unauthorized => {
                            self.gh_rejected(login);
                            first
                        }
                        e if hidden_by_restriction(&e) => first,
                        other => other,
                    })
                }
                None => Err(first),
            },
            Ok(v) => {
                if self.is_restricted(login, owner) {
                    self.unrestrict(login, owner);
                }
                Ok(v)
            }
            other => other,
        }
    }
}

/// Errors an organization restricting OAuth Apps produces: the explicit restriction, or
/// the repository looking missing (GraphQL `NOT_FOUND`, REST 404).
pub fn hidden_by_restriction(e: &GithubError) -> bool {
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

/// `gh` refused `--user`: it is older than 2.40 and cannot choose the account.
pub fn gh_stderr_too_old(stderr: &str) -> bool {
    stderr.to_lowercase().contains("unknown flag: --user")
}

/// Ask the GitHub CLI for its github.com token of account `user` (RetroGit never acts as
/// another account), or of its active account when no `user` is given. `path` replaces PATH (apps started from the Finder get a
/// minimal one). The caller's `GH_TOKEN` / `GITHUB_TOKEN` are not passed on, so `gh`
/// answers from its own login. Blocking (runs a process).
pub fn gh_auth_token(path: Option<&str>, user: Option<&str>) -> Option<String> {
    match gh_auth_token_checked(path, user) {
        GhToken::Token(t) => Some(t),
        GhToken::Missing | GhToken::TooOld => None,
    }
}

/// [`gh_auth_token`], telling a `gh` too old to choose the account from a missing one.
pub fn gh_auth_token_checked(path: Option<&str>, user: Option<&str>) -> GhToken {
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
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let Ok(out) = cmd.output() else {
        return GhToken::Missing;
    };
    if !out.status.success() {
        return if user.is_some() && gh_stderr_too_old(&String::from_utf8_lossy(&out.stderr)) {
            GhToken::TooOld
        } else {
            GhToken::Missing
        };
    }
    parse_gh_token(&String::from_utf8_lossy(&out.stdout)).map_or(GhToken::Missing, GhToken::Token)
}
