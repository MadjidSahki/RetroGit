//! Worker side of several accounts: signing in and out, checking them at startup
//! (migrating the single account of older versions), and choosing the account of a
//! repository for every GitHub and Git operation.

use std::time::{Duration, Instant};

use gitcore::{CloneRequest, Credentials, GitError, NetAuth};
use github::{Account, GithubError, RepoAccount, RepoInfo, User, choose_account};

use super::{Throttle, Worker};
use crate::logging;
use crate::protocol::{AppError, Event, Op, Severity, Slug};
use crate::strings as s;

/// Key of a repository in `repo_accounts`: `owner/repo`, lowercase.
pub fn repo_key(slug: &Slug) -> String {
    format!("{}/{}", slug.0, slug.1).to_lowercase()
}

/// Merge the repository lists of several accounts: one entry per repository (case-
/// insensitive), listing every account that sees it, most recently updated first.
pub fn merge_repo_lists(lists: Vec<(String, Vec<RepoInfo>)>) -> Vec<RepoInfo> {
    let mut out: Vec<RepoInfo> = Vec::new();
    for (login, repos) in lists {
        for r in repos {
            match out
                .iter_mut()
                .find(|o| o.full_name.eq_ignore_ascii_case(&r.full_name))
            {
                Some(o) => {
                    if !o.accounts.iter().any(|a| a.eq_ignore_ascii_case(&login)) {
                        o.accounts.push(login.clone());
                    }
                }
                None => out.push(RepoInfo {
                    accounts: vec![login.clone()],
                    ..r
                }),
            }
        }
    }
    out.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    out
}

/// A clone of a github.com repository (`slug_is_github`) that no signed-in account
/// (`tried`) could see, refused by the server: say so, with the accounts tried.
pub fn clone_refused(slug_is_github: bool, tried: &[String], e: &GitError) -> Option<AppError> {
    let refused = matches!(e, GitError::Auth(_) | GitError::AccessDenied(_));
    (slug_is_github && !tried.is_empty() && refused).then(|| {
        let mut error = AppError::new(Severity::Warning, s::ERR_NO_ACCOUNT_SEES_REPO);
        error.detail = Some(s::tried_accounts(tried));
        error
    })
}

/// A failed clone of a github.com repository (`slug_is_github`): the account chosen for it
/// that must sign in again (`unusable`, nothing else was tried), else `clone_refused`.
pub fn clone_error(
    slug_is_github: bool,
    unusable: Option<&str>,
    tried: &[String],
    e: &GitError,
) -> Option<AppError> {
    let refused = matches!(e, GitError::Auth(_) | GitError::AccessDenied(_));
    match unusable {
        Some(login) if slug_is_github && refused => Some(AppError::new(
            Severity::Warning,
            &s::ERR_ACCOUNT_MUST_SIGN_IN.replace("{login}", login),
        )),
        Some(_) => None,
        None => clone_refused(slug_is_github, tried, e),
    }
}

impl Worker {
    fn accounts_changed(&self) {
        self.emit(Event::AccountsChanged(self.accounts.statuses()));
    }

    /// Add (or refresh) a validated account and remember its organizations.
    fn add_account(&mut self, token: String, user: &User) {
        self.accounts.upsert(
            Account {
                login: user.login.clone(),
                token: token.clone(),
            },
            user.name.clone(),
        );
        if let Ok(orgs) = self.deps.client.user_orgs(&token) {
            self.accounts.set_orgs(&user.login, orgs);
        }
        self.no_account.clear();
    }

