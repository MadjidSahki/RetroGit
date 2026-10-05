//! Running the `git` command line inside a repository.

use std::process::Stdio;

use crate::{GitError, Repo};

/// stdout + stderr of a finished `git` process.
pub(crate) struct GitOutput {
    pub success: bool,
    pub stdout: String,
    pub text: String,
}

impl Repo {
    /// Run `git -C <workdir> <args>` without a terminal and return its output.
    pub(crate) fn run_git(&self, args: &[&str]) -> Result<GitOutput, GitError> {
        self.run_git_env(args, &[])
    }

    /// [`Repo::run_git`] with extra environment variables (they win over the user's).
    pub(crate) fn run_git_env(
        &self,
        args: &[&str],
        env: &[(&str, &str)],
    ) -> Result<GitOutput, GitError> {
        if !crate::git_available() {
            return Err(GitError::GitMissing);
        }
        let out = crate::commit::git_command()
            .arg("-C")
            .arg(self.workdir()?)
            .args(args)
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_EDITOR", "true")
            // Messages are parsed: keep them untranslated (Git for Windows, Homebrew builds).
            .env("LC_ALL", "C")
            .env("LANGUAGE", "C")
            .envs(env.iter().copied())
            .stdin(Stdio::null())
            .output()
            .map_err(|e| GitError::Other(format!("cannot run git: {e}")))?;
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        let text = format!("{stdout}{}", String::from_utf8_lossy(&out.stderr))
            .trim()
            .to_string();
        Ok(GitOutput {
            success: out.status.success(),
            stdout,
            text,
        })
    }

    /// Like `run_git`, but the process is killed (`GitError::Cancelled`) as soon as `cancel`
    /// is set. For long reads: blame, log searches, grep.
    pub(crate) fn run_git_cancel(
        &self,
        args: &[&str],
        cancel: &std::sync::atomic::AtomicBool,
    ) -> Result<GitOutput, GitError> {
        self.run_git_capped(args, cancel, usize::MAX)
            .map(|(out, _)| out)
    }

    /// [`Repo::run_git_cancel`] keeping the first `max_lines` lines of stdout: git is stopped
    /// once more arrive (`true`: the output was cut).
    pub(crate) fn run_git_capped(
        &self,
        args: &[&str],
        cancel: &std::sync::atomic::AtomicBool,
        max_lines: usize,
    ) -> Result<(GitOutput, bool), GitError> {
        use std::io::{BufRead, Read};
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};
        if !crate::git_available() {
            return Err(GitError::GitMissing);
        }
        if cancel.load(Ordering::Relaxed) {
            return Err(GitError::Cancelled);
        }
        let mut cmd = crate::commit::git_command();
        // Untranslated messages, but UTF-8 characters: case-insensitive searches must fold
        // accented letters (`CAFÉ` finds `café`), which the C locale does not.
        cmd.arg("-C")
            .arg(self.workdir()?)
            .args(args)
            .env("GIT_TERMINAL_PROMPT", "0")
            .env_remove("LC_ALL")
            .env("LC_CTYPE", utf8_locale())
            .env("LC_MESSAGES", "C")
            .env("LANGUAGE", "C");
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);
        }
        let mut child = cmd
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| GitError::Other(format!("cannot run git: {e}")))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| GitError::Other("no stdout".into()))?;
        let mut stderr = child
            .stderr
            .take()
            .ok_or_else(|| GitError::Other("no stderr".into()))?;
        let enough = Arc::new(AtomicBool::new(false));
        let full = enough.clone();
        let out_reader = std::thread::spawn(move || {
            let mut reader = std::io::BufReader::new(stdout);
            let mut kept = Vec::new();
            let mut line = Vec::new();
            let mut lines = 0;
            while reader.read_until(b'\n', &mut line).unwrap_or(0) > 0 {
                if lines == max_lines {
                    full.store(true, Ordering::Relaxed);
                    break;
                }
                kept.append(&mut line);
                lines += 1;
            }
            kept
        });
        let err_reader = std::thread::spawn(move || {
            let mut b = Vec::new();
            let _ = stderr.read_to_end(&mut b);
            b
        });
        let status = loop {
            if cancel.load(Ordering::Relaxed) {
                crate::remote::kill_tree(&mut child);
                let _ = child.wait();
                return Err(GitError::Cancelled);
            }
            if enough.load(Ordering::Relaxed) {
                crate::remote::kill_tree(&mut child);
                break child.wait().ok();
            }
            match child.try_wait() {
                Ok(Some(s)) => break Some(s),
                Ok(None) => std::thread::sleep(std::time::Duration::from_millis(10)),
                Err(e) => return Err(GitError::Other(format!("git failed: {e}"))),
            }
        };
        // The reader may still be on the last lines once git has exited: wait for it.
        let stdout = String::from_utf8_lossy(&out_reader.join().unwrap_or_default()).into_owned();
        let truncated = enough.load(Ordering::Relaxed);
        let err = if truncated {
            Vec::new()
        } else {
            err_reader.join().unwrap_or_default()
        };
        let err = String::from_utf8_lossy(&err).into_owned();
        let success = truncated || status.is_some_and(|s| s.success());
        Ok((
            GitOutput {
                success,
                text: format!("{stdout}{err}").trim().to_string(),
                stdout,
            },
            truncated,
        ))
    }

    /// Like `run_git`, but a failure becomes `GitError::Other(output)`.
    pub(crate) fn git_ok(&self, args: &[&str]) -> Result<GitOutput, GitError> {
        let out = self.run_git(args)?;
        if out.success {
            Ok(out)
        } else {
            Err(GitError::Other(out.text))
        }
    }
}

