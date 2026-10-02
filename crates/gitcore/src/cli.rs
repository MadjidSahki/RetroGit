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

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

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
