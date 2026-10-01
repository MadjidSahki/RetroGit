//! Watching the pull requests the user is involved in, for notifications.
//!
//! `watch_snapshot` takes a compact picture of those pull requests; `diff_snapshots`
//! compares two pictures and says what happened in between (pure, tested on its own).

use std::collections::HashMap;

use serde_json::json;

use crate::GithubError;
use crate::client::Client;
use crate::pulls::{
    ChecksState, PrState, ReviewState, checks_state, last_rollup, login, nodes, pr_state, text,
};

const WATCH_QUERY: &str = include_str!("queries/watch.graphql");

/// What the watcher knows about one pull request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrSnapshot {
    /// `owner/repo#number`.
    pub key: String,
    /// `owner/repo`.
    pub repo: String,
    pub number: u64,
    pub title: String,
    pub url: String,
    pub state: PrState,
    pub merged_by: Option<String>,
    /// Last commit of the head branch.
    pub head: String,
    pub checks: ChecksState,
    pub failed_checks: u32,
    pub review_count: u32,
    pub last_review: Option<(String, ReviewState)>,
    pub comment_count: u32,
    pub last_commenter: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrEventKind {
    ChecksPassed,
    ChecksFailed {
        failed: u32,
    },
    Approved {
        by: String,
    },
    ChangesRequested {
        by: String,
    },
    /// A conversation comment, or a review that only comments.
    Commented {
        by: String,
    },
    Merged,
    Closed,
}

/// Something that happened to a watched pull request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrEvent {
    pub key: String,
    pub repo: String,
    pub number: u64,
    pub title: String,
    pub url: String,
    pub kind: PrEventKind,
}

/// Events between two pictures. Pull requests absent from `prev` (first pass, new ones)
/// or from `next` (left the search) give nothing; the user's own actions are ignored.
pub fn diff_snapshots(prev: &[PrSnapshot], next: &[PrSnapshot], me: &str) -> Vec<PrEvent> {
    let before: HashMap<&str, &PrSnapshot> = prev.iter().map(|p| (p.key.as_str(), p)).collect();
    let mine = |who: &str| who.eq_ignore_ascii_case(me);
    let mut out = Vec::new();
    for n in next {
        let Some(p) = before.get(n.key.as_str()) else {
            continue;
        };
        let mut push = |kind| {
            out.push(PrEvent {
                key: n.key.clone(),
                repo: n.repo.clone(),
                number: n.number,
                title: n.title.clone(),
                url: n.url.clone(),
                kind,
            })
        };
        if p.state == PrState::Open && n.state != PrState::Open {
            let by_me = n.merged_by.as_deref().is_some_and(mine);
            match n.state {
                PrState::Merged if !by_me => push(PrEventKind::Merged),
                PrState::Closed => push(PrEventKind::Closed),
                _ => {}
            }
            continue;
        }
        let checks_settled = matches!(n.checks, ChecksState::Success | ChecksState::Failure);
        let was_running = p.checks == ChecksState::Pending || p.head != n.head;
        if checks_settled && was_running && (p.checks != n.checks || p.head != n.head) {
            push(match n.checks {
                ChecksState::Success => PrEventKind::ChecksPassed,
                _ => PrEventKind::ChecksFailed {
                    failed: n.failed_checks,
                },
            });
        }
        if n.review_count > p.review_count
            && let Some((by, state)) = &n.last_review
            && !mine(by)
        {
            let by = by.clone();
            match state {
                ReviewState::Approved => push(PrEventKind::Approved { by }),
                ReviewState::ChangesRequested => push(PrEventKind::ChangesRequested { by }),
                ReviewState::Commented => push(PrEventKind::Commented { by }),
                ReviewState::Dismissed => {}
            }
        }
        if n.comment_count > p.comment_count
            && let Some(by) = &n.last_commenter
            && !mine(by)
        {
            push(PrEventKind::Commented { by: by.clone() });
        }
    }
    out
}