/// First UTF-8 locale `exists` accepts: C.UTF-8 (Linux, macOS 15+, Git for Windows), else
/// en_US.UTF-8 (every macOS). Defaults to C.UTF-8.
pub(crate) fn pick_utf8_locale(exists: impl Fn(&str) -> bool) -> &'static str {
    ["C.UTF-8", "en_US.UTF-8"]
        .into_iter()
        .find(|l| exists(l))
        .unwrap_or("C.UTF-8")
}

/// UTF-8 locale for git's character classes (looked up once).
fn utf8_locale() -> &'static str {
    static LOCALE: std::sync::OnceLock<&'static str> = std::sync::OnceLock::new();
    LOCALE.get_or_init(|| {
        if cfg!(target_os = "macos") {
            pick_utf8_locale(|l| std::path::Path::new("/usr/share/locale").join(l).exists())
        } else {
            "C.UTF-8"
        }
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    #[test]
    fn a_utf8_locale_that_exists_is_chosen() {
        assert_eq!(super::pick_utf8_locale(|_| true), "C.UTF-8");
        // macOS 14 has no C.UTF-8 (git would fall back to C: no accent folding).
        assert_eq!(super::pick_utf8_locale(|l| l != "C.UTF-8"), "en_US.UTF-8");
        assert_eq!(super::pick_utf8_locale(|_| false), "C.UTF-8");
    }

    #[test]
    fn git_runs_with_untranslated_messages() {
        if !crate::git_available() {
            return;
        }
        let d = tempfile::tempdir().unwrap();
        git2::Repository::init(d.path()).unwrap();
        let r = crate::Repo::open(d.path()).unwrap();
        // RetroGit parses git's messages: they must not be translated (Git for Windows, Homebrew).
        let out = r.run_git(&["-c", "alias.showenv=!env", "showenv"]).unwrap();
        assert!(
            out.stdout.lines().any(|l| l == "LC_ALL=C"),
            "{}",
            out.stdout
        );
        assert!(out.stdout.lines().any(|l| l == "LANGUAGE=C"));
    }
}
