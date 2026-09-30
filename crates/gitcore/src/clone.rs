use std::cell::{Cell, RefCell};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::{GitError, Repo};

/// HTTPS credentials. `Debug` never prints the password.
#[derive(Clone)]
pub struct Credentials {
    pub username: String,
    pub password: String,
}

impl fmt::Debug for Credentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Credentials")
            .field("username", &self.username)
            .field("password", &"***")
            .finish()
    }
}

#[derive(Debug, Clone)]
pub struct CloneRequest {
    /// Clone URL without any embedded credentials.
    pub url: String,
    pub dest: PathBuf,
    pub credentials: Option<Credentials>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CloneProgress {
    pub received_objects: usize,
    pub total_objects: usize,
    pub received_bytes: usize,
    pub indexed_deltas: usize,
    pub total_deltas: usize,
    /// Files written to the working tree (after the download).
    pub checkout_done: usize,
    pub checkout_total: usize,
}

impl CloneProgress {
    /// Overall completion between 0.0 and 1.0 (objects 70 %, deltas 20 %, checkout 10 %).
    pub fn fraction(&self) -> f32 {
        let objects = ratio(self.received_objects, self.total_objects);
        let deltas = if self.total_deltas == 0 {
            if self.total_objects > 0 && self.received_objects == self.total_objects {
                1.0
            } else {
                0.0
            }
        } else {
            ratio(self.indexed_deltas, self.total_deltas)
        };
        let checkout = if self.checkout_total == 0 {
            if deltas >= 1.0 { 1.0 } else { 0.0 }
        } else {
            ratio(self.checkout_done, self.checkout_total)
        };
        0.7 * objects + 0.2 * deltas + 0.1 * checkout
    }
}

fn ratio(a: usize, b: usize) -> f32 {
    if b == 0 {
        0.0
    } else {
        (a as f32 / b as f32).clamp(0.0, 1.0)
    }
}

/// Clone `req.url` into `req.dest`.
///
/// `dest` must not exist or be an empty directory. On failure or cancellation, everything
/// this function created is removed again.
pub fn clone(
    req: &CloneRequest,
    progress: impl FnMut(CloneProgress),
    cancel: &AtomicBool,
) -> Result<Repo, GitError> {
    let created_dest = prepare_destination(&req.dest)?;

    // Download and checkout both report through the same callback.
    let progress = RefCell::new(progress);
    let current = Cell::new(CloneProgress::default());
    let report = |update: &dyn Fn(&mut CloneProgress)| {
        let mut p = current.get();
        update(&mut p);
        current.set(p);
        if let Ok(mut f) = progress.try_borrow_mut() {
            f(p);
        }
    };

    let mut callbacks = git2::RemoteCallbacks::new();
    if let Some(creds) = req.credentials.clone() {
        let mut attempts = 0u8;
        callbacks.credentials(move |_url, _user, _allowed| {
            attempts += 1;
            if attempts > 1 {
                // libgit2 retries forever with the same (rejected) credentials otherwise.
                return Err(git2::Error::new(
                    git2::ErrorCode::Auth,
                    git2::ErrorClass::Http,
                    "credentials rejected",
                ));
            }
            git2::Cred::userpass_plaintext(&creds.username, &creds.password)
        });
    }
    callbacks.transfer_progress(|p| {
        report(&|c| {
            c.received_objects = p.received_objects();
            c.total_objects = p.total_objects();
            c.received_bytes = p.received_bytes();
            c.indexed_deltas = p.indexed_deltas();
            c.total_deltas = p.total_deltas();
        });
        !cancel.load(Ordering::Relaxed)
    });
    // Called during the server-side "counting/compressing" phase, when no objects arrive yet.
    callbacks.sideband_progress(|_| !cancel.load(Ordering::Relaxed));
    let mut fetch = git2::FetchOptions::new();
    fetch.remote_callbacks(callbacks);
    // Honour http.proxy / HTTPS_PROXY like the REST client does (libgit2 default: no proxy).
    let mut proxy = git2::ProxyOptions::new();
    proxy.auto();
    fetch.proxy_options(proxy);

    let mut checkout = git2::build::CheckoutBuilder::new();
    // libgit2 consults notify while planning the checkout (before writing files): a cancel
    // pressed right after the download skips the checkout entirely.
    checkout.notify_on(git2::CheckoutNotificationType::UPDATED);
    checkout.notify(|_, _, _, _, _| !cancel.load(Ordering::Relaxed));
    checkout.progress(|_path, done, total| {
        report(&|c| {
            c.checkout_done = done;
            c.checkout_total = total;
        });
    });

    let result = git2::build::RepoBuilder::new()
        .fetch_options(fetch)
        .with_checkout(checkout)
        .clone(&req.url, &req.dest);
    match result {
        Ok(inner) if !cancel.load(Ordering::Relaxed) => {
            Ok(Repo::from_git2(inner, req.dest.clone()))
        }
        Ok(inner) => {
            // Close libgit2's file handles first: Windows cannot delete open files.
            drop(inner);
            cleanup(&req.dest, created_dest);
            Err(GitError::Cancelled)
        }
        Err(e) => {
            cleanup(&req.dest, created_dest);
            if cancel.load(Ordering::Relaxed) {
                Err(GitError::Cancelled)
            } else {
                Err(GitError::from_git2(&e))
            }
        }
    }
}

/// Returns `true` if the directory did not exist (so we are the ones creating it).
fn prepare_destination(dest: &Path) -> Result<bool, GitError> {
    match std::fs::read_dir(dest) {
        Ok(mut entries) => {
            if entries.next().is_some() {
                Err(GitError::DestinationNotEmpty(dest.to_path_buf()))
            } else {
                Ok(false)
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(true),
        Err(e) => Err(GitError::Other(format!(
            "cannot use '{}': {e}",
            dest.display()
        ))),
    }
}

fn cleanup(dest: &Path, created_dest: bool) {
    if created_dest {
        let _ = std::fs::remove_dir_all(dest);
        return;
    }
    // The directory existed and was empty: remove only what the clone wrote into it.
    if let Ok(entries) = std::fs::read_dir(dest) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                let _ = std::fs::remove_dir_all(&p);
            } else {
                let _ = std::fs::remove_file(&p);
            }
        }
    }
}