    /// Startup: move the token of older versions to its login, then check every account.
    pub(super) fn validate(&mut self) {
        self.deps.tokens.forget_restricted();
        self.recheck_unchecked();
        let mut known = self.deps.known_accounts.clone();
        let mut legacy_offline = false;
        match self.deps.store.load_legacy() {
            Ok(Some(token)) => {
                logging::add_secret(&token);
                match self.deps.client.current_user(&token) {
                    Ok(user) => {
                        let was_known = known.iter().any(|k| k.eq_ignore_ascii_case(&user.login));
                        let saved = matches!(self.deps.store.load(&user.login), Ok(Some(_)));
                        if was_known && saved {
                            // Moved by an earlier start (its token may have been renewed
                            // since): only the old slot is left to clear.
                            let _ = self.deps.store.clear_legacy();
                        } else {
                            // The old token is cleared only once the configuration lists
                            // its login (a later start): until then, it is its only trace.
                            if self.deps.store.save(&user.login, &token).is_ok() && was_known {
                                let _ = self.deps.store.clear_legacy();
                            }
                            if !was_known {
                                known.push(user.login.clone());
                            }
                            // Still usable this session even if it could not be moved.
                            self.add_account(token, &user);
                        }
                    }
                    Err(GithubError::Unauthorized) => {
                        let _ = self.deps.store.clear_legacy();
                    }
                    // Offline: keep it and try again next time (Sign in... retries).
                    Err(e) => {
                        log::info!("single-account token not migrated yet: {e}");
                        legacy_offline = true;
                    }
                }
            }
            Ok(None) => {}
            Err(e) => self.fail(Op::Auth, AppError::from_store(&e)),
        }
        let (mut first, mut offline) = (None, legacy_offline);
        for login in known {
            if self.accounts.get(&login).is_some() {
                first = first.or_else(|| {
                    Some(User {
                        login: login.clone(),
                        name: None,
                    })
                });
                continue;
            }
            let token = match self.deps.store.load(&login) {
                Ok(Some(t)) => t,
                Ok(None) => {
                    self.accounts.add_invalid(&login);
                    continue;
                }
                Err(e) => {
                    self.fail(Op::Auth, AppError::from_store(&e));
                    self.accounts.add_invalid(&login);
                    continue;
                }
            };
            logging::add_secret(&token);
            match self.deps.client.current_user(&token) {
                Ok(user) => {
                    self.add_account(token, &user);
                    first = first.or(Some(user));
                }
                Err(GithubError::Unauthorized) => {
                    let _ = self.deps.store.clear(&login);
                    self.accounts.add_invalid(&login);
                }
                Err(e) => {
                    // Keep it (the network may come back); its login is known, so the
                    // GitHub CLI fallback still asks for the right account.
                    log::info!("account {login} not checked: {e}");
                    self.unchecked.insert(login.clone());
                    self.accounts.upsert(Account { login, token }, None);
                    offline = true;
                }
            }
        }
        self.accounts_changed();
        match first {
            Some(user) => {
                self.offline = false;
                self.emit(Event::SignedIn(user));
            }
            None if offline => {
                self.offline = true;
                self.fail(
                    Op::Auth,
                    AppError::from_github(&GithubError::Network(String::new())),
                );
                self.emit(Event::Offline);
            }
            None => self.emit(Event::SignedOut),
        }
    }

    /// Check again the accounts kept offline (name, organizations, still valid).
    pub(super) fn recheck_unchecked(&mut self) {
        if self.unchecked.is_empty() {
            return;
        }
        let mut back: Option<User> = None;
        for login in std::mem::take(&mut self.unchecked) {
            let Some(account) = self.accounts.get(&login) else {
                continue;
            };
            match self.deps.client.current_user(&account.token) {
                Ok(user) => {
                    self.add_account(account.token, &user);
                    back = back.or(Some(user));
                }
                Err(GithubError::Unauthorized) => self.invalidate(&login, Op::Auth),
                Err(e) => {
                    log::info!("account {login} still not checked: {e}");
                    self.unchecked.insert(login);
                }
            }
        }
        if let Some(user) = back {
            self.accounts_changed();
            // Back online after an offline start: the app is signed in now.
            if std::mem::take(&mut self.offline) {
                self.emit(Event::SignedIn(user));
            }
        }
    }

