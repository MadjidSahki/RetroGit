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
        if !crate::git_available() {
            return Err(GitError::GitMissing);
        }
        let out = crate::commit::git_command()
            .arg("-C")
            .arg(self.workdir()?)
            .args(args)
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_EDITOR", "true")
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
