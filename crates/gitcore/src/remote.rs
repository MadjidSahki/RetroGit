//! fetch / pull / push through the git command line, with progress and cancellation.

use std::io::Read;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::channel;
use std::time::Duration;

use crate::net::{NetAuth, askpass_program, net_settings};
use crate::{GitError, Refusal, Repo};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetProgress {
    /// e.g. "Receiving objects".
    pub phase: String,
    pub percent: Option<u8>,
}

/// Parse one line of `git --progress` output.
pub fn parse_progress(line: &str) -> Option<NetProgress> {
    let line = line.trim().trim_start_matches("remote:").trim();
    let (phase, rest) = line.split_once(':')?;
    let pct = rest.trim_start().split('%').next()?.trim();
    let percent = pct.parse::<u8>().ok().filter(|p| *p <= 100);
    let phase = phase.trim();
    (percent.is_some() && !phase.is_empty()).then(|| NetProgress {
        phase: phase.to_string(),
        percent,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PullMode {
    /// Fast-forward only; `Diverged` otherwise.
    FastForwardOnly,
    Merge,
    Rebase,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PullOutcome {
    UpToDate,
    FastForwarded,
    Merged,
    Rebased,
    /// Merge or rebase stopped on conflicts (see `operation_in_progress`).
    Conflicts,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PushMode {
    Normal,
    /// First push of a branch: `push -u origin <branch>`.
    SetUpstream,
}

/// Map a failed network command's output to an error.
/// `output` without git's progress lines (`remote: Resolving deltas: 66% ...`): they are
/// noise in an error message.
pub fn strip_progress(output: &str) -> String {
    output
        .lines()
        .filter(|l| {
            let t = l.trim();
            !t.is_empty() && parse_progress(t).is_none() && !t.starts_with("Enumerating objects")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn classify_net_failure(output: &str) -> GitError {
    let output = &strip_progress(output);
    let o = output.to_ascii_lowercase();
    if o.contains("without `workflow` scope") {
        GitError::MissingWorkflowScope(output.trim().to_string())
    } else if o.contains("[rejected]")
        && (o.contains("non-fast-forward") || o.contains("fetch first") || o.contains("stale info"))
    {
        GitError::PushRejected
    } else if o.contains("repository not found") || o.contains("returned error: 403") {
        GitError::AccessDenied(output.trim().to_string())
    } else if o.contains("authentication failed")
        || o.contains("could not read username")
        || o.contains("terminal prompts disabled")
        || o.contains("permission denied (publickey")
        || o.contains("returned error: 401")
    {
        GitError::Auth(output.trim().to_string())
    } else if o.contains("could not resolve host")
        || o.contains("connection timed out")
        || o.contains("failed to connect")
        || o.contains("network is unreachable")
    {
        GitError::Network(output.trim().to_string())
    } else {
        GitError::Other(output.trim().to_string())
    }
}

/// Run with RetroGit's token; if the server refuses it (e.g. the organization restricts the
/// OAuth App), try once more with the user's own git credentials (Keychain, credential
/// manager), like `git` in a terminal. If that fails too, the first error is returned.
pub fn retry_without_token<T>(
    auth: &NetAuth,
    mut run: impl FnMut(&NetAuth) -> Result<T, GitError>,
) -> Result<T, GitError> {
    match run(auth) {
        Err(GitError::AccessDenied(first)) if auth.github_token.is_some() => {
            match run(&NetAuth::default()) {
                Err(GitError::AccessDenied(_) | GitError::Auth(_)) => {
                    Err(GitError::AccessDenied(first))
                }
                other => other,
            }
        }
        // The user's own credentials (often the GitHub CLI's) may be allowed to change
        // workflows; if not, say what is missing.
        Err(GitError::MissingWorkflowScope(first)) if auth.github_token.is_some() => {
            match run(&NetAuth::default()) {
                Err(_) => Err(GitError::MissingWorkflowScope(first)),
                ok => ok,
            }
        }
        other => other,
    }
}

impl Repo {
    pub(crate) fn origin_url(&self) -> Option<String> {
        self.git()
            .find_remote("origin")
            .ok()
            .and_then(|r| r.url().ok().map(str::to_owned))
    }

    /// Run a network command; progress lines are reported; the process is killed on cancel.
    pub(crate) fn run_net(
        &self,
        auth: &NetAuth,
        args: &[&str],
        mut progress: impl FnMut(NetProgress),
        cancel: &AtomicBool,
    ) -> Result<String, GitError> {
        if !crate::git_available() {
            return Err(GitError::GitMissing);
        }
        let ssh_configured = crate::net::user_ssh_configured(
            std::env::var_os("GIT_SSH_COMMAND").is_some(),
            std::env::var_os("GIT_SSH").is_some(),
            self.git()
                .config()
                .ok()
                .and_then(|c| c.get_string("core.sshCommand").ok())
                .is_some(),
        );
        let settings = net_settings(
            &self.origin_url().unwrap_or_default(),
            auth,
            askpass_program(),
            ssh_configured,
        );
        let mut cmd = crate::commit::git_command();
        cmd.args(&settings.pre_args)
            .arg("-C")
            .arg(self.workdir()?)
            .args(args);
        for (k, v) in &settings.env {
            cmd.env(k, v);
        }
        // Output is parsed (rejections, auth failures): keep it untranslated.
        cmd.env("LC_ALL", "C").env("LANGUAGE", "C");
        // Own process group, so cancelling can kill git *and* its helpers (remote-https, ssh).
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
        let mut stderr = child
            .stderr
            .take()
            .ok_or_else(|| GitError::Other("no stderr".into()))?;
        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| GitError::Other("no stdout".into()))?;
        let stdout_reader = std::thread::spawn(move || {
            let mut out = String::new();
            let _ = stdout.read_to_string(&mut out);
            out
        });
        let (tx, rx) = channel::<String>();
        let reader = std::thread::spawn(move || {
            // Progress lines end with '\r' (updates) or '\n'.
            let mut all = String::new();
            let mut buf = [0u8; 4096];
            let mut line = Vec::new();
            while let Ok(n) = stderr.read(&mut buf) {
                if n == 0 {
                    break;
                }
                for &b in &buf[..n] {
                    if b == b'\r' || b == b'\n' {
                        let s = String::from_utf8_lossy(&line).into_owned();
                        all.push_str(&s);
                        all.push('\n');
                        let _ = tx.send(s);
                        line.clear();
                    } else {
                        line.push(b);
                    }
                }
            }
            all.push_str(&String::from_utf8_lossy(&line));
            all
        });
        let status = loop {
            while let Ok(l) = rx.try_recv() {
                if let Some(p) = parse_progress(&l) {
                    progress(p);
                }
            }
            if cancel.load(Ordering::Relaxed) {
                kill_tree(&mut child);
                // Do not join the readers: a helper that survived could keep the pipes open.
                drop(reader);
                drop(stdout_reader);
                return Err(GitError::Cancelled);
            }
            match child.try_wait() {
                Ok(Some(s)) => break s,
                Ok(None) => std::thread::sleep(Duration::from_millis(50)),
                Err(e) => return Err(GitError::Other(format!("git failed: {e}"))),
            }
        };
        let out = stdout_reader.join().unwrap_or_default();
        let err = reader.join().unwrap_or_default();
        let text = format!("{out}{err}");
        if status.success() {
            Ok(text)
        } else {
            Err(classify_net_failure(&text))
        }
    }

    pub fn fetch(
        &self,
        auth: &NetAuth,
        mut progress: impl FnMut(NetProgress),
        cancel: &AtomicBool,
    ) -> Result<(), GitError> {
        // Branches gone from origin are pruned, tags never: a user's `fetch.pruneTags` would
        // delete the tags never pushed (no reflog to get them back).
        let args = [
            "fetch",
            "--prune",
            "--no-prune-tags",
            "--progress",
            "origin",
        ];
        retry_without_token(auth, |a| self.run_net(a, &args, &mut progress, cancel)).map(|_| ())
    }

    /// Fetch, then integrate the upstream according to `mode`.
    pub fn pull(
        &self,
        auth: &NetAuth,
        mode: PullMode,
        progress: impl FnMut(NetProgress),
        cancel: &AtomicBool,
    ) -> Result<PullOutcome, GitError> {
        let branch = self
            .current_branch()
            .ok_or(GitError::Refused(Refusal::DetachedHead))?;
        if branch.upstream.is_none() {
            return Err(GitError::Refused(Refusal::NoUpstream { publish: true }));
        }
        self.fetch(auth, progress, cancel)?;
        let b = self
            .current_branch()
            .ok_or(GitError::Refused(Refusal::DetachedHead))?;
        if b.behind == 0 {
            return Ok(PullOutcome::UpToDate);
        }
        if b.ahead == 0 {
            self.git_ok(&["merge", "--ff-only", "@{u}"])?;
            return Ok(PullOutcome::FastForwarded);
        }
        let (args, done): (&[&str], PullOutcome) = match mode {
            PullMode::FastForwardOnly => {
                return Err(GitError::Diverged {
                    ahead: b.ahead,
                    behind: b.behind,
                });
            }
            PullMode::Merge => (&["merge", "--no-edit", "@{u}"], PullOutcome::Merged),
            PullMode::Rebase => (&["rebase", "@{u}"], PullOutcome::Rebased),
        };
        let out = self.run_git(args)?;
        if out.success {
            Ok(done)
        } else if self.operation_in_progress().is_some() {
            Ok(PullOutcome::Conflicts)
        } else {
            Err(crate::classify_commit_failure(&out.text))
        }
    }

    pub fn push(
        &self,
        auth: &NetAuth,
        mode: PushMode,
        mut progress: impl FnMut(NetProgress),
        cancel: &AtomicBool,
    ) -> Result<(), GitError> {
        let branch = self
            .current_branch()
            .ok_or(GitError::Refused(Refusal::DetachedHead))?;
        let args: Vec<&str> = match mode {
            PushMode::Normal => vec!["push", "--progress"],
            PushMode::SetUpstream => {
                vec!["push", "--progress", "-u", "origin", branch.name.as_str()]
            }
        };
        retry_without_token(auth, |a| self.run_net(a, &args, &mut progress, cancel)).map(|_| ())
    }
}

impl Repo {
    /// Force-push the current branch after rewriting pushed commits (amend), but only if the
    /// remote branch is still at `expected` — the commit it pointed to when we rewrote.
    /// A fetch in between cannot weaken the lease; `PushRejected` if someone pushed.
    pub fn push_force_with_lease(
        &self,
        auth: &NetAuth,
        expected: &str,
        mut progress: impl FnMut(NetProgress),
        cancel: &AtomicBool,
    ) -> Result<(), GitError> {
        let branch = self
            .current_branch()
            .ok_or(GitError::Refused(Refusal::DetachedHead))?;
        let upstream = branch
            .upstream
            .ok_or(GitError::Refused(Refusal::NoUpstream { publish: false }))?;
        let remote_branch = upstream
            .split_once('/')
            .map(|(_, b)| b.to_string())
            .unwrap_or(upstream);
        let lease = format!("--force-with-lease=refs/heads/{remote_branch}:{expected}");
        let refspec = format!("HEAD:refs/heads/{remote_branch}");
        let args = [
            "push",
            "--progress",
            lease.as_str(),
            "origin",
            refspec.as_str(),
        ];
        retry_without_token(auth, |a| self.run_net(a, &args, &mut progress, cancel)).map(|_| ())
    }
}

/// Kill git and every helper it started.
pub(crate) fn kill_tree(child: &mut std::process::Child) {
    #[cfg(unix)]
    {
        // The child leads its own process group (see `process_group(0)`).
        let _ = std::process::Command::new("kill")
            .args(["-KILL", &format!("-{}", child.id())])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let _ = std::process::Command::new("taskkill")
            .args(["/T", "/F", "/PID", &child.id().to_string()])
            .creation_flags(CREATE_NO_WINDOW)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_progress_lines() {
        assert_eq!(
            parse_progress("Receiving objects:  45% (450/1000), 1.2 MiB | 2 MiB/s"),
            Some(NetProgress {
                phase: "Receiving objects".into(),
                percent: Some(45)
            })
        );
        assert_eq!(
            parse_progress("remote: Counting objects: 100% (12/12), done."),
            Some(NetProgress {
                phase: "Counting objects".into(),
                percent: Some(100)
            })
        );
        assert_eq!(parse_progress("From github.com:o/r"), None);
        assert_eq!(parse_progress("To github.com:o/r.git"), None);
        assert_eq!(parse_progress(""), None);
    }

    #[test]
    fn a_push_changing_workflows_needs_the_workflow_scope() {
        let out = "remote: Resolving deltas: 100% (60/60)\nTo https://github.com/o/r.git\n ! [remote rejected] feat -> feat (refusing to allow an OAuth App to create or update workflow `.github/workflows/ci.yml` without `workflow` scope)\nerror: failed to push some refs";
        assert!(matches!(
            classify_net_failure(out),
            GitError::MissingWorkflowScope(_)
        ));
        // The user's own git credentials (often the GitHub CLI's) may have it.
        let auth = NetAuth {
            github_token: Some("gho_x".into()),
        };
        let mut calls = Vec::new();
        let r = retry_without_token(&auth, |a| {
            calls.push(a.github_token.is_some());
            if a.github_token.is_some() {
                Err(GitError::MissingWorkflowScope("x".into()))
            } else {
                Ok(1)
            }
        });
        assert_eq!((r, calls), (Ok(1), vec![true, false]));
    }

    #[test]
    fn progress_lines_are_left_out_of_error_details() {
        let out = "Enumerating objects: 138, done.\nremote: Resolving deltas:  66% (40/60)        \nremote: Resolving deltas: 100% (60/60), completed with 23 local objects.        \nTo https://github.com/o/r.git\n ! [remote rejected] main -> main (protected)\nerror: failed to push some refs";
        assert_eq!(
            strip_progress(out),
            "To https://github.com/o/r.git\n ! [remote rejected] main -> main (protected)\nerror: failed to push some refs"
        );
    }

    #[test]
    fn repository_not_found_and_403_mean_access_denied() {
        let nf = "remote: Repository not found.\nfatal: repository 'https://github.com/O/r.git/' not found";
        assert!(matches!(
            classify_net_failure(nf),
            GitError::AccessDenied(_)
        ));
        let forbidden = "fatal: unable to access 'https://github.com/O/r.git/': The requested URL returned error: 403";
        assert!(matches!(
            classify_net_failure(forbidden),
            GitError::AccessDenied(_)
        ));
    }

    #[test]
    fn a_refused_token_is_retried_once_with_the_users_git_credentials() {
        let auth = NetAuth {
            github_token: Some("gho_x".into()),
        };
        let mut calls = Vec::new();
        let r = retry_without_token(&auth, |a| {
            calls.push(a.github_token.is_some());
            if a.github_token.is_some() {
                Err(GitError::AccessDenied("not found".into()))
            } else {
                Ok(42)
            }
        });
        assert_eq!(r, Ok(42));
        assert_eq!(calls, vec![true, false]);
    }

    #[test]
    fn if_the_retry_fails_too_the_first_error_is_kept() {
        let auth = NetAuth {
            github_token: Some("gho_x".into()),
        };
        let r: Result<(), _> = retry_without_token(&auth, |a| {
            if a.github_token.is_some() {
                Err(GitError::AccessDenied("org restricts the app".into()))
            } else {
                Err(GitError::Auth("could not read Username".into()))
            }
        });
        assert_eq!(
            r,
            Err(GitError::AccessDenied("org restricts the app".into()))
        );
        // Without a token there is nothing to retry; other errors are not retried.
        let mut n = 0;
        let _: Result<(), _> = retry_without_token(&NetAuth::default(), |_| {
            n += 1;
            Err(GitError::AccessDenied("x".into()))
        });
        let _: Result<(), _> = retry_without_token(&auth, |_| {
            n += 1;
            Err(GitError::PushRejected)
        });
        assert_eq!(n, 2);
    }

    #[test]
    fn classifies_network_failures() {
        let rejected =
            " ! [rejected]        main -> main (fetch first)\nerror: failed to push some refs";
        assert_eq!(classify_net_failure(rejected), GitError::PushRejected);
        assert!(matches!(
            classify_net_failure("fatal: Authentication failed for 'https://github.com/o/r'"),
            GitError::Auth(_)
        ));
        assert!(matches!(
            classify_net_failure("git@github.com: Permission denied (publickey)."),
            GitError::Auth(_)
        ));
        assert!(matches!(
            classify_net_failure(
                "fatal: could not read Username for 'https://x': terminal prompts disabled"
            ),
            GitError::Auth(_)
        ));
        assert!(matches!(
            classify_net_failure("ssh: Could not resolve host github.com"),
            GitError::Network(_)
        ));
        assert!(matches!(
            classify_net_failure("remote: error: GH006: Protected branch update failed"),
            GitError::Other(_)
        ));
    }
}