    /// Validate `token` with `GET /user`, then store it as an account (new or refreshed).
    pub(super) fn sign_in_with(&mut self, token: String, is_pat: bool) {
        logging::add_secret(&token);
        self.deps.tokens.forget_restricted();
        match self.deps.client.current_user(&token) {
            Ok(user) => {
                if let Err(e) = self.deps.store.save(&user.login, &token) {
                    // Still signed in for this session; warn that it won't persist.
                    self.fail(Op::Auth, AppError::from_store(&e));
                }
                self.deps.tokens.forget_account(&user.login);
                self.add_account(token, &user);
                self.accounts_changed();
                self.offline = false;
                self.emit(Event::SignedIn(user));
                self.send_repo_account();
            }
            Err(GithubError::Unauthorized) if is_pat => {
                self.fail(
                    Op::Auth,
                    AppError::new(Severity::Warning, s::ERR_PAT_REJECTED),
                );
            }
            Err(e) => self.fail(Op::Auth, AppError::from_github(&e)),
        }
    }

    /// Forget `login` (and its token, and the repositories that use it).
    pub(super) fn remove_account(&mut self, login: &str) {
        // An account moved from older versions this session: its old slot goes too, or the
        // next start would sign it in again.
        let token = self.deps.store.load(login).ok().flatten();
        if token.is_some() && self.deps.store.load_legacy().ok().flatten() == token {
            let _ = self.deps.store.clear_legacy();
        }
        if let Err(e) = self.deps.store.clear(login) {
            self.fail(Op::Auth, AppError::from_store(&e));
        }
        self.accounts.remove(login);
        self.deps.tokens.forget_account(login);
        let gone: Vec<String> = self
            .repo_accounts
            .iter()
            .filter(|(_, r)| r.login.eq_ignore_ascii_case(login))
            .map(|(k, _)| k.clone())
            .collect();
        for key in gone {
            self.repo_accounts.remove(&key);
            self.emit(Event::RepoAccountLearned { key, account: None });
        }
        for logins in self.seen.values_mut() {
            logins.retain(|l| !l.eq_ignore_ascii_case(login));
        }
        self.accounts_changed();
        if self.accounts.list().is_empty() {
            self.emit(Event::SignedOut);
        }
        self.send_repo_account();
    }

    /// GitHub refused `login`'s token: that account only must sign in again.
    pub(super) fn invalidate(&mut self, login: &str, during: Op) {
        let _ = self.deps.store.clear(login);
        self.accounts.invalidate(login);
        self.accounts_changed();
        self.fail(
            during,
            AppError::new(
                Severity::Warning,
                &s::ERR_ACCOUNT_REJECTED.replace("{login}", login),
            ),
        );
        if self.accounts.list().is_empty() {
            self.emit(Event::SignedOut);
        }
        // Without probing: the other accounts are tried on the next GitHub call.
        self.send_repo_account_known();
    }

    fn learn(&mut self, slug: &Slug, login: &str) {
        let key = repo_key(slug);
        if self.repo_accounts.get(&key).is_some_and(|r| r.manual) {
            return;
        }
        let account = RepoAccount {
            login: login.to_string(),
            manual: false,
        };
        if self.repo_accounts.get(&key) != Some(&account) {
            self.repo_accounts.insert(key.clone(), account.clone());
            self.emit(Event::RepoAccountLearned {
                key,
                account: Some(account),
            });
        }
    }

    /// Login chosen by the user for `slug` whose account cannot be used (must sign in
    /// again): nothing else is used in its place.
    pub(super) fn unusable_choice(&self, slug: &Slug) -> Option<String> {
        let r = self.repo_accounts.get(&repo_key(slug))?;
        (r.manual && self.accounts.get(&r.login).is_none()).then(|| r.login.clone())
    }

    fn is_manual(&self, slug: &Slug) -> bool {
        self.repo_accounts
            .get(&repo_key(slug))
            .is_some_and(|r| r.manual)
    }

