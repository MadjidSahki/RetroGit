#![allow(clippy::unwrap_used)]
//! Token choice over time: restricted owners tried again, `gh` tokens cached, old `gh`.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use github::{GH_CACHE, GhToken, GithubError, RETRY_RESTRICTED, TokenProvider};

/// A clock the test moves forward.
struct Clock(Arc<Mutex<Instant>>);

impl Clock {
    fn new() -> Clock {
        Clock(Arc::new(Mutex::new(Instant::now())))
    }
    fn advance(&self, by: Duration) {
        *self.0.lock().unwrap() += by;
    }
    fn source(&self) -> Arc<dyn Fn() -> Instant + Send + Sync> {
        let now = self.0.clone();
        Arc::new(move || *now.lock().unwrap())
    }
}

/// Provider whose `gh` gives `gho_cli` and counts its calls.
fn provider(clock: &Clock, calls: Arc<AtomicUsize>) -> TokenProvider {
    TokenProvider::new(Arc::new(move |_: &str| {
        calls.fetch_add(1, Ordering::SeqCst);
        Some("gho_cli".to_string())
    }))
    .with_clock(clock.source())
}

fn restricted() -> GithubError {
    GithubError::OAuthRestricted {
        org: Some("ExampleOrg".into()),
    }
}

#[test]
fn a_restricted_owner_is_tried_again_with_retrogits_token_after_a_while() {
    let clock = Clock::new();
    let p = provider(&clock, Arc::new(AtomicUsize::new(0)));
    let used = Mutex::new(Vec::new());
    let approved = Mutex::new(false);
    let call = |t: &str| {
        used.lock().unwrap().push(t.to_string());
        if t == "gho_app" && !*approved.lock().unwrap() {
            Err(restricted())
        } else {
            Ok(())
        }
    };
    p.with_token("ada", "gho_app", "ExampleOrg", call).unwrap();
    clock.advance(RETRY_RESTRICTED - Duration::from_secs(1));
    p.with_token("ada", "gho_app", "ExampleOrg", call).unwrap();
    assert_eq!(*used.lock().unwrap(), ["gho_app", "gho_cli", "gho_cli"]);
    // Still restricted after the delay: one more try, then gh, remembered again.
    used.lock().unwrap().clear();
    clock.advance(Duration::from_secs(2));
    p.with_token("ada", "gho_app", "ExampleOrg", call).unwrap();
    p.with_token("ada", "gho_app", "ExampleOrg", call).unwrap();
    assert_eq!(*used.lock().unwrap(), ["gho_app", "gho_cli", "gho_cli"]);
    // Approved meanwhile: RetroGit's token works and the restriction is forgotten.
    used.lock().unwrap().clear();
    *approved.lock().unwrap() = true;
    clock.advance(RETRY_RESTRICTED);
    p.with_token("ada", "gho_app", "ExampleOrg", call).unwrap();
    p.with_token("ada", "gho_app", "ExampleOrg", call).unwrap();
    assert_eq!(*used.lock().unwrap(), ["gho_app", "gho_app"]);
    assert!(!p.is_restricted("ada", "ExampleOrg"));
}

#[test]
fn forget_restricted_clears_every_owner() {
    let clock = Clock::new();
    let p = provider(&clock, Arc::new(AtomicUsize::new(0)));
    p.remember("ada", "ExampleOrg");
    p.remember("bob", "Other");
    p.forget_restricted();
    assert!(!p.is_restricted("ada", "ExampleOrg"));
    assert!(!p.is_restricted("bob", "Other"));
}

#[test]
fn gh_tokens_are_cached_for_a_few_minutes_per_account() {
    let clock = Clock::new();
    let calls = Arc::new(AtomicUsize::new(0));
    let p = provider(&clock, calls.clone());
    assert_eq!(p.gh_token_for("ada").as_deref(), Some("gho_cli"));
    clock.advance(GH_CACHE - Duration::from_secs(1));
    assert_eq!(p.gh_token_for("Ada").as_deref(), Some("gho_cli"));
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "one gh call within the delay"
    );
    p.gh_token_for("bob");
    assert_eq!(calls.load(Ordering::SeqCst), 2, "per account");
    clock.advance(Duration::from_secs(2));
    p.gh_token_for("ada");
    assert_eq!(
        calls.load(Ordering::SeqCst),
        3,
        "asked again after the delay"
    );
}

#[test]
fn the_gh_cache_is_cleared_by_forgetting_the_account_or_a_refused_gh_token() {
    let clock = Clock::new();
    let calls = Arc::new(AtomicUsize::new(0));
    let p = provider(&clock, calls.clone());
    p.gh_token_for("ada");
    p.forget_account("ada");
    p.gh_token_for("ada");
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    p.gh_rejected("ada");
    p.gh_token_for("ada");
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    // A 401 on gh's token through with_token clears it too.
    p.remember("ada", "ExampleOrg");
    let r: Result<(), _> = p.with_token("ada", "gho_app", "ExampleOrg", |_| {
        Err(GithubError::Unauthorized)
    });
    assert!(r.is_err());
    p.gh_token_for("ada");
    assert_eq!(calls.load(Ordering::SeqCst), 4);
}

#[test]
fn an_old_gh_is_told_apart_from_a_missing_one() {
    for (stderr, old) in [
        (
            "unknown flag: --user\n\nUsage:  gh auth token [flags]",
            true,
        ),
        ("Unknown flag: --user", true),
        ("no oauth token found for github.com", false),
        ("", false),
        ("unknown flag: --hostname", false),
    ] {
        assert_eq!(github::gh_stderr_too_old(stderr), old, "{stderr:?}");
    }
    let p = TokenProvider::from_gh(Arc::new(|_: &str| GhToken::TooOld));
    assert!(!p.gh_too_old());
    assert_eq!(p.gh_token_for("ada"), None);
    assert!(p.gh_too_old());
    let p = TokenProvider::from_gh(Arc::new(|_: &str| GhToken::Missing));
    assert_eq!(p.gh_token_for("ada"), None);
    assert!(!p.gh_too_old());
}

#[cfg(unix)]
#[test]
fn gh_auth_token_reads_the_cli_errors() {
    use std::os::unix::fs::PermissionsExt;
    let d = tempfile::tempdir().unwrap();
    let gh = d.path().join("gh");
    std::fs::write(
        &gh,
        "#!/bin/sh\necho 'unknown flag: --user' >&2\necho 'Usage:  gh auth token [flags]' >&2\nexit 1\n",
    )
    .unwrap();
    std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!("{}:/usr/bin:/bin", d.path().display());
    assert_eq!(
        github::gh_auth_token_checked(Some(&path), Some("ada")),
        GhToken::TooOld
    );
    let empty = tempfile::tempdir().unwrap();
    assert_eq!(
        github::gh_auth_token_checked(Some(&empty.path().display().to_string()), Some("ada")),
        GhToken::Missing
    );
}