/// `YYYY-MM-DD` of `days` days before `epoch_secs` (UTC), for search date qualifiers.
pub fn date_days_before(epoch_secs: i64, days: i64) -> String {
    // Howard Hinnant's civil-from-days algorithm.
    let z = epoch_secs.div_euclid(86_400) - days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

fn snapshot(n: &serde_json::Value) -> PrSnapshot {
    let repo = text(&n["repository"]["nameWithOwner"]);
    let number = n["number"].as_u64().unwrap_or_default();
    let rollup = last_rollup(&n["commits"]);
    let failed_checks = nodes(&rollup["contexts"])
        .filter(|c| {
            matches!(
                c["conclusion"].as_str().or(c["state"].as_str()),
                Some("FAILURE" | "ERROR" | "TIMED_OUT" | "ACTION_REQUIRED" | "STARTUP_FAILURE")
            )
        })
        .count() as u32;
    let review = &n["reviews"]["nodes"][0];
    let last_review = match review["state"].as_str() {
        Some("APPROVED") => Some(ReviewState::Approved),
        Some("CHANGES_REQUESTED") => Some(ReviewState::ChangesRequested),
        Some("COMMENTED") => Some(ReviewState::Commented),
        Some("DISMISSED") => Some(ReviewState::Dismissed),
        _ => None,
    }
    .map(|s| (login(&review["author"]), s));
    PrSnapshot {
        key: format!("{repo}#{number}"),
        repo,
        number,
        title: text(&n["title"]),
        url: text(&n["url"]),
        state: pr_state(&n["state"]),
        merged_by: n["mergedBy"]["login"].as_str().map(str::to_string),
        head: text(&n["commits"]["nodes"][0]["commit"]["oid"]),
        checks: checks_state(rollup),
        failed_checks,
        review_count: n["reviews"]["totalCount"].as_u64().unwrap_or_default() as u32,
        last_review,
        comment_count: n["comments"]["totalCount"].as_u64().unwrap_or_default() as u32,
        last_commenter: n["comments"]["nodes"][0]["author"]["login"]
            .as_str()
            .map(str::to_string),
    }
}

impl Client {
    /// Pull requests involving the token's user, updated since `since` (`YYYY-MM-DD`).
    /// Organizations refusing the token are skipped (partial results).
    pub fn watch_snapshot(&self, token: &str, since: &str) -> Result<Vec<PrSnapshot>, GithubError> {
        let q = format!("is:pr involves:@me updated:>={since} sort:updated-desc");
        let r = self.graphql_partial(token, WATCH_QUERY, json!({ "q": q }))?;
        Ok(nodes(&r.data["search"])
            .filter(|n| n["number"].is_u64())
            .map(snapshot)
            .collect())
    }
}

#[cfg(test)]
#[allow(clippy::cloned_ref_to_slice_refs)]
mod tests {
    use super::*;

    fn pr(n: u64) -> PrSnapshot {
        PrSnapshot {
            key: format!("o/r#{n}"),
            repo: "o/r".into(),
            number: n,
            title: format!("PR {n}"),
            url: format!("https://github.com/o/r/pull/{n}"),
            state: PrState::Open,
            merged_by: None,
            head: "h1".into(),
            checks: ChecksState::Pending,
            failed_checks: 0,
            review_count: 0,
            last_review: None,
            comment_count: 0,
            last_commenter: None,
        }
    }

    fn kinds(prev: &[PrSnapshot], next: &[PrSnapshot]) -> Vec<PrEventKind> {
        diff_snapshots(prev, next, "me")
            .into_iter()
            .map(|e| e.kind)
            .collect()
    }

    #[test]
    fn first_pass_or_no_change_gives_no_event() {
        assert!(kinds(&[], &[pr(1), pr(2)]).is_empty());
        assert!(kinds(&[pr(1)], &[pr(1)]).is_empty());
        let done = PrSnapshot {
            checks: ChecksState::Success,
            ..pr(1)
        };
        assert!(kinds(&[done.clone()], &[done]).is_empty(), "no repeat");
    }

    #[test]
    fn checks_pending_to_success_and_failure() {
        let ok = PrSnapshot {
            checks: ChecksState::Success,
            ..pr(1)
        };
        assert_eq!(kinds(&[pr(1)], &[ok]), [PrEventKind::ChecksPassed]);
        let ko = PrSnapshot {
            checks: ChecksState::Failure,
            failed_checks: 2,
            ..pr(1)
        };
        assert_eq!(
            kinds(&[pr(1)], &[ko]),
            [PrEventKind::ChecksFailed { failed: 2 }]
        );
    }

    #[test]
    fn checks_that_finished_between_two_polls_on_a_new_commit_count() {
        let old = PrSnapshot {
            checks: ChecksState::Success,
            ..pr(1)
        };
        let new = PrSnapshot {
            checks: ChecksState::Success,
            head: "h2".into(),
            ..pr(1)
        };
        assert_eq!(kinds(&[old], &[new]), [PrEventKind::ChecksPassed]);
        // A new commit without CI says nothing.
        let old = PrSnapshot {
            checks: ChecksState::Success,
            ..pr(1)
        };
        let new = PrSnapshot {
            checks: ChecksState::None,
            head: "h2".into(),
            ..pr(1)
        };
        assert!(kinds(&[old], &[new]).is_empty());
    }

    #[test]
    fn review_received_but_not_my_own() {
        let approved = PrSnapshot {
            review_count: 1,
            last_review: Some(("ada".into(), ReviewState::Approved)),
            ..pr(1)
        };
        assert_eq!(
            kinds(&[pr(1)], &[approved.clone()]),
            [PrEventKind::Approved { by: "ada".into() }]
        );
        let changes = PrSnapshot {
            review_count: 2,
            last_review: Some(("bob".into(), ReviewState::ChangesRequested)),
            ..pr(1)
        };
        assert_eq!(
            kinds(&[approved.clone()], &[changes]),
            [PrEventKind::ChangesRequested { by: "bob".into() }]
        );
        let commented = PrSnapshot {
            review_count: 2,
            last_review: Some(("bob".into(), ReviewState::Commented)),
            ..pr(1)
        };
        assert_eq!(
            kinds(&[approved.clone()], &[commented]),
            [PrEventKind::Commented { by: "bob".into() }]
        );
        let mine = PrSnapshot {
            review_count: 2,
            last_review: Some(("Me".into(), ReviewState::Approved)),
            ..pr(1)
        };
        assert!(kinds(&[approved], &[mine]).is_empty());
    }

    #[test]
    fn new_comment_by_someone_else() {
        let c = PrSnapshot {
            comment_count: 1,
            last_commenter: Some("ada".into()),
            ..pr(1)
        };
        assert_eq!(
            kinds(&[pr(1)], &[c]),
            [PrEventKind::Commented { by: "ada".into() }]
        );
        let own = PrSnapshot {
            comment_count: 1,
            last_commenter: Some("me".into()),
            ..pr(1)
        };
        assert!(kinds(&[pr(1)], &[own]).is_empty());
    }

    #[test]
    fn merged_and_closed() {
        let merged = PrSnapshot {
            state: PrState::Merged,
            merged_by: Some("ada".into()),
            checks: ChecksState::Success,
            ..pr(1)
        };
        assert_eq!(kinds(&[pr(1)], &[merged.clone()]), [PrEventKind::Merged]);
        assert!(kinds(&[merged.clone()], &[merged]).is_empty(), "once");
        let by_me = PrSnapshot {
            state: PrState::Merged,
            merged_by: Some("me".into()),
            ..pr(1)
        };
        assert!(kinds(&[pr(1)], &[by_me]).is_empty());
        let closed = PrSnapshot {
            state: PrState::Closed,
            ..pr(1)
        };
        assert_eq!(kinds(&[pr(1)], &[closed]), [PrEventKind::Closed]);
    }

    #[test]
    fn a_pr_leaving_the_search_is_not_closed() {
        assert!(kinds(&[pr(1), pr(2)], &[pr(2)]).is_empty());
    }

    #[test]
    fn events_carry_what_the_notification_needs() {
        let ok = PrSnapshot {
            checks: ChecksState::Success,
            ..pr(5)
        };
        let e = &diff_snapshots(&[pr(5)], &[ok], "me")[0];
        assert_eq!(
            (e.key.as_str(), e.repo.as_str(), e.number, e.title.as_str()),
            ("o/r#5", "o/r", 5, "PR 5")
        );
        assert_eq!(e.url, "https://github.com/o/r/pull/5");
    }

    #[test]
    fn dates_for_the_search() {
        assert_eq!(date_days_before(0, 0), "1970-01-01");
        // 2026-10-01T12:00:00Z
        assert_eq!(date_days_before(1_790_856_000, 0), "2026-10-01");
        assert_eq!(date_days_before(1_790_856_000, 7), "2026-09-24");
        assert_eq!(
            date_days_before(1_709_208_000, 1),
            "2024-02-28",
            "leap year"
        );
    }
}
