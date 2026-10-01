//! Worker side of sub-project 4: pull requests through the GitHub API.

use github::{Client, GithubError, NewPull, PrFilter};

use super::Worker;
use crate::protocol::{AppError, Command, Event, Op, Severity, Slug, SyncOp};
use crate::strings as s;

impl Worker {
    /// Run a GitHub call for `slug` with the right token (RetroGit's, or `gh`'s when the
    /// organization restricts OAuth Apps).
    fn on_github<T>(
        &self,
        slug: &Slug,
        call: impl Fn(&Client, &str, &str, &str) -> Result<T, GithubError>,
    ) -> Result<T, GithubError> {
        let (owner, repo) = slug;
        let client = &self.deps.client;
        self.deps
            .tokens
            .with_token(self.token.as_deref(), owner, |t| {
                crate::logging::add_secret(t);
                call(client, t, owner, repo)
            })
    }

    /// Report a GitHub failure; a restricted organization gets a link to approve RetroGit.
    fn github_failed(&mut self, during: Op, e: &GithubError) {
        if *e == GithubError::Unauthorized && self.token.is_some() {
            return self.drop_token(during);
        }
        let mut error = AppError::from_github(e);
        if matches!(e, GithubError::OAuthRestricted { .. }) {
            error.link = Some(self.sso_settings_link());
        }
        self.fail(during, error);
    }

    pub(super) fn handle_pulls(&mut self, cmd: Command) {
        match cmd {
            Command::LoadPulls { slug, filter } => self.load_pulls(slug, filter),
            Command::LoadPull { slug, number } => self.load_pull(&slug, number),
            Command::LoadRepoMeta(slug) => {
                match self.on_github(&slug, |c, t, o, r| c.repo_meta(t, o, r)) {
                    Ok(meta) => self.emit(Event::RepoMetaLoaded { slug, meta }),
                    Err(e) => self.github_failed(Op::Pulls, &e),
                }
            }
            Command::CreatePull {
                slug,
                pull,
                publish,
            } => self.create_pull(slug, pull, publish),
            Command::SubmitReview {
                slug,
                number,
                review,
            } => self.pull_action(&slug, number, s::NOTE_REVIEW_SENT, |c, t, o, r| {
                c.submit_review(t, o, r, number, &review)
            }),
            Command::ReplyToThread {
                slug,
                number,
                comment_id,
                body,
            } => self.pull_action(&slug, number, s::NOTE_COMMENTED, |c, t, o, r| {
                c.reply_to_thread(t, o, r, number, comment_id, &body)
            }),
            Command::AddPullComment { slug, number, body } => {
                self.pull_action(&slug, number, s::NOTE_COMMENTED, |c, t, o, r| {
                    c.add_issue_comment(t, o, r, number, &body)
                })
            }
            Command::SetLabels {
                slug,
                number,
                labels,
            } => self.pull_action(&slug, number, s::NOTE_LABELS, |c, t, o, r| {
                c.set_labels(t, o, r, number, &labels)
            }),
            Command::MergePull {
                slug,
                number,
                merge,
                delete_branch,
            } => self.merge_pull(slug, number, &merge, delete_branch),
            Command::CheckoutPull { number, head } => self.checkout_pull(number, head),
            _ => {}
        }
    }

    fn load_pulls(&mut self, slug: Slug, filter: PrFilter) {
        match self.on_github(&slug, |c, t, o, r| c.list_pulls(t, o, r, filter)) {
            Ok(list) => self.emit(Event::PullsLoaded { slug, filter, list }),
            Err(e) => self.github_failed(Op::Pulls, &e),
        }
    }

    fn load_pull(&mut self, slug: &Slug, number: u64) {
        match self.on_github(slug, |c, t, o, r| c.pull_detail(t, o, r, number)) {
            Ok(detail) => self.emit(Event::PullLoaded {
                slug: slug.clone(),
                detail: Box::new(detail),
            }),
            Err(e) => return self.github_failed(Op::Pulls, &e),
        }
        match self.on_github(slug, |c, t, o, r| c.pull_files(t, o, r, number)) {
            Ok(files) => self.emit(Event::PullFilesLoaded {
                slug: slug.clone(),
                number,
                files,
            }),
            Err(e) => self.github_failed(Op::Pulls, &e),
        }
    }

    /// A change, then the reloaded pull request.
    fn pull_action(
        &mut self,
        slug: &Slug,
        number: u64,
        note: &str,
        call: impl Fn(&Client, &str, &str, &str) -> Result<(), GithubError>,
    ) {
        match self.on_github(slug, call) {
            Ok(()) => self.emit(Event::PullActionDone {
                number,
                note: note.to_string(),
            }),
            Err(e) => self.github_failed(Op::PullAction, &e),
        }
        self.load_pull(slug, number);
    }

