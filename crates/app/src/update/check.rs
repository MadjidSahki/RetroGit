//! Asking GitHub for the latest release: now and then, or when the user asks.

use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::time::Duration;

use ureq::Agent;
use ureq::tls::{RootCerts, TlsConfig};

use super::{REPO, Release, parse_release};

/// Shared by the check and the download: the OS trust store (corporate proxies), no
/// automatic redirects (each one is checked), RetroGit's user agent.
pub(crate) fn agent(timeout: Duration) -> Agent {
    Agent::config_builder()
        .http_status_as_error(false)
        .max_redirects(0)
        .user_agent(concat!("RetroGit/", env!("CARGO_PKG_VERSION")))
        .timeout_connect(Some(Duration::from_secs(10)))
        .timeout_global(Some(timeout))
        .tls_config(
            TlsConfig::builder()
                .root_certs(RootCerts::PlatformVerifier)
                .build(),
        )
        .build()
        .into()
}

/// `GET {api}/repos/MadjidSahki/RetroGit/releases/latest` (no token: public repository).
pub fn fetch_latest(api: &str) -> Result<Release, String> {
    let url = format!("{}/repos/{REPO}/releases/latest", api.trim_end_matches('/'));
    let mut resp = agent(Duration::from_secs(15))
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| e.to_string())?;
    let status = resp.status().as_u16();
    let body = resp
        .body_mut()
        .read_to_string()
        .map_err(|e| e.to_string())?;
    if status != 200 {
        let message = serde_json::from_str::<serde_json::Value>(&body)
            .ok()
            .and_then(|v| v["message"].as_str().map(String::from))
            .unwrap_or_default();
        return Err(format!("GitHub answered {status}: {message}"));
    }
    parse_release(&body)
}

/// Background checks: `first` after start, then every `every`; `check_now` asks at once.
/// `deliver(result, manual)` runs on the checker thread.
pub struct Checker {
    tx: Sender<()>,
}

impl Checker {
    pub fn start(
        api: String,
        first: Duration,
        every: Duration,
        automatic: impl Fn() -> bool + Send + 'static,
        deliver: impl Fn(Result<Release, String>, bool) + Send + 'static,
    ) -> Checker {
        let (tx, rx): (Sender<()>, Receiver<()>) = channel();
        let spawned = std::thread::Builder::new()
            .name("retrogit-update-check".into())
            .spawn(move || {
                let mut wait = first;
                loop {
                    let manual = match rx.recv_timeout(wait) {
                        Ok(()) => true,
                        Err(RecvTimeoutError::Timeout) => false,
                        Err(RecvTimeoutError::Disconnected) => return,
                    };
                    if manual || automatic() {
                        deliver(fetch_latest(&api), manual);
                    }
                    if !manual {
                        wait = every;
                    }
                }
            });
        if let Err(e) = spawned {
            log::warn!("update checks not started: {e}");
        }
        Checker { tx }
    }

    pub fn check_now(&self) {
        let _ = self.tx.send(());
    }
}
