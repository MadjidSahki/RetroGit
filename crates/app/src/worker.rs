//! The single background thread doing all network and Git work.

mod changes;
mod pulls;
mod sync;

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use gitcore::{CloneRequest, CommitBackend, Credentials, GitError, Repo, Side};
use github::{Client, DeviceFlow, GithubError, Step, TokenStore};

use crate::logging;
use crate::protocol::{AppError, Command, Event, Op, Severity};
use crate::strings as s;

pub struct WorkerDeps {
    pub client: Client,
    pub store: Arc<dyn TokenStore>,
    /// Empty = Device Flow unavailable (PAT only).
    pub client_id: String,
    pub commit_backend: CommitBackend,
    /// Chooses the token per organization (`gh` fallback for restricted organizations).
    pub tokens: github::TokenProvider,
}

/// Token and login of the signed-in user, shared with the pull request watcher.
pub type Session = Arc<std::sync::Mutex<Option<(String, String)>>>;

/// UI-side handle. Cancellation flags bypass the command queue so they act immediately.
pub struct WorkerHandle {
    session: Session,
    tx: Sender<Command>,
    pub events: Receiver<Event>,
    cancel_flow: Arc<AtomicBool>,
    cancel_clone: Arc<AtomicBool>,
    busy: Arc<AtomicBool>,
    refresh_pending: Arc<AtomicBool>,
    cancel_net: Arc<AtomicBool>,
}

impl WorkerHandle {
    pub fn send(&self, cmd: Command) {
        match cmd {
            Command::StartDeviceFlow => self.cancel_flow.store(false, Ordering::SeqCst),
            Command::Clone { .. } => self.cancel_clone.store(false, Ordering::SeqCst),
            Command::Fetch { .. } | Command::Pull(_) | Command::Push(_) => {
                self.cancel_net.store(false, Ordering::SeqCst)
            }
            // At most one refresh waiting in the queue.
            Command::RefreshStatus if self.refresh_pending.swap(true, Ordering::SeqCst) => return,
            _ => {}
        }
        if self.tx.send(cmd).is_err() {
            log::error!("worker thread is gone");
        }
    }

    /// Who is signed in, kept up to date by the worker (read by the pull request watcher).
    pub fn session(&self) -> Session {
        self.session.clone()
    }

    /// Thread-safe "please refresh the status" callback (for the file watcher).
    pub fn refresher(&self) -> impl Fn() + Send + 'static {
        let tx = self.tx.clone();
        let pending = self.refresh_pending.clone();
        move || {
            if !pending.swap(true, Ordering::SeqCst) {
                let _ = tx.send(Command::RefreshStatus);
            }
        }
    }

    pub fn cancel_device_flow(&self) {
        self.cancel_flow.store(true, Ordering::SeqCst);
    }

    pub fn cancel_clone(&self) {
        self.cancel_clone.store(true, Ordering::SeqCst);
    }

    /// Stop the running fetch / pull / push (kills the git process).
    pub fn cancel_network(&self) {
        self.cancel_net.store(true, Ordering::SeqCst);
    }

    /// Cancel whatever is running and wait (up to `timeout`) for the worker to be idle,
    /// so a clone interrupted by quitting still removes its partial folder.
    /// Returns `true` if the worker became idle in time.
    pub fn shutdown(&self, timeout: Duration) -> bool {
        self.cancel_device_flow();
        self.cancel_clone();
        self.cancel_network();
        let deadline = Instant::now() + timeout;
        while self.busy.load(Ordering::SeqCst) {
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        true
    }
}

/// Emit at most one progress event per `every` (the last one is always sent separately).
pub struct Throttle {
    every: Duration,
    last: Option<Instant>,
}

impl Throttle {
    pub fn new(every: Duration) -> Throttle {
        Throttle { every, last: None }
    }

    pub fn ready(&mut self, now: Instant) -> bool {
        match self.last {
            Some(t) if now.duration_since(t) < self.every => false,
            _ => {
                self.last = Some(now);
                true
            }
        }
    }
}

