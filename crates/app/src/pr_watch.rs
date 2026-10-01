//! Background watcher of the user's pull requests (all repositories): every 2 minutes it
//! takes a snapshot and turns changes into notifications. The first snapshot after
//! starting (or signing in as someone else) is only a reference: nothing is announced.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use github::{Client, GithubError, PrEvent, PrSnapshot, TokenProvider, diff_snapshots};

use crate::worker::Session;

/// Normal polling interval.
pub const INTERVAL: Duration = Duration::from_secs(120);

/// Wait before the next poll after `failures` failed polls in a row (2, 5, 10, 15 min).
pub fn next_delay(failures: u32) -> Duration {
    let minutes = match failures {
        0 => return INTERVAL,
        1 => 5,
        2 => 10,
        _ => 15,
    };
    Duration::from_secs(minutes * 60)
}

/// Snapshots of every token that sees the user's pull requests, merged by key (RetroGit's
/// token, plus `gh`'s when it is the same account: it sees restricted organizations).
pub struct Poller {
    client: Client,
    tokens: TokenProvider,
    /// `gh` token already checked, and the account it belongs to.
    gh_checked: Option<(String, Option<String>)>,
}

impl Poller {
    pub fn new(client: Client, tokens: TokenProvider) -> Poller {
        Poller {
            client,
            tokens,
            gh_checked: None,
        }
    }

    /// The `gh` token if it belongs to `login` (its account is looked up once per token).
    fn gh_token_for(&mut self, login: &str) -> Option<String> {
        let gh = self.tokens.gh_token()?;
        if self.gh_checked.as_ref().map(|(t, _)| t) != Some(&gh) {
            let account = self.client.current_user(&gh).ok().map(|u| u.login);
            self.gh_checked = Some((gh.clone(), account));
        }
        let account = self.gh_checked.as_ref().and_then(|(_, a)| a.as_deref());
        account
            .is_some_and(|a| a.eq_ignore_ascii_case(login))
            .then_some(gh)
    }

    /// One snapshot of the pull requests involving `login`, updated in the last 7 days.
    pub fn poll(
        &mut self,
        token: &str,
        login: &str,
        now_epoch: i64,
    ) -> Result<Vec<PrSnapshot>, GithubError> {
        let since = github::date_days_before(now_epoch, 7);
        let mut all = self.client.watch_snapshot(token, &since)?;
        if let Some(gh) = self.gh_token_for(login).filter(|g| g != token)
            && let Ok(more) = self.client.watch_snapshot(&gh, &since)
        {
            let known: HashMap<String, usize> = all
                .iter()
                .enumerate()
                .map(|(i, s)| (s.key.clone(), i))
                .collect();
            for s in more {
                if !known.contains_key(&s.key) {
                    all.push(s);
                }
            }
        }
        Ok(all)
    }
}

/// What the watcher remembers between two polls.
#[derive(Default)]
pub struct WatchState {
    login: Option<String>,
    last: Option<Vec<PrSnapshot>>,
}

impl WatchState {
    /// Events of a new snapshot for `login` (none for the first one, or a new login).
    pub fn advance(&mut self, login: &str, snaps: Vec<PrSnapshot>) -> Vec<PrEvent> {
        if self.login.as_deref() != Some(login) {
            self.login = Some(login.to_string());
            self.last = None;
        }
        let events = match &self.last {
            Some(prev) => diff_snapshots(prev, &snaps, login),
            None => Vec::new(),
        };
        self.last = Some(snaps);
        events
    }

    pub fn reset(&mut self) {
        self.login = None;
        self.last = None;
    }
}

/// Handle of the watcher thread; dropping it stops the thread at its next wake-up.
pub struct PrWatcher {
    stop: Arc<AtomicBool>,
}

impl Drop for PrWatcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

impl PrWatcher {
    /// Poll now, then every `interval` (longer after failures). `deliver` gets each
    /// non-empty batch of events, on the watcher thread.
    pub fn start(
        client: Client,
        tokens: TokenProvider,
        session: Session,
        interval: Duration,
        deliver: impl Fn(Vec<PrEvent>) + Send + 'static,
    ) -> PrWatcher {
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let spawned = std::thread::Builder::new()
            .name("retrogit-pr-watch".into())
            .spawn(move || {
                let mut poller = Poller::new(client, tokens);
                let mut state = WatchState::default();
                let mut failures = 0;
                while !stopped.load(Ordering::SeqCst) {
                    let current = session.lock().ok().and_then(|s| s.clone());
                    match current {
                        None => state.reset(),
                        Some((token, login)) => {
                            let now = std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .map(|d| d.as_secs() as i64)
                                .unwrap_or(0);
                            match poller.poll(&token, &login, now) {
                                Ok(snaps) => {
                                    failures = 0;
                                    let events = state.advance(&login, snaps);
                                    if !events.is_empty() {
                                        deliver(events);
                                    }
                                }
                                Err(e) => {
                                    failures += 1;
                                    log::info!("pull request watch failed: {e}");
                                }
                            }
                        }
                    }
                    let wait = if failures == 0 {
                        interval
                    } else {
                        next_delay(failures).max(interval)
                    };
                    let end = std::time::Instant::now() + wait;
                    while std::time::Instant::now() < end && !stopped.load(Ordering::SeqCst) {
                        std::thread::sleep(Duration::from_millis(200).min(wait));
                    }
                }
            });
        if let Err(e) = spawned {
            log::warn!("pull request notifications disabled: {e}");
        }
        PrWatcher { stop }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_schedule() {
        assert_eq!(next_delay(0), Duration::from_secs(120));
        assert_eq!(next_delay(1), Duration::from_secs(300));
        assert_eq!(next_delay(2), Duration::from_secs(600));
        assert_eq!(next_delay(3), Duration::from_secs(900));
        assert_eq!(next_delay(9), Duration::from_secs(900));
    }
}
