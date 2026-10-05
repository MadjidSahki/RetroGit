//! Working-tree watcher: tells the UI when the status or the refs may have changed.

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

/// Remove Windows' verbatim prefix (`\\?\C:\...`, `\\?\UNC\server\...`).
pub fn strip_verbatim(path: &str) -> String {
    if let Some(rest) = path.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = path.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        path.to_string()
    }
}

/// Canonical path without the Windows verbatim prefix (falls back to `path` itself).
pub fn canonical(path: &Path) -> PathBuf {
    match path.canonicalize() {
        Ok(p) => PathBuf::from(strip_verbatim(&p.to_string_lossy())),
        Err(_) => path.to_path_buf(),
    }
}

/// What a burst of changes may have changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refresh {
    /// The working tree, the index or an operation in progress: reload the status.
    Status,
    /// HEAD or a ref (commit, checkout, fetch...): reload branches, history and status.
    Refs,
}

/// `path` relative to `root/.git`, or `None` when it is not inside it.
fn in_dot_git(root: &Path, path: &Path) -> Option<PathBuf> {
    let mut parts = path.strip_prefix(root).ok()?.components();
    match parts.next() {
        Some(first) if first.as_os_str() == ".git" => Some(parts.collect()),
        _ => None,
    }
}

/// Something strictly inside the `dir` folder of `.git/`.
fn under(rest: &Path, dir: &str) -> bool {
    rest.starts_with(dir) && rest != Path::new(dir)
}

/// Whether a change at `path` can affect what RetroGit shows for the repo at `root`.
/// Inside `.git/`, only the index, HEAD, refs and operation state matter (objects, logs,
/// locks are noise).
pub fn is_relevant(root: &Path, path: &Path) -> bool {
    let Ok(rel) = path.strip_prefix(root) else {
        return false;
    };
    if rel.as_os_str().is_empty() {
        return false;
    }
    let Some(rest) = in_dot_git(root, path) else {
        return true;
    };
    if rest.extension().is_some_and(|e| e == "lock") {
        return false;
    }
    [
        "index",
        "HEAD",
        "packed-refs",
        "MERGE_HEAD",
        "CHERRY_PICK_HEAD",
        "REVERT_HEAD",
    ]
    .iter()
    .any(|f| rest == Path::new(f))
        || ["refs", "rebase-merge", "rebase-apply"]
            .iter()
            .any(|d| under(&rest, d))
}

/// Whether a change at `path` moves HEAD or a ref (branches and history must be reloaded).
pub fn touches_refs(root: &Path, path: &Path) -> bool {
    in_dot_git(root, path).is_some_and(|rest| {
        // `*.lock` files are git's writes in progress: the ref itself changes after.
        rest.extension().is_none_or(|e| e != "lock")
            && (rest == Path::new("HEAD")
                || rest == Path::new("packed-refs")
                || under(&rest, "refs"))
    })
}

impl Watcher {
    /// Watch `root` recursively; `on_change` runs on a background thread, at most once per
    /// `DEBOUNCE` burst, with `Refresh::Refs` if any change of the burst touched a ref.
    pub fn start(
        root: &Path,
        on_change: impl Fn(Refresh) + Send + 'static,
    ) -> notify::Result<Watcher> {
        // FSEvents reports canonical paths (e.g. /private/var/... for /var/...); on Windows
        // canonicalize adds a \\?\ prefix that event paths do not have.
        let root = canonical(root);
        // Each relevant change, `true` when it touches a ref.
        let (tx, rx) = channel::<bool>();
        let filter_root = root.clone();
        let mut inner = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            let Ok(event) = res else { return };
            if matches!(event.kind, notify::EventKind::Access(_)) {
                return;
            }
            if event.paths.iter().any(|p| is_relevant(&filter_root, p)) {
                let refs = event.paths.iter().any(|p| touches_refs(&filter_root, p));
                let _ = tx.send(refs);
            }
        })?;
        inner.watch(&root, RecursiveMode::Recursive)?;
        std::thread::Builder::new()
            .name("retrogit-watch".into())
            .spawn(move || {
                // Ends when the watcher (and so `tx`) is dropped.
                while let Ok(mut refs) = rx.recv() {
                    let deadline = Instant::now() + DEBOUNCE;
                    loop {
                        match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                            Ok(r) => refs |= r,
                            Err(RecvTimeoutError::Timeout) => break,
                            Err(RecvTimeoutError::Disconnected) => return,
                        }
                    }
                    on_change(if refs { Refresh::Refs } else { Refresh::Status });
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
    fn windows_verbatim_prefixes_are_removed() {
        assert_eq!(
            super::strip_verbatim(r"\\?\C:\Users\me\repo"),
            r"C:\Users\me\repo"
        );
        assert_eq!(
            super::strip_verbatim(r"\\?\UNC\server\share\repo"),
            r"\\server\share\repo"
        );
        assert_eq!(
            super::strip_verbatim("/private/var/folders/x"),
            "/private/var/folders/x"
        );
        assert_eq!(super::strip_verbatim(r"C:\plain"), r"C:\plain");
    }

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