/// Start the worker thread. `notify` is called after each event (wakes up egui).
pub fn spawn(deps: WorkerDeps, notify: impl Fn() + Send + 'static) -> WorkerHandle {
    let (tx, rx) = channel::<Command>();
    let (etx, erx) = channel::<Event>();
    let cancel_flow = Arc::new(AtomicBool::new(false));
    let cancel_clone = Arc::new(AtomicBool::new(false));
    let busy = Arc::new(AtomicBool::new(false));
    let worker_busy = busy.clone();
    let refresh_pending = Arc::new(AtomicBool::new(false));
    let cancel_net = Arc::new(AtomicBool::new(false));
    let session: Session = Arc::new(std::sync::Mutex::new(None));
    let mut worker = Worker {
        session: session.clone(),
        cancel_net: cancel_net.clone(),
        deps,
        token: None,
        repo: None,
        shown: None,
        lease: None,
        refresh_pending: refresh_pending.clone(),
        cancel_flow: cancel_flow.clone(),
        cancel_clone: cancel_clone.clone(),
        emit: Box::new(move |ev| {
            let _ = etx.send(ev);
            notify();
        }),
    };
    let spawned = std::thread::Builder::new()
        .name("retrogit-worker".into())
        .spawn(move || {
            for cmd in rx {
                worker_busy.store(true, Ordering::SeqCst);
                let name = format!("{cmd:?}");
                let outcome = catch_unwind(AssertUnwindSafe(|| worker.handle(cmd)));
                worker_busy.store(false, Ordering::SeqCst);
                if outcome.is_err() {
                    log::error!(
                        "worker panicked while handling {}",
                        logging::redact(&name, &[])
                    );
                    (worker.emit)(Event::Error {
                        during: Op::Internal,
                        error: AppError::new(Severity::Error, s::ERR_INTERNAL),
                    });
                }
            }
        });
    if let Err(e) = spawned {
        log::error!("could not start worker thread: {e}");
    }
    WorkerHandle {
        session,
        tx,
        events: erx,
        cancel_flow,
        cancel_clone,
        busy,
        refresh_pending,
        cancel_net,
    }
}

struct Worker {
    session: Session,
    deps: WorkerDeps,
    token: Option<String>,
    /// Repository opened last (target of all sub-project 2 commands).
    repo: Option<std::path::PathBuf>,
    /// File whose diff the UI displays; its diff is re-sent after every change.
    shown: Option<(String, Side)>,
    refresh_pending: Arc<AtomicBool>,
    cancel_net: Arc<AtomicBool>,
    /// `(branch, remote commit)` recorded when a pushed commit of `branch` was amended:
    /// the only case where RetroGit offers a force push, leased on that commit.
    lease: Option<(String, String)>,
    cancel_flow: Arc<AtomicBool>,
    cancel_clone: Arc<AtomicBool>,
    emit: Box<dyn Fn(Event) + Send>,
}

impl Worker {
    /// Signed in (or out): tell the token provider and the watcher.
    fn set_session(&self, session: Option<(&str, &str)>) {
        self.deps.tokens.set_login(session.map(|(_, login)| login));
        if let Ok(mut s) = self.session.lock() {
            *s = session.map(|(t, l)| (t.to_string(), l.to_string()));
        }
    }

    fn emit(&self, ev: Event) {
        (self.emit)(ev);
    }

    fn fail(&self, during: Op, error: AppError) {
        log::warn!("{during:?}: {} {:?}", error.message, error.detail);
        self.emit(Event::Error { during, error });
    }

