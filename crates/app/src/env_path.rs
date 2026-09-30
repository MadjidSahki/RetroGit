//! PATH for `git` and its hooks.

use std::sync::mpsc::channel;
use std::time::Duration;

const MARKER: &str = "__RG_PATH__";

/// The PATH printed between two markers (shell startup files may print other text).
pub fn extract_marked(output: &str) -> Option<String> {
    let start = output.find(MARKER)? + MARKER.len();
    let len = output[start..].find(MARKER)?;
    let path = output[start..start + len].trim();
    path.contains('/').then(|| path.to_string())
}

/// On macOS, apps started from the Finder/Dock get a minimal PATH (no Homebrew, no node),
/// which breaks `git` hooks such as husky or pre-commit. Ask the user's shell, as an
/// interactive login shell (so `~/.zprofile` *and* `~/.zshrc` are read: nvm, pyenv, pipx),
/// for the real PATH. Shell startup can be slow. Elsewhere, the inherited PATH is already
/// right: `None`. Blocking: call it from a background thread.
pub fn login_shell_path(timeout: Duration) -> Option<String> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let script = format!("printf '{MARKER}%s{MARKER}' \"$PATH\"");
        let out = std::process::Command::new(shell)
            .args(["-i", "-l", "-c", &script])
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output();
        let _ = tx.send(out);
    });
    let result = match rx.recv_timeout(timeout) {
        Ok(Ok(out)) => extract_marked(&String::from_utf8_lossy(&out.stdout)),
        Ok(Err(e)) => {
            log::warn!("cannot run the login shell to read PATH: {e}");
            None
        }
        Err(_) => {
            log::warn!("login shell did not print PATH within {timeout:?}");
            None
        }
    };
    if result.is_none() {
        log::warn!("hooks will run with the app's PATH");
    }
    result
}

#[cfg(test)]
mod tests {
    #[test]
    fn extracts_path_between_markers_ignoring_shell_noise() {
        let out = "Welcome!\n__RG_PATH__/opt/homebrew/bin:/usr/bin__RG_PATH__\nbye";
        assert_eq!(
            super::extract_marked(out).as_deref(),
            Some("/opt/homebrew/bin:/usr/bin")
        );
        assert_eq!(super::extract_marked("no markers /usr/bin"), None);
        assert_eq!(super::extract_marked("__RG_PATH____RG_PATH__"), None);
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn login_shell_path_includes_system_dirs() {
        let p = super::login_shell_path(std::time::Duration::from_secs(20)).unwrap_or_default();
        assert!(p.split(':').any(|d| d == "/usr/bin"), "{p}");
    }
}