    /// The account to use for `slug`: the user's choice or the one learned, then one that
    /// lists it, then the owner or a member of the organization, else the first that can
    /// see it (tried in order, with the GitHub CLI fallback; the result is remembered).
    pub(super) fn account_for(&mut self, slug: &Slug) -> Option<Account> {
        if self.unusable_choice(slug).is_some() {
            return None;
        }
        match self.known_login_for(slug) {
            Some(login) => self.accounts.get(&login),
            None => self.probe(slug, None),
        }
    }

    /// `account_for` without asking GitHub: the choice, the account learned or listing it,
    /// the owner or a member of the organization.
    fn known_account_for(&mut self, slug: &Slug) -> Option<Account> {
        if self.unusable_choice(slug).is_some() {
            return None;
        }
        let login = self.known_login_for(slug)?;
        self.accounts.get(&login)
    }

    /// Login `choose_account` gives for `slug` (remembered when a list showed it).
    fn known_login_for(&mut self, slug: &Slug) -> Option<String> {
        let key = repo_key(slug);
        let seen = self.seen.get(&key).cloned().unwrap_or_default();
        let login = choose_account(&slug.0, &self.accounts, self.repo_accounts.get(&key), &seen)?;
        if seen.iter().any(|l| l.eq_ignore_ascii_case(&login)) {
            self.learn(slug, &login);
        }
        Some(login)
    }

    /// Try each account (except `exclude`) until one can see `slug`, and remember it.
    /// "No account can see it" is remembered for the session only when every account got
    /// that answer (not after a network error).
    pub(super) fn probe(&mut self, slug: &Slug, exclude: Option<&str>) -> Option<Account> {
        let key = repo_key(slug);
        if exclude.is_none() && self.no_account.contains(&key) {
            return None;
        }
        let mut all_hidden = true;
        for account in self.accounts.list() {
            if exclude.is_some_and(|x| x.eq_ignore_ascii_case(&account.login)) {
                continue;
            }
            let found = self
                .deps
                .tokens
                .with_token(&account.login, &account.token, &slug.0, |t| {
                    logging::add_secret(t);
                    self.deps.client.check_repo(t, &slug.0, &slug.1)
                });
            match found {
                Ok(()) => {
                    self.learn(slug, &account.login);
                    return Some(account);
                }
                Err(e) if github::hidden_by_restriction(&e) => {}
                Err(e) => {
                    log::info!("cannot check {}'s access to {key}: {e}", account.login);
                    all_hidden = false;
                }
            }
        }
        if all_hidden && exclude.is_none() {
            self.no_account.insert(key);
        }
        None
    }

    /// `failed` (chosen automatically) cannot see `slug`: find another account. Returns
    /// whether one was found (and remembered).
    pub(super) fn replace_account(&mut self, slug: &Slug, failed: &str) -> bool {
        !self.is_manual(slug) && self.probe(slug, Some(failed)).is_some()
    }

    /// The token to give `git` for `slug` (the GitHub CLI's for a restricted owner).
    pub(super) fn git_token_for(&mut self, slug: &Slug) -> Option<String> {
        let account = self.account_for(slug)?;
        if self.deps.tokens.is_restricted(&account.login, &slug.0)
            && let Some(gh) = self.deps.tokens.gh_token_for(&account.login)
        {
            logging::add_secret(&gh);
            return Some(gh);
        }
        Some(account.token)
    }

    /// Credentials for git network commands on the open repository.
    pub(super) fn repo_net_auth(&mut self) -> NetAuth {
        let slug = self.open_current(Op::Sync).and_then(|r| r.github_slug());
        NetAuth {
            github_token: slug.and_then(|s| self.git_token_for(&s)),
        }
    }

    /// Tell the UI which account the open repository uses.
    pub(super) fn send_repo_account(&mut self) {
        self.tell_repo_account(true);
    }

    /// `send_repo_account` without probing GitHub (after a rejected token).
    fn send_repo_account_known(&mut self) {
        self.tell_repo_account(false);
    }