    fn handle(&mut self, cmd: Command) {
        match cmd {
            Command::ValidateToken => self.validate(),
            Command::StartDeviceFlow => self.device_flow(),
            Command::SavePat(t) => self.sign_in_with(t.trim().to_string(), true),
            Command::SignOut => self.sign_out(),
            Command::ListRepos => self.list_repos(),
            Command::Clone { url, dest } => self.clone(url, dest),
            Command::OpenRepo(path) => match Repo::open(&path).and_then(|r| r.summary()) {
                Ok(summary) => self.opened(summary, false),
                Err(e) => self.fail(Op::Open(path), AppError::from_git(&e)),
            },
            Command::RefreshStatus => {
                self.refresh_pending.store(false, Ordering::SeqCst);
                self.refresh();
            }
            Command::LoadDiff { path, side } => self.load_diff(path, side),
            Command::Stage {
                path,
                selection,
                shown,
            } => self.stage(&path, &selection, shown.as_ref(), true),
            Command::Unstage {
                path,
                selection,
                shown,
            } => self.stage(&path, &selection, shown.as_ref(), false),
            Command::Discard {
                path,
                selection,
                shown,
            } => self.discard(&path, &selection, shown.as_ref()),
            Command::DiscardFiles(paths) => self.discard_files(&paths),
            Command::StageFiles(paths) => self.stage_files(&paths, true),
            Command::UnstageFiles(paths) => self.stage_files(&paths, false),
            Command::Commit { message, amend } => self.commit(&message, amend),
            Command::AddToGitignore(pattern) => self.add_to_gitignore(&pattern),
            Command::LoadAmendInfo => self.amend_info(),
            Command::LoadLog { skip } => self.load_log(skip),
            Command::LoadCommit(id) => self.load_commit(&id),
            Command::LoadCommitFileDiff { id, path } => self.load_commit_file_diff(&id, &path),
            Command::LoadBranches => self.load_branches(),
            Command::CreateBranch { name, switch } => self.create_branch(&name, switch),
            Command::SwitchBranch { name, stash } => self.switch_branch(&name, stash),
            Command::RenameBranch { old, new } => self.rename_branch(&old, &new),
            Command::DeleteBranch { name, force } => self.delete_branch(&name, force),
            Command::Fetch { background } => self.fetch(background),
            Command::Pull(mode) => self.pull(mode),
            Command::Push(mode) => self.push(mode),
            Command::ForcePush => self.force_push(),
            Command::AbortOperation => self.abort_operation(),
            Command::ContinueRebase => self.continue_rebase(),
            pr @ (Command::LoadPulls { .. }
            | Command::LoadPull { .. }
            | Command::LoadRepoMeta(_)
            | Command::CreatePull { .. }
            | Command::SubmitReview { .. }
            | Command::ReplyToThread { .. }
            | Command::AddPullComment { .. }
            | Command::MergePull { .. }
            | Command::SetLabels { .. }
            | Command::CheckoutPull { .. }) => self.handle_pulls(pr),
        }
    }

    fn validate(&mut self) {
        let token = match self.deps.store.load() {
            Ok(Some(t)) => t,
            Ok(None) => return self.emit(Event::SignedOut),
            Err(e) => {
                self.fail(Op::Auth, AppError::from_store(&e));
                return self.emit(Event::SignedOut);
            }
        };
        logging::add_secret(&token);
        match self.deps.client.current_user(&token) {
            Ok(user) => {
                self.set_session(Some((&token, &user.login)));
                self.token = Some(token);
                self.emit(Event::SignedIn(user));
            }
            Err(GithubError::Unauthorized) => {
                let _ = self.deps.store.clear();
                self.emit(Event::SignedOut);
            }
            Err(e) => {
                // Keep the token: the network may come back (VPN, Wi-Fi).
                self.token = Some(token);
                self.fail(Op::Auth, AppError::from_github(&e));
                self.emit(Event::Offline);
            }
        }
    }

