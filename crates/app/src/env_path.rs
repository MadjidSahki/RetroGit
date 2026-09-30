//! PATH for `git` and its hooks.

use std::sync::mpsc::channel;
use std::time::Duration;

/// On macOS, apps started from the Finder/Dock get a minimal PATH (no Homebrew, no node),
/// which breaks `git` hooks such as husky or pre-commit. Ask the login shell for the user's
/// real PATH (shell startup can be slow: nvm, conda...). Elsewhere, the inherited PATH is
/// already right: `None`. Blocking: call it from a background thread.
pub fn login_shell_path(timeout: Duration) -> Option<String> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let out = std::process::Command::new(shell)
            .args(["-l", "-c", "printf '%s' \"$PATH\""])
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output();
        let _ = tx.send(out);
    });
    let out = rx.recv_timeout(timeout).ok()?.ok()?;
    let path = String::from_utf8(out.stdout).ok()?.trim().to_string();
    (out.status.success() && path.contains('/')).then_some(path)
}

#[cfg(test)]
mod tests {
    #[test]
    #[cfg(target_os = "macos")]
    fn login_shell_path_includes_system_dirs() {
        let p = super::login_shell_path(std::time::Duration::from_secs(20)).unwrap_or_default();
        assert!(p.split(':').any(|d| d == "/usr/bin"), "{p}");
    }
}