    fn create_pull(&mut self, slug: Slug, pull: NewPull, publish: bool) {
        if publish {
            // Failures are reported as `Op::Sync` errors, which also end the "busy" state.
            let Some((repo, result)) = self.network(SyncOp::Push, false, |r, a, p, c| {
                r.push(a, gitcore::PushMode::SetUpstream, p, c)
            }) else {
                return;
            };
            let pushed = result.is_ok();
            match result {
                Ok(()) => self.emit(Event::SyncFinished {
                    op: SyncOp::Push,
                    ok: true,
                }),
                Err(e) => self.net_failed(SyncOp::Push, false, &e),
            }
            self.after_ref_change(&repo);
            if !pushed {
                return;
            }
        }
        let created = self.on_github(&slug, |c, t, o, r| c.create_pull(t, o, r, &pull));
        let number = match created {
            Ok(n) => n,
            Err(GithubError::Rejected {
                status: 422,
                message,
            }) if message.contains("already exists") => {
                let owner = slug.0.clone();
                match self.on_github(&slug, |c, t, o, r| {
                    c.find_open_pull(t, o, r, &owner, &pull.head)
                }) {
                    Ok(Some(n)) => {
                        self.fail(
                            Op::PullAction,
                            AppError::new(Severity::Info, s::INFO_PULL_EXISTS),
                        );
                        n
                    }
                    _ => {
                        let e = GithubError::Rejected {
                            status: 422,
                            message,
                        };
                        return self.github_failed(Op::PullAction, &e);
                    }
                }
            }
            Err(e) => return self.github_failed(Op::PullAction, &e),
        };
        self.emit(Event::PullCreated {
            slug: slug.clone(),
            number,
        });
        self.load_pull(&slug, number);
    }

    fn merge_pull(
        &mut self,
        slug: Slug,
        number: u64,
        merge: &github::Merge,
        delete_branch: Option<String>,
    ) {
        match self.on_github(&slug, |c, t, o, r| c.merge_pull(t, o, r, number, merge)) {
            Ok(()) => {
                if let Some(branch) = delete_branch
                    && let Err(e) =
                        self.on_github(&slug, |c, t, o, r| c.delete_branch(t, o, r, &branch))
                {
                    let mut w = AppError::new(Severity::Warning, s::WARN_BRANCH_NOT_DELETED);
                    w.detail = Some(e.to_string());
                    self.fail(Op::PullAction, w);
                }
                self.emit(Event::PullActionDone {
                    number,
                    note: s::NOTE_MERGED.to_string(),
                });
                // The base branch moved on GitHub: bring the remote branches up to date.
                if self.repo.is_some() {
                    self.fetch(true);
                }
            }
            Err(GithubError::Rejected { status: 409, .. }) => self.fail(
                Op::PullAction,
                AppError::new(Severity::Info, s::INFO_PULL_MOVED),
            ),
            Err(e) => self.github_failed(Op::PullAction, &e),
        }
        self.load_pull(&slug, number);
    }

    /// Same-repository pull requests: their branch, tracking `origin`. Forks: `pr/N`.
    fn checkout_pull(&mut self, number: u64, head: Option<String>) {
        let Some(repo) = self.open_current(Op::PullAction) else {
            return;
        };
        let auth = self.net_auth();
        let never = std::sync::atomic::AtomicBool::new(false);
        let target = match head {
            Some(branch) => repo.fetch(&auth, |_| {}, &never).map(|()| {
                let local = repo
                    .branches()
                    .unwrap_or_default()
                    .iter()
                    .any(|b| !b.remote && b.name == branch);
                if local {
                    branch
                } else {
                    format!("origin/{branch}")
                }
            }),
            None => repo.fetch_pull(number, &auth, &never),
        };
        drop(repo);
        match target {
            Ok(name) => {
                // Local changes in the way: the usual "stash and switch" dialog appears.
                self.switch_branch(&name, false);
                let branch = name.trim_start_matches("origin/");
                let repo = self.open_current(Op::PullAction);
                let switched = repo
                    .as_ref()
                    .and_then(|r| r.current_branch())
                    .is_some_and(|b| b.name == branch);
                // A branch checked out before may be behind the pull request: follow
                // `origin` when the local branch has no commits of its own.
                if switched
                    && !name.starts_with("origin/")
                    && !branch.starts_with("pr/")
                    && let Some(r) = &repo
                {
                    match r.fast_forward(&format!("origin/{branch}")) {
                        Ok(true) => self.after_ref_change(r),
                        Ok(false) => {}
                        Err(e) => self.fail(Op::PullAction, AppError::from_git(&e)),
                    }
                }
                if switched {
                    self.emit(Event::PullActionDone {
                        number,
                        note: format!("{} {branch}", s::NOTE_CHECKED_OUT),
                    });
                }
            }
            Err(e) => self.fail(Op::PullAction, AppError::from_git(&e)),
        }
    }
}