    /// Validate `token` with `GET /user`, then store it.
    fn sign_in_with(&mut self, token: String, is_pat: bool) {
        logging::add_secret(&token);
        match self.deps.client.current_user(&token) {
            Ok(user) => {
                if let Err(e) = self.deps.store.save(&token) {
                    // Still signed in for this session; warn that it won't persist.
                    self.fail(Op::Auth, AppError::from_store(&e));
                }
                self.set_session(Some((&token, &user.login)));
                self.token = Some(token);
                self.emit(Event::SignedIn(user));
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

    fn device_flow(&mut self) {
        if self.deps.client_id.is_empty() {
            return self.fail(Op::Auth, AppError::new(Severity::Info, s::ERR_NO_CLIENT_ID));
        }
        let code = match self
            .deps
            .client
            .request_device_code(&self.deps.client_id, github::SCOPES)
        {
            Ok(c) => c,
            Err(e) => return self.fail(Op::Auth, AppError::from_github(&e)),
        };
        let started = Instant::now();
        self.emit(Event::DeviceCode {
            user_code: code.user_code.clone(),
            verification_uri: code.verification_uri.clone(),
        });
        let mut flow = DeviceFlow::new(&code);
        let mut wait = flow.first_wait();
        loop {
            if !sleep_unless_cancelled(wait, &self.cancel_flow) {
                return self.emit(Event::DeviceFlowCancelled);
            }
            let response = match self
                .deps
                .client
                .poll_token(&self.deps.client_id, &code.device_code)
            {
                Ok(r) => r,
                Err(e) => return self.fail(Op::Auth, AppError::from_github(&e)),
            };
            match flow.on_response(response, started.elapsed()) {
                Step::Wait(d) => wait = d,
                Step::Done(token) => return self.sign_in_with(token, false),
                Step::Failed(f) => return self.fail(Op::Auth, AppError::from_device_flow(&f)),
            }
        }
    }

    fn sign_out(&mut self) {
        self.token = None;
        self.set_session(None);
        if let Err(e) = self.deps.store.clear() {
            self.fail(Op::Auth, AppError::from_store(&e));
        }
        self.emit(Event::SignedOut);
    }

    fn list_repos(&mut self) {
        let Some(token) = self.token.clone() else {
            return self.emit(Event::SignedOut);
        };
        match self.deps.client.list_repos(&token) {
            Ok(listing) => {
                self.emit(Event::ReposLoaded(listing.repos));
                if !listing.sso_hidden_orgs.is_empty() {
                    let mut warning = AppError::new(Severity::Warning, s::ERR_SSO_PARTIAL);
                    warning.link = Some(self.sso_settings_link());
                    self.fail(Op::Repos, warning);
                }
            }
            Err(GithubError::Unauthorized) => self.drop_token(Op::Repos),
            Err(e) => self.fail(Op::Repos, AppError::from_github(&e)),
        }
    }

    fn clone(&mut self, url: String, dest: std::path::PathBuf) {
        let credentials = self.token.clone().map(|t| Credentials {
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
            Ok(summary) => self.opened(summary, true),
            Err(GitError::Cancelled) => self.emit(Event::CloneCancelled),
            Err(e @ GitError::Auth(_)) => self.clone_auth_failed(&e),
            Err(e) => self.fail(Op::Clone, AppError::from_git(&e)),
        }
    }
}

impl Worker {
    /// Where the user grants SSO access: the OAuth App's connection page, or token settings.
    fn sso_settings_link(&self) -> String {
        if self.deps.client_id.is_empty() {
            "https://github.com/settings/tokens".to_string()
        } else {
            format!(
                "https://github.com/settings/connections/applications/{}",
                self.deps.client_id
            )
        }
    }

    /// The token was rejected: forget it everywhere and ask to sign in again.
    fn drop_token(&mut self, during: Op) {
        self.token = None;
        self.set_session(None);
        let _ = self.deps.store.clear();
        self.fail(during, AppError::from_github(&GithubError::Unauthorized));
        self.emit(Event::SignedOut);
    }

    /// Git refused our credentials: a revoked token or a missing SSO authorization.
    fn clone_auth_failed(&mut self, e: &GitError) {
        if let Some(token) = self.token.clone()
            && self.deps.client.current_user(&token) == Err(GithubError::Unauthorized)
        {
            return self.drop_token(Op::Clone);
        }
        let mut error = AppError::from_git(e);
        error.link = Some(self.sso_settings_link());
        self.fail(Op::Clone, error);
    }
}

/// Sleep `d` in small steps. Returns `false` if cancelled.
fn sleep_unless_cancelled(d: Duration, cancel: &AtomicBool) -> bool {
    let end = Instant::now() + d;
    while Instant::now() < end {
        if cancel.load(Ordering::SeqCst) {
            return false;
        }
        std::thread::sleep(
            Duration::from_millis(50).min(end.saturating_duration_since(Instant::now())),
        );
    }
    !cancel.load(Ordering::SeqCst)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn throttle_limits_rate() {
        let mut t = Throttle::new(Duration::from_millis(50));
        let t0 = Instant::now();
        assert!(t.ready(t0));
        assert!(!t.ready(t0 + Duration::from_millis(10)));
        assert!(t.ready(t0 + Duration::from_millis(60)));
    }

    #[test]
    fn sleep_stops_when_cancelled() {
        let flag = AtomicBool::new(true);
        let t0 = Instant::now();
        assert!(!sleep_unless_cancelled(Duration::from_secs(5), &flag));
        assert!(t0.elapsed() < Duration::from_secs(1));
        assert!(sleep_unless_cancelled(
            Duration::from_millis(1),
            &AtomicBool::new(false)
        ));
    }
}
