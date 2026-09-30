//! Environment for network git commands: never prompt, and hand the RetroGit token to git
//! for github.com HTTPS remotes through GIT_ASKPASS.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Credentials RetroGit can offer to `git`.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct NetAuth {
    pub github_token: Option<String>,
}

impl std::fmt::Debug for NetAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NetAuth")
            .field("github_token", &self.github_token.as_ref().map(|_| "***"))
            .finish()
    }
}

/// Environment variable carrying the token to the askpass helper (child process only).
pub const ASKPASS_TOKEN_VAR: &str = "RETROGIT_ASKPASS_TOKEN";

static ASKPASS_PROGRAM: OnceLock<PathBuf> = OnceLock::new();

/// Program `git` runs to ask for credentials: the RetroGit executable (`--askpass` mode).
pub fn set_askpass_program(path: PathBuf) {
    let _ = ASKPASS_PROGRAM.set(path);
}

/// What the askpass helper prints for git's `prompt`.
pub fn askpass_answer(prompt: &str, token: &str) -> String {
    if prompt.to_ascii_lowercase().contains("username") {
        "x-access-token".to_string()
    } else {
        token.to_string()
    }
}

/// SSH for network commands: never prompt, give up on unreachable hosts after 15 s.
pub const SSH_COMMAND: &str = "ssh -o BatchMode=yes -o ConnectTimeout=15";

/// Extra arguments (before the subcommand) and environment for a network command.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NetSettings {
    pub pre_args: Vec<String>,
    pub env: Vec<(String, String)>,
}

/// Pure: settings for a remote `url`. The token is only used for `https://github.com/`.
pub fn net_settings(
    url: &str,
    auth: &NetAuth,
    askpass: Option<&Path>,
    ssh_configured: bool,
) -> NetSettings {
    let mut s = NetSettings::default();
    s.env.push(("GIT_TERMINAL_PROMPT".into(), "0".into()));
    // Give up on a stalled HTTP transfer (< 1 KB/s for 30 s) instead of waiting forever.
    s.env
        .push(("GIT_HTTP_LOW_SPEED_LIMIT".into(), "1000".into()));
    s.env.push(("GIT_HTTP_LOW_SPEED_TIME".into(), "30".into()));
    if !ssh_configured {
        // Fail fast instead of waiting for a passphrase nobody can type.
        s.env.push(("GIT_SSH_COMMAND".into(), SSH_COMMAND.into()));
    }
    if let (true, Some(token), Some(program)) = (
        url.starts_with("https://github.com/"),
        &auth.github_token,
        askpass,
    ) {
        // Ignore stale credentials from helpers (Keychain...) for this call only.
        s.pre_args
            .extend(["-c".to_string(), "credential.helper=".to_string()]);
        s.env
            .push(("GIT_ASKPASS".into(), program.display().to_string()));
        s.env.push((ASKPASS_TOKEN_VAR.into(), token.clone()));
    }
    s
}

pub(crate) fn askpass_program() -> Option<&'static Path> {
    ASKPASS_PROGRAM.get().map(PathBuf::as_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn token() -> NetAuth {
        NetAuth {
            github_token: Some("gho_x".into()),
        }
    }

    #[test]
    fn github_https_gets_the_token_through_askpass() {
        let s = net_settings(
            "https://github.com/o/r.git",
            &token(),
            Some(Path::new("/app/retrogit")),
            false,
        );
        assert_eq!(s.pre_args, vec!["-c", "credential.helper="]);
        assert!(
            s.env
                .contains(&("GIT_ASKPASS".into(), "/app/retrogit".into()))
        );
        assert!(s.env.contains(&(ASKPASS_TOKEN_VAR.into(), "gho_x".into())));
        assert!(s.env.contains(&("GIT_TERMINAL_PROMPT".into(), "0".into())));
    }

    #[test]
    fn ssh_and_other_hosts_never_see_the_token() {
        for url in [
            "git@github.com:o/r.git",
            "https://gitlab.com/o/r.git",
            "https://github.company.com/o/r",
        ] {
            let s = net_settings(url, &token(), Some(Path::new("/app/retrogit")), false);
            assert!(s.pre_args.is_empty(), "{url}");
            assert!(
                s.env
                    .iter()
                    .all(|(k, _)| k != ASKPASS_TOKEN_VAR && k != "GIT_ASKPASS"),
                "{url}"
            );
            assert!(
                s.env
                    .contains(&("GIT_SSH_COMMAND".into(), SSH_COMMAND.into()))
            );
            assert!(
                s.env
                    .contains(&("GIT_HTTP_LOW_SPEED_TIME".into(), "30".into()))
            );
        }
    }

    #[test]
    fn a_custom_ssh_command_is_left_alone() {
        let s = net_settings("git@github.com:o/r.git", &NetAuth::default(), None, true);
        assert!(s.env.iter().all(|(k, _)| k != "GIT_SSH_COMMAND"));
    }

    #[test]
    fn askpass_answers_username_then_token() {
        assert_eq!(
            askpass_answer("Username for 'https://github.com': ", "t"),
            "x-access-token"
        );
        assert_eq!(
            askpass_answer("Password for 'https://x-access-token@github.com': ", "t"),
            "t"
        );
    }

    #[test]
    fn debug_hides_the_token() {
        assert!(!format!("{:?}", token()).contains("gho_x"));
    }
}
