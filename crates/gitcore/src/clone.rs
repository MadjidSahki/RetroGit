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
}

impl CloneProgress {
    /// Overall completion between 0.0 and 1.0 (objects count for 80 %, deltas for 20 %).
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
        0.8 * objects + 0.2 * deltas
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
    mut progress: impl FnMut(CloneProgress),
    cancel: &AtomicBool,
) -> Result<Repo, GitError> {
    let created_dest = prepare_destination(&req.dest)?;

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
        progress(CloneProgress {
            received_objects: p.received_objects(),
            total_objects: p.total_objects(),
            received_bytes: p.received_bytes(),
            indexed_deltas: p.indexed_deltas(),
            total_deltas: p.total_deltas(),
        });
        !cancel.load(Ordering::Relaxed)
    });
    let mut fetch = git2::FetchOptions::new();
    fetch.remote_callbacks(callbacks);

    let result = git2::build::RepoBuilder::new()
        .fetch_options(fetch)
        .clone(&req.url, &req.dest);
    match result {
        Ok(inner) if !cancel.load(Ordering::Relaxed) => {
            Ok(Repo::from_git2(inner, req.dest.clone()))
        }
        Ok(_) => {
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