    fn tell_repo_account(&mut self, probe: bool) {
        let Some(slug) = self
            .repo
            .clone()
            .and_then(|p| gitcore::Repo::open(&p).ok())
            .and_then(|r| r.github_slug())
        else {
            return;
        };
        let account = if probe {
            self.account_for(&slug)
        } else {
            self.known_account_for(&slug)
        };
        let login = account.map(|a| a.login);
        self.emit(Event::RepoAccount { slug, login });
    }

    /// The user chose the account of `slug` (`None`: automatic again).
    pub(super) fn set_repo_account(&mut self, slug: &Slug, login: Option<String>) {
        let key = repo_key(slug);
        let account = login.map(|login| RepoAccount {
            login,
            manual: true,
        });
        match &account {
            Some(a) => {
                self.repo_accounts.insert(key.clone(), a.clone());
            }
            None => {
                self.repo_accounts.remove(&key);
            }
        }
        self.no_account.remove(&key);
        self.emit(Event::RepoAccountLearned { key, account });
        self.send_repo_account();
    }

    /// Repositories of every account (with the GitHub CLI's token too, which sees the
    /// organizations that restrict RetroGit), merged.
    pub(super) fn list_repos(&mut self) {
        let accounts = self.accounts.list();
        if accounts.is_empty() {
            return self.emit(Event::SignedOut);
        }
        let mut lists = Vec::new();
        let mut sso_hidden = false;
        for a in accounts {
            match self.deps.client.list_repos(&a.token) {
                Ok(listing) => {
                    sso_hidden |= !listing.sso_hidden_orgs.is_empty();
                    lists.push((a.login.clone(), listing.repos));
                }
                Err(GithubError::Unauthorized) => {
                    self.invalidate(&a.login, Op::Repos);
                    continue;
                }
                Err(e) => {
                    self.fail(Op::Repos, AppError::from_github(&e));
                    continue;
                }
            }
            if let Some(gh) = self
                .deps
                .tokens
                .gh_token_for(&a.login)
                .filter(|g| *g != a.token)
            {
                logging::add_secret(&gh);
                if let Ok(listing) = self.deps.client.list_repos(&gh) {
                    // Owners listed only with gh restrict RetroGit's token: clone and fetch
                    // their repositories with gh's token straight away.
                    let own: Vec<String> = lists
                        .last()
                        .map(|(_, repos): &(String, Vec<RepoInfo>)| {
                            repos.iter().map(|r| r.owner.to_lowercase()).collect()
                        })
                        .unwrap_or_default();
                    for r in &listing.repos {
                        if !own.contains(&r.owner.to_lowercase()) {
                            self.deps.tokens.remember(&a.login, &r.owner);
                        }
                    }
                    lists.push((a.login.clone(), listing.repos));
                }
            }
        }
        let merged = merge_repo_lists(lists);
        for r in &merged {
            self.seen
                .insert(r.full_name.to_lowercase(), r.accounts.clone());
        }
        self.emit(Event::ReposLoaded(merged));
        if sso_hidden {
            let mut warning = AppError::new(Severity::Warning, s::ERR_SSO_PARTIAL);
            warning.link = Some(self.sso_settings_link());
            self.fail(Op::Repos, warning);
        }
    }

