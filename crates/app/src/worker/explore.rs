//! Explore service: answers file, blame, history and search requests on their own threads,
//! so the worker stays free. A newer request of a kind cancels the older one (its `git`
//! process is killed) and only answers that are still wanted are sent.

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use gitcore::{GitError, Repo};

use crate::protocol::{Event, ExploreRequest, ExploreResult};

/// Commits listed at most (file history, History filter) and matches of a search.
pub const EXPLORE_LIMIT: usize = 1000;

type Emit = Arc<dyn Fn(Event) + Send + Sync>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Lane {
    Refs,
    Tree,
    File,
    FileDiff,
    Blame,
    History,
    Grep,
    LogSearch,
}

fn lane(r: &ExploreRequest) -> Option<Lane> {
    Some(match r {
        ExploreRequest::Refs => Lane::Refs,
        ExploreRequest::Tree { .. } => Lane::Tree,
        ExploreRequest::File { .. } => Lane::File,
        ExploreRequest::FileDiff { .. } => Lane::FileDiff,
        ExploreRequest::Blame { .. } => Lane::Blame,
        ExploreRequest::FileHistory { .. } => Lane::History,
        ExploreRequest::Grep { .. } => Lane::Grep,
        ExploreRequest::LogSearch { .. } => Lane::LogSearch,
        ExploreRequest::CancelSearch | ExploreRequest::CancelLogSearch => return None,
    })
}

pub(super) struct ExploreService {
    emit: Emit,
    running: Mutex<HashMap<Lane, Arc<AtomicBool>>>,
}

impl ExploreService {
    pub(super) fn new(emit: Emit) -> ExploreService {
        ExploreService {
            emit,
            running: Mutex::new(HashMap::new()),
        }
    }

    fn cancel(&self, lane: Lane) {
        if let Ok(mut m) = self.running.lock()
            && let Some(flag) = m.remove(&lane)
        {
            flag.store(true, Ordering::SeqCst);
        }
    }

    pub(super) fn request(&self, repo: &Path, req: ExploreRequest) {
        let Some(lane) = lane(&req) else {
            self.cancel(match req {
                ExploreRequest::CancelSearch => Lane::Grep,
                _ => Lane::LogSearch,
            });
            return;
        };
        self.cancel(lane);
        let cancel = Arc::new(AtomicBool::new(false));
        if let Ok(mut m) = self.running.lock() {
            m.insert(lane, cancel.clone());
        }
        let emit = self.emit.clone();
        let repo = repo.to_path_buf();
        let spawned = std::thread::Builder::new()
            .name("retrogit-explore".into())
            .spawn(move || {
                let result = answer(&repo, &req, &cancel);
                // Superseded or cancelled meanwhile: nobody waits for it.
                if cancel.load(Ordering::SeqCst) {
                    return;
                }
                let result = match result {
                    Ok(r) => r,
                    Err(GitError::Cancelled) => return,
                    Err(e) => {
                        let a = crate::protocol::AppError::from_git(&e);
                        ExploreResult::Failed {
                            request: req,
                            message: match a.detail {
                                Some(d) => format!("{}\n{d}", a.message),
                                None => a.message,
                            },
                        }
                    }
                };
                emit(Event::ExploreLoaded { repo, result });
            });
        if let Err(e) = spawned {
            log::warn!("explore request not started: {e}");
        }
    }
}

fn answer(
    dir: &Path,
    req: &ExploreRequest,
    cancel: &AtomicBool,
) -> Result<ExploreResult, GitError> {
    let repo = Repo::open(dir)?;
    Ok(match req.clone() {
        ExploreRequest::Refs => ExploreResult::Refs(repo.explore_refs()),
        ExploreRequest::Tree { rev } => {
            let commit = match repo.resolve(&rev) {
                Some(c) => c,
                None if rev == "HEAD" => String::new(),
                None => return Err(GitError::Refused(gitcore::Refusal::UnknownRev(rev))),
            };
            ExploreResult::Tree {
                entries: if commit.is_empty() {
                    Vec::new()
                } else {
                    repo.tree(&commit)?
                },
                rev,
                commit,
            }
        }
        ExploreRequest::File { rev, path } => ExploreResult::File {
            content: repo.file_at(&rev, &path)?,
            rev,
            path,
        },
        ExploreRequest::FileDiff { commit, path } => ExploreResult::FileDiff {
            diff: repo.commit_file_diff(&commit, &path)?,
            commit,
            path,
        },
        ExploreRequest::Blame { rev, path } => ExploreResult::Blame {
            blocks: repo.blame(&rev, &path, cancel)?,
            rev,
            path,
        },
        ExploreRequest::FileHistory { rev, path } => ExploreResult::FileHistory {
            commits: repo.file_history(&rev, &path, EXPLORE_LIMIT, cancel)?,
            rev,
            path,
        },
        ExploreRequest::Grep {
            rev,
            text,
            match_case,
            paths,
        } => ExploreResult::Grep {
            result: repo.grep(&rev, &text, match_case, &paths, EXPLORE_LIMIT, cancel)?,
            rev,
            text,
        },
        ExploreRequest::LogSearch { kind, query } => {
            let (entries, truncated) = repo.search_log(kind, &query, EXPLORE_LIMIT, cancel)?;
            ExploreResult::LogSearch {
                kind,
                query,
                entries,
                truncated,
            }
        }
        ExploreRequest::CancelSearch | ExploreRequest::CancelLogSearch => {
            return Err(GitError::Cancelled);
        }
    })
}
