use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::OnceLock;

use crate::{CommitInfo, GitError, Repo};

/// How to create commits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommitBackend {
    /// `git commit` (hooks, signing, global config); falls back to libgit2 if `git` is missing.
    PreferCli,
    /// libgit2 only: no hooks, no signing.
    Git2,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitOutcome {
    pub commit: CommitInfo,
    /// `false` when the commit was made by libgit2 (hooks and signing skipped).
    pub used_cli: bool,
}

const MAX_OUTPUT: usize = 20_000;
static SEARCH_PATH: OnceLock<String> = OnceLock::new();
static GIT_AVAILABLE: OnceLock<bool> = OnceLock::new();

/// PATH used to find `git` and given to hooks. GUI apps on macOS start with a minimal PATH
/// (no Homebrew), so the app passes the login shell's PATH here once at startup.
pub fn set_git_search_path(path: String) {
    let _ = SEARCH_PATH.set(path);
}

pub(crate) fn git_command() -> Command {
    let mut cmd = Command::new("git");
    if let Some(p) = SEARCH_PATH.get() {
        cmd.env("PATH", p);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// Whether a `git` executable can be run.
pub fn git_available() -> bool {
    *GIT_AVAILABLE.get_or_init(|| {
        git_command()
            .arg("--version")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    })
}

/// Turn the output of a failed `git commit` into an error.
pub fn classify_commit_failure(output: &str) -> GitError {
    if output.contains("Please tell me who you are") || output.contains("Author identity unknown") {
        return GitError::MissingIdentity;
    }
    let mut output = output.trim().to_string();
    if output.len() > MAX_OUTPUT {
        let mut cut = MAX_OUTPUT;
        while !output.is_char_boundary(cut) {
            cut -= 1;
        }
        output.truncate(cut);
        output.push_str("\n[...]");
    }
    GitError::CommitRejected { output }
}

impl Repo {
    /// Commit the index. With `amend`, replace HEAD (keeping its parents).
    pub fn commit(
        &self,
        message: &str,
        amend: bool,
        backend: CommitBackend,
    ) -> Result<CommitOutcome, GitError> {
        let used_cli = backend == CommitBackend::PreferCli && git_available();
        if used_cli {
            self.commit_cli(message, amend)?;
        } else if self.signing_config().is_ok_and(|s| s.enabled) {
            // libgit2 cannot sign: never create an unsigned commit when signing is required.
            return Err(GitError::SigningRequiresGit);
        } else {
            self.commit_git2(message, amend)?;
        }
        let commit = self
            .summary()?
            .last_commit
            .ok_or_else(|| GitError::Other("commit not found".into()))?;
        Ok(CommitOutcome { commit, used_cli })
    }

    fn commit_cli(&self, message: &str, amend: bool) -> Result<(), GitError> {
        let mut cmd = git_command();
        cmd.arg("-C")
            .arg(self.workdir()?)
            .args(["commit", "--cleanup=whitespace", "-F", "-"]);
        if amend {
            cmd.arg("--amend");
        }
        let mut child = cmd
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_EDITOR", "true")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| GitError::Other(format!("cannot run git: {e}")))?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(message.as_bytes())
                .map_err(|e| GitError::Other(format!("cannot send the message to git: {e}")))?;
        }
        let out = child
            .wait_with_output()
            .map_err(|e| GitError::Other(format!("git failed: {e}")))?;
        if out.status.success() {
            return Ok(());
        }
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        Err(classify_commit_failure(&text))
    }

    fn commit_git2(&self, message: &str, amend: bool) -> Result<(), GitError> {
        let repo = self.git();
        let map = |e: git2::Error| GitError::from_git2(&e);
        let sig = repo.signature().map_err(|_| GitError::MissingIdentity)?;
        let tree = {
            let mut index = repo.index().map_err(map)?;
            let id = index.write_tree().map_err(map)?;
            repo.find_tree(id).map_err(map)?
        };
        let head = repo.head().ok().and_then(|h| h.peel_to_commit().ok());
        match (amend, head) {
            (true, Some(head)) => {
                head.amend(
                    Some("HEAD"),
                    None,
                    Some(&sig),
                    None,
                    Some(message),
                    Some(&tree),
                )
                .map_err(map)?;
            }
            (true, None) => {
                return Err(GitError::Unsupported("there is no commit to amend".into()));
            }
            (false, head) => {
                let parents: Vec<&git2::Commit<'_>> = head.iter().collect();
                repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &parents)
                    .map_err(map)?;
            }
        }
        Ok(())
    }

    /// Full message of HEAD, if any (used to pre-fill "Amend").
    pub fn last_commit_message(&self) -> Result<Option<String>, GitError> {
        Ok(self
            .git()
            .head()
            .ok()
            .and_then(|h| h.peel_to_commit().ok())
            .and_then(|c| c.message().ok().map(|m| m.trim_end().to_string())))
    }

    /// `true` if the current branch has an upstream that already contains HEAD.
    pub fn head_is_pushed(&self) -> Result<bool, GitError> {
        let repo = self.git();
        let Ok(head) = repo.head() else {
            return Ok(false);
        };
        if !head.is_branch() {
            return Ok(false);
        }
        let Some(head_id) = head.target() else {
            return Ok(false);
        };
        let Ok(name) = head.shorthand() else {
            return Ok(false);
        };
        let Ok(branch) = repo.find_branch(name, git2::BranchType::Local) else {
            return Ok(false);
        };
        let Ok(upstream) = branch.upstream() else {
            return Ok(false);
        };
        let Some(up_id) = upstream.get().target() else {
            return Ok(false);
        };
        Ok(up_id == head_id || repo.graph_descendant_of(up_id, head_id).unwrap_or(false))
    }
}
