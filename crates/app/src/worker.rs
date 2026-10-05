//! The single background thread doing all network and Git work.

mod accounts;
pub use accounts::{clone_error, clone_refused};
mod changes;
mod conflicts;
mod explore;
mod git_ops;
mod pulls;
mod sync;

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use gitcore::{CommitBackend, Repo, Side};
use github::{Client, DeviceFlow, Step};

use crate::logging;
use crate::protocol::{AppError, Command, Event, Op, Severity};
use crate::strings as s;
use crate::watch::Refresh;

pub struct WorkerDeps {
    pub client: Client,
    /// Account tokens (system credential store).
    pub store: Arc<dyn github::AccountStore>,
    /// Empty = Device Flow unavailable (PAT only).
    pub client_id: String,
    pub commit_backend: CommitBackend,
    /// Chooses the token per organization (`gh` fallback for restricted organizations).
    pub tokens: github::TokenProvider,
    /// Logins remembered in the config, checked at startup.
    pub known_accounts: Vec<String>,
    /// Account of each repository (`owner/repo`, lowercase), as remembered in the config.
    pub repo_accounts: std::collections::BTreeMap<String, github::RepoAccount>,
}

/// UI-side handle. Cancellation flags bypass the command queue so they act immediately.
pub struct WorkerHandle {
    accounts: github::Accounts,
    tx: Sender<Command>,
    pub events: Receiver<Event>,
    cancel_flow: Arc<AtomicBool>,
    cancel_clone: Arc<AtomicBool>,
    busy: Arc<AtomicBool>,
    refresh_pending: Arc<AtomicBool>,
    refs_pending: Arc<AtomicBool>,
    cancel_net: Arc<AtomicBool>,
    explore: explore::ExploreService,
}

impl WorkerHandle {
    /// Ask the explore service (answered on its own threads, as `Event::ExploreLoaded`).
    pub fn explore(&self, repo: &std::path::Path, req: crate::protocol::ExploreRequest) {
        self.explore.request(repo, req);
    }

    pub fn send(&self, cmd: Command) {
        match cmd {
            Command::StartDeviceFlow => self.cancel_flow.store(false, Ordering::SeqCst),
            Command::Clone { .. } => self.cancel_clone.store(false, Ordering::SeqCst),
            Command::Fetch { .. }
            | Command::Pull(_)
            | Command::Push(_)
            | Command::PushTags(_)
            | Command::DeleteTag { .. }
            | Command::CheckoutPull { .. } => self.cancel_net.store(false, Ordering::SeqCst),
            // At most one refresh of each kind waiting in the queue; a refs refresh also
            // refreshes the status.
            Command::RefreshRefs if self.refs_pending.swap(true, Ordering::SeqCst) => return,
            Command::RefreshStatus
                if self.refs_pending.load(Ordering::SeqCst)
                    || self.refresh_pending.swap(true, Ordering::SeqCst) =>
            {
                return;
            }
            _ => {}
        }
        if self.tx.send(cmd).is_err() {
            log::error!("worker thread is gone");
        }
    }

    /// The worker is running a command (an update waits for it).
    pub fn is_busy(&self) -> bool {
        self.busy.load(Ordering::SeqCst)
    }

    /// The signed-in accounts, kept up to date by the worker (read by the watcher).
    pub fn accounts(&self) -> github::Accounts {
        self.accounts.clone()
    }

    /// Thread-safe "please refresh the status / the refs" callback (for the file watcher).
    pub fn refresher(&self) -> impl Fn(Refresh) + Send + 'static {
        let tx = self.tx.clone();
        let pending = self.refresh_pending.clone();
        let refs_pending = self.refs_pending.clone();
        move |kind| {
            let cmd = match kind {
                Refresh::Refs if !refs_pending.swap(true, Ordering::SeqCst) => Command::RefreshRefs,
                Refresh::Status
                    if !refs_pending.load(Ordering::SeqCst)
                        && !pending.swap(true, Ordering::SeqCst) =>
                {
                    Command::RefreshStatus
                }
                _ => return,
            };
            let _ = tx.send(cmd);
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
    let refs_pending = Arc::new(AtomicBool::new(false));
    let cancel_net = Arc::new(AtomicBool::new(false));
    let accounts = github::Accounts::default();
    // Both the worker and the explore service send events and wake the UI up.
    let notify = Arc::new(std::sync::Mutex::new(notify));
    let emit: Arc<dyn Fn(Event) + Send + Sync> = Arc::new(move |ev| {
        let _ = etx.send(ev);
        if let Ok(n) = notify.lock() {
            n();
        }
    });
    let worker_emit = emit.clone();
    let mut worker = Worker {
        accounts: accounts.clone(),
        repo_accounts: deps.repo_accounts.clone(),
        seen: Default::default(),
        no_account: Default::default(),
        unchecked: Default::default(),
        offline: false,
        recheck_throttle: Throttle::new(Duration::from_secs(5 * 60)),
        last_account: None,
        cancel_net: cancel_net.clone(),
        deps,
        repo: None,
        shown: None,
        shown_conflict: None,
        lease: None,
        refresh_pending: refresh_pending.clone(),
        refs_pending: refs_pending.clone(),
        cancel_flow: cancel_flow.clone(),
        cancel_clone: cancel_clone.clone(),
        emit: Box::new(move |ev| worker_emit(ev)),
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
        accounts,
        tx,
        events: erx,
        cancel_flow,
        cancel_clone,
        busy,
        refresh_pending,
        refs_pending,
        cancel_net,
        explore: explore::ExploreService::new(emit),
    }
}

struct Worker {
    /// Signed-in accounts (shared with the watcher).
    accounts: github::Accounts,
    /// Account of each repository (`owner/repo`, lowercase): chosen or learned.
    repo_accounts: std::collections::BTreeMap<String, github::RepoAccount>,
    /// Accounts each repository was listed for (from the repository lists).
    seen: std::collections::HashMap<String, Vec<String>>,
    /// Repositories no account could see this session (not tried again).
    no_account: std::collections::HashSet<String>,
    /// Accounts kept without being checked (offline at startup): checked again on the
    /// next `ValidateToken` and after the next GitHub call that works.
    unchecked: std::collections::HashSet<String>,
    /// The app was told it is offline (no account checked at startup).
    offline: bool,
    /// Checks of `unchecked` after a GitHub call that works (validate checks every time).
    recheck_throttle: Throttle,
    /// Account used by the last GitHub call (to sign out the right one on a 401).
    last_account: Option<String>,
    deps: WorkerDeps,
    /// Repository opened last (target of all sub-project 2 commands).
    repo: Option<std::path::PathBuf>,
    /// File whose diff the UI displays; its diff is re-sent after every change.
    shown: Option<(String, Side)>,
    /// File open in the conflict editor; re-sent after every refresh (changes on disk).
    shown_conflict: Option<String>,
    refresh_pending: Arc<AtomicBool>,
    refs_pending: Arc<AtomicBool>,
    cancel_net: Arc<AtomicBool>,
    /// `(branch, remote commit)` recorded when a pushed commit of `branch` was amended:
    /// the only case where RetroGit offers a force push, leased on that commit.
    lease: Option<(String, String)>,
    cancel_flow: Arc<AtomicBool>,
    cancel_clone: Arc<AtomicBool>,
    emit: Box<dyn Fn(Event) + Send>,
}

impl Worker {
    fn emit(&self, ev: Event) {
        (self.emit)(ev);
    }