    /// Clone with `account` (from the repository list), or the account of the URL's
    /// github.com repository, or the user's own git credentials.
    pub(super) fn clone(&mut self, url: String, dest: std::path::PathBuf, account: Option<String>) {
        let slug = gitcore::parse_github_slug(&url);
        let account = match (account, &slug) {
            (Some(login), _) => self.accounts.get(&login),
            (None, Some(slug)) => self.account_for(slug),
            (None, None) => None,
        };
        // A chosen account that must sign in again: no other one was tried.
        let unusable = match (&account, &slug) {
            (None, Some(slug)) => self.unusable_choice(slug),
            _ => None,
        };
        let tried: Vec<String> = self.accounts.list().into_iter().map(|a| a.login).collect();
        let token = match (&account, &slug) {
            (Some(a), Some(slug)) if self.deps.tokens.is_restricted(&a.login, &slug.0) => self
                .deps
                .tokens
                .gh_token_for(&a.login)
                .or(Some(a.token.clone())),
            (Some(a), _) => Some(a.token.clone()),
            _ => None,
        };
        let credentials = token.map(|t| Credentials {
            username: "x-access-token".into(),
            password: t,
        });
        let req = CloneRequest {
            url,
            dest,
            credentials,
        };
        let mut throttle = Throttle::new(Duration::from_millis(50));
        let mut last = None;
        let emit = &self.emit;
        let result = gitcore::clone(
            &req,
            |p| {
                last = Some(p);
                if throttle.ready(Instant::now()) {
                    emit(Event::CloneProgress(p));
                }
            },
            &self.cancel_clone,
        );
        if let Some(p) = last {
            self.emit(Event::CloneProgress(p));
        }
        match result.and_then(|repo| repo.summary()) {
            Ok(summary) => {
                if let (Some(a), Some(slug)) = (&account, &slug) {
                    self.learn(slug, &a.login);
                }
                self.opened(summary, true)
            }
            Err(GitError::Cancelled) => self.emit(Event::CloneCancelled),
            Err(e) => match clone_error(
                slug.is_some() && account.is_none(),
                unusable.as_deref(),
                &tried,
                &e,
            ) {
                Some(error) if unusable.is_some() => self.fail(Op::Clone, error),
                Some(mut error) => {
                    error.link = Some(self.sso_settings_link());
                    self.fail(Op::Clone, error)
                }
                None if matches!(e, GitError::Auth(_)) => {
                    self.clone_auth_failed(&e, account.as_ref())
                }
                None => self.fail(Op::Clone, AppError::from_git(&e)),
            },
        }
    }

    /// Git refused our credentials: a revoked token or a missing SSO authorization.
    fn clone_auth_failed(&mut self, e: &GitError, account: Option<&Account>) {
        if let Some(a) = account
            && self.deps.client.current_user(&a.token) == Err(GithubError::Unauthorized)
        {
            return self.invalidate(&a.login, Op::Clone);
        }
        let mut error = AppError::from_git(e);
        error.link = Some(self.sso_settings_link());
        self.fail(Op::Clone, error);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo(full: &str, updated: &str) -> RepoInfo {
        let (owner, name) = full.split_once('/').unwrap_or((full, full));
        RepoInfo {
            full_name: full.into(),
            name: name.into(),
            owner: owner.into(),
            private: false,
            clone_url: format!("https://github.com/{full}.git"),
            updated_at: updated.into(),
            accounts: vec![],
        }
    }

    #[test]
    fn lists_are_merged_with_the_accounts_that_see_each_repository() {
        let merged = merge_repo_lists(vec![
            (
                "perso".into(),
                vec![repo("me/app", "2026-09-01"), repo("Corp/x", "2026-09-03")],
            ),
            (
                "pro".into(),
                vec![repo("corp/X", "2026-09-03"), repo("corp/y", "2026-09-02")],
            ),
            ("pro".into(), vec![repo("corp/y", "2026-09-02")]),
        ]);
        let rows: Vec<(String, Vec<String>)> = merged
            .into_iter()
            .map(|r| (r.full_name, r.accounts))
            .collect();
        assert_eq!(
            rows,
            [
                (
                    "Corp/x".to_string(),
                    vec!["perso".to_string(), "pro".to_string()]
                ),
                ("corp/y".to_string(), vec!["pro".to_string()]),
                ("me/app".to_string(), vec!["perso".to_string()]),
            ]
        );
    }

    #[test]
    fn repository_keys_ignore_case() {
        assert_eq!(repo_key(&("Corp".into(), "App".into())), "corp/app");
    }
}
