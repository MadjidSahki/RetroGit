//! Working-tree watcher: tells the UI when the status may have changed.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{RecvTimeoutError, channel};
use std::time::{Duration, Instant};

use notify::{RecursiveMode, Watcher as _};

/// Changes arriving within this window are reported once.
pub const DEBOUNCE: Duration = Duration::from_millis(300);

/// Stops watching when dropped.
pub struct Watcher {
    _inner: notify::RecommendedWatcher,
    root: PathBuf,
}

/// Whether a change at `path` can affect `git status` for the repo at `root`.
/// Inside `.git/`, only the index and HEAD matter (objects, logs, locks are noise).
pub fn is_relevant(root: &Path, path: &Path) -> bool {
    let Ok(rel) = path.strip_prefix(root) else {
        return false;
    };
    let mut parts = rel.components();
    match parts.next() {
        Some(first) if first.as_os_str() == ".git" => {
            let rest: PathBuf = parts.collect();
            rest == Path::new("index") || rest == Path::new("HEAD")
        }
        Some(_) => true,
        None => false,
    }
}

impl Watcher {
    /// Watch `root` recursively; `on_change` runs on a background thread, at most once per
    /// `DEBOUNCE` burst.
    pub fn start(root: &Path, on_change: impl Fn() + Send + 'static) -> notify::Result<Watcher> {
        // FSEvents reports canonical paths (e.g. /private/var/... for /var/...).
        let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        let (tx, rx) = channel::<()>();
        let filter_root = root.clone();
        let mut inner = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            let Ok(event) = res else { return };
            if matches!(event.kind, notify::EventKind::Access(_)) {
                return;
            }
            if event.paths.iter().any(|p| is_relevant(&filter_root, p)) {
                let _ = tx.send(());
            }
        })?;
        inner.watch(&root, RecursiveMode::Recursive)?;
        std::thread::Builder::new()
            .name("retrogit-watch".into())
            .spawn(move || {
                // Ends when the watcher (and so `tx`) is dropped.
                while rx.recv().is_ok() {
                    let deadline = Instant::now() + DEBOUNCE;
                    loop {
                        match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                            Ok(()) => continue,
                            Err(RecvTimeoutError::Timeout) => break,
                            Err(RecvTimeoutError::Disconnected) => return,
                        }
                    }
                    on_change();
                }
            })
            .map_err(|e| notify::Error::generic(&e.to_string()))?;
        Ok(Watcher {
            _inner: inner,
            root,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}

#[cfg(test)]
mod tests {
    use super::is_relevant;
    use std::path::Path;

    #[test]
    fn only_index_and_head_matter_inside_dot_git() {
        let root = Path::new("/r");
        assert!(is_relevant(root, Path::new("/r/src/main.rs")));
        assert!(is_relevant(root, Path::new("/r/.gitignore")));
        assert!(is_relevant(root, Path::new("/r/.git/index")));
        assert!(is_relevant(root, Path::new("/r/.git/HEAD")));
        assert!(!is_relevant(root, Path::new("/r/.git/objects/ab/cdef")));
        assert!(!is_relevant(root, Path::new("/r/.git/index.lock")));
        assert!(!is_relevant(root, Path::new("/elsewhere/file")));
        assert!(!is_relevant(root, Path::new("/r")));
    }
}