    fn fail(&self, during: Op, mut error: AppError) {
        if self.deps.tokens.gh_too_old() {
            error = error.for_old_gh();
        }
        log::warn!("{during:?}: {} {:?}", error.message, error.detail);
        self.emit(Event::Error { during, error });
    }

    fn handle(&mut self, cmd: Command) {
        match cmd {
            Command::ValidateToken => self.validate(),
            Command::StartDeviceFlow => self.device_flow(),
            Command::SavePat(t) => self.sign_in_with(t.trim().to_string(), true),
            Command::RemoveAccount(login) => self.remove_account(&login),
            Command::SetRepoAccount { slug, login } => self.set_repo_account(&slug, login),
            Command::ListRepos => self.list_repos(),
            Command::Clone { url, dest, account } => self.clone(url, dest, account),
            Command::OpenRepo(path) => match Repo::open(&path).and_then(|r| r.summary()) {
                Ok(summary) => self.opened(summary, false),
                Err(e) => self.fail(Op::Open(path), AppError::from_git(&e)),
            },
            Command::RefreshStatus => {
                self.refresh_pending.store(false, Ordering::SeqCst);
                self.refresh();
            }
            Command::RefreshRefs => {
                self.refs_pending.store(false, Ordering::SeqCst);
                if let Some(repo) = self.open_current(Op::Changes) {
                    self.after_ref_change(&repo);
                }
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
            Command::LoadConflict(path) => self.load_conflict(&path),
            Command::ResolveConflict { path, content } => {
                self.resolve(&path, |r| r.resolve_with_content(&path, &content))
            }
            Command::ResolveConflictWith { path, pick } => {
                self.resolve(&path, |r| r.resolve_with(&path, pick))
            }
            Command::ResolveDelete(path) => self.resolve(&path, |r| r.resolve_delete(&path)),
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
            op @ (Command::CherryPick { .. }
            | Command::Revert { .. }
            | Command::Reset { .. }
            | Command::LoadResetInfo(_)
            | Command::LoadRebaseList(_)
            | Command::InteractiveRebase { .. }
            | Command::ContinueOperation
            | Command::SkipOperation
            | Command::LoadStashes
            | Command::StashSave { .. }
            | Command::StashApply { .. }
            | Command::StashPop { .. }
            | Command::StashDrop { .. }
            | Command::LoadStashFiles(_)
            | Command::LoadStashFileDiff { .. }
            | Command::LoadTags
            | Command::CreateTag { .. }
            | Command::DeleteTag { .. }
            | Command::PushTags(_)
            | Command::StashAndRetry { .. }) => self.handle_git_ops(op),
            pr @ (Command::LoadPulls { .. }
            | Command::LoadPull { .. }
            | Command::RefreshPull { .. }
            | Command::LoadRepoMeta(_)
            | Command::CreatePull { .. }
            | Command::SubmitReview { .. }
            | Command::ReplyToThread { .. }
            | Command::AddPullComment { .. }
            | Command::AddLineComment { .. }
            | Command::ResolveThread { .. }
            | Command::UpdatePull { .. }
            | Command::SetPeople { .. }
            | Command::SetDraft { .. }
            | Command::LoadAssignable { .. }
            | Command::ApplySuggestion { .. }
            | Command::MergePull { .. }
            | Command::SetLabels { .. }
            | Command::CheckoutPull { .. }) => self.handle_pulls(pr),
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
