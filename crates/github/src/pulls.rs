//! Reading pull requests: list (GraphQL search), detail (one GraphQL query), files (REST).

use serde::Deserialize;
use serde_json::{Value, json};

use crate::client::{Client, header};
use crate::{GithubError, next_link};

const LIST_QUERY: &str = include_str!("queries/list_pulls.graphql");
const DETAIL_QUERY: &str = include_str!("queries/pull_detail.graphql");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrState {
    Open,
    Closed,
    Merged,
}

/// Combined state of a commit's checks (GitHub's status check rollup).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChecksState {
    /// No check ran on the last commit.
    None,
    Pending,
    Success,
    Failure,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewDecision {
    /// The repository does not require reviews and none decided.
    None,
    Approved,
    ChangesRequested,
    ReviewRequired,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Label {
    pub name: String,
    pub color: [u8; 3],
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrSummary {
    pub number: u64,
    pub title: String,
    pub url: String,
    pub author: String,
    /// Source branch.
    pub head: String,
    /// Target branch.
    pub base: String,
    pub draft: bool,
    pub state: PrState,
    pub labels: Vec<Label>,
    pub checks: ChecksState,
    pub review_decision: ReviewDecision,
    /// ISO-8601, e.g. `2026-10-01T08:38:17Z`.
    pub updated_at: String,
}

/// Which pull requests the list shows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PrFilter {
    #[default]
    Open,
    /// Open, authored by me.
    Mine,
    /// Open, my review is requested.
    ReviewRequested,
    /// Closed or merged.
    Closed,
}

/// GitHub search query for `filter` in `owner/repo`, most recently updated first.
pub fn search_query(owner: &str, repo: &str, filter: PrFilter) -> String {
    let which = match filter {
        PrFilter::Open => "is:open",
        PrFilter::Mine => "is:open author:@me",
        PrFilter::ReviewRequested => "is:open review-requested:@me",
        PrFilter::Closed => "is:closed",
    };
    format!("repo:{owner}/{repo} is:pr {which} sort:updated-desc")
}

/// The pull requests of `owner/repo` on github.com, with the same search as `filter`.
pub fn pulls_web_url(owner: &str, repo: &str, filter: PrFilter) -> String {
    let q = crate::pulls_write::encode_segment(&search_query(owner, repo, filter), false);
    format!(
        "https://github.com/{owner}/{repo}/pulls?q={}",
        q.replace("%20", "+")
    )
}

/// Comments, reviews and line threads a pull request detail reads at most (each).
pub const DETAIL_PAGE: u32 = 100;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrCommit {
    pub oid: String,
    pub short_oid: String,
    pub headline: String,
    /// GitHub login when known, else the Git author name.
    pub author: String,
    pub date: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckStatus {
    Pending,
    Success,
    Failure,
    /// Skipped, neutral, cancelled.
    Neutral,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckRun {
    pub name: String,
    pub status: CheckStatus,
    pub url: Option<String>,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewState {
    Approved,
    ChangesRequested,
    Commented,
    Dismissed,
}

/// Conversation entry, in time order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TimelineItem {
    Comment {
        author: String,
        body: String,
        at: String,
    },
    Review {
        author: String,
        state: ReviewState,
        body: String,
        at: String,
    },
}

impl TimelineItem {
    pub fn at(&self) -> &str {
        match self {
            TimelineItem::Comment { at, .. } | TimelineItem::Review { at, .. } => at,
        }
    }
}

/// Side of a diff line: `Left` = old file (removed lines), `Right` = new file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffSide {
    Left,
    Right,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadComment {
    /// REST id (replies are posted to it).
    pub id: u64,
    pub author: String,
    /// The author's numeric user id (`None` for bots and deleted accounts).
    pub author_id: Option<u64>,
    pub body: String,
    pub at: String,
}

/// Comments attached to one line of the diff.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewThread {
    /// GraphQL node id (resolving uses it).
    pub id: String,
    pub can_resolve: bool,
    pub can_unresolve: bool,
    pub path: String,
    /// Line in the current diff; `None` when the thread is outdated.
    pub line: Option<u32>,
    pub original_line: Option<u32>,
    pub side: DiffSide,
    /// First line of a thread on several lines (`line` is the last one).
    pub start_line: Option<u32>,
    pub start_side: Option<DiffSide>,
    pub outdated: bool,
    pub resolved: bool,
    pub comments: Vec<ThreadComment>,
}

/// Someone asked for a review, or who reviewed (latest review state).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reviewer {
    pub login: String,
    /// `None`: requested, not reviewed yet.
    pub state: Option<ReviewState>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mergeable {
    Mergeable,
    Conflicting,
    /// GitHub is still computing it.
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeMethod {
    Merge,
    Squash,
    Rebase,
}

impl MergeMethod {
    /// Value of the REST `merge_method` field.
    pub fn api_name(self) -> &'static str {
        match self {
            MergeMethod::Merge => "merge",
            MergeMethod::Squash => "squash",
            MergeMethod::Rebase => "rebase",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrDetail {
    pub summary: PrSummary,
    pub body: String,
    /// Commit the PR head points to (reviews and merges are pinned to it).
    pub head_sha: String,
    /// `(owner, name)` of the repository holding the head branch; `None` if it was deleted.
    pub head_repo: Option<(String, String)>,
    /// The head branch lives in a fork.
    pub cross_repository: bool,
    pub commits: Vec<PrCommit>,
    pub commit_count: u32,
    /// Sizes of the conversation connections; above `DETAIL_PAGE`, only part is read.
    pub comments_total: u32,
    pub reviews_total: u32,
    pub threads_total: u32,
    pub check_runs: Vec<CheckRun>,
    pub timeline: Vec<TimelineItem>,
    pub threads: Vec<ReviewThread>,
    pub mergeable: Mergeable,
    /// GitHub's `mergeStateStatus`: CLEAN, BLOCKED, BEHIND, DIRTY, DRAFT, UNSTABLE...
    pub merge_state: String,
    pub allowed_methods: Vec<MergeMethod>,
    /// Signed-in login.
    pub viewer: String,
    pub viewer_is_author: bool,
    /// The viewer may merge and edit labels (write access or more).
    pub viewer_can_write: bool,
    pub repo_labels: Vec<Label>,
    /// GraphQL node id (draft / ready mutations).
    pub id: String,
    /// The viewer may edit the title, description, reviewers and assignees.
    pub viewer_can_update: bool,
    /// Requested reviewers first, then people who reviewed (latest state).
    pub reviewers: Vec<Reviewer>,
    pub assignees: Vec<String>,
}

/// A changed file of a pull request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrFile {
    pub path: String,
    pub previous_path: Option<String>,
    /// added, removed, modified, renamed, copied, changed, unchanged.
    pub status: String,
    pub additions: u32,
    pub deletions: u32,
    /// Unified diff hunks; `None` for binary files and diffs GitHub finds too large.
    pub patch: Option<String>,
}

#[derive(Deserialize)]
struct RawFile {
    filename: String,
    previous_filename: Option<String>,
    status: String,
    #[serde(default)]
    additions: u32,
    #[serde(default)]
    deletions: u32,
    patch: Option<String>,
}

impl Client {
    pub fn list_pulls(
        &self,
        token: &str,
        owner: &str,
        repo: &str,
        filter: PrFilter,
    ) -> Result<(Vec<PrSummary>, u32), GithubError> {
        // The first 50 results, and how many match in all.
        let data = self.graphql(
            token,
            LIST_QUERY,
            json!({ "q": search_query(owner, repo, filter), "owner": owner, "name": repo }),
        )?;
        Ok((
            nodes(&data["search"])
                .filter(|n| n["number"].is_u64())
                .map(summary)
                .collect(),
            count(&data["search"]["issueCount"]),
        ))
    }

    pub fn pull_detail(
        &self,
        token: &str,
        owner: &str,
        repo: &str,
        number: u64,
    ) -> Result<PrDetail, GithubError> {
        let data = self.graphql(
            token,
            DETAIL_QUERY,
            json!({ "owner": owner, "name": repo, "number": number }),
        )?;
        parse_detail(&data)
    }

    /// Changed files with their patches, following pagination (GitHub stops at 3000).
    pub fn pull_files(
        &self,
        token: &str,
        owner: &str,
        repo: &str,
        number: u64,
    ) -> Result<Vec<PrFile>, GithubError> {
        let mut url = format!(
            "{}/repos/{owner}/{repo}/pulls/{number}/files?per_page=100",
            self.api_base
        );
        let mut out = Vec::new();
        loop {
            let mut resp = self.api_get(&url, token)?;
            let next = header(&resp, "link").and_then(next_link);
            let page: Vec<RawFile> = resp.body_mut().read_json()?;
            out.extend(page.into_iter().map(|f| PrFile {
                path: f.filename,
                previous_path: f.previous_filename,
                status: f.status,
                additions: f.additions,
                deletions: f.deletions,
                patch: f.patch,
            }));
            match next {
                Some(n) => url = n,
                None => return Ok(out),
            }
        }
    }
}

/// `connection.nodes` as an iterator (empty when missing).
pub(crate) fn nodes(connection: &Value) -> impl Iterator<Item = &Value> {
    connection["nodes"].as_array().into_iter().flatten()
}

fn count(v: &Value) -> u32 {
    v.as_u64().unwrap_or_default().min(u32::MAX as u64) as u32
}

pub(crate) fn text(v: &Value) -> String {
    v.as_str().unwrap_or_default().to_string()
}

fn opt_text(v: &Value) -> Option<String> {
    v.as_str().filter(|s| !s.is_empty()).map(str::to_string)
}

/// Login of an `author { login }` object; deleted accounts show as "ghost" like on GitHub.
pub(crate) fn login(author: &Value) -> String {
    author["login"].as_str().unwrap_or("ghost").to_string()
}

/// `"d73a4a"` => `[0xd7, 0x3a, 0x4a]` (gray if malformed).
pub fn parse_color(hex: &str) -> [u8; 3] {
    let hex = hex.trim_start_matches('#');
    let byte = |i: usize| {
        hex.get(i..i + 2)
            .and_then(|h| u8::from_str_radix(h, 16).ok())
    };
    match (byte(0), byte(2), byte(4)) {
        (Some(r), Some(g), Some(b)) if hex.len() == 6 => [r, g, b],
        _ => [0xC0, 0xC0, 0xC0],
    }
}

pub(crate) fn labels(connection: &Value) -> Vec<Label> {
    nodes(connection)
        .map(|l| Label {
            name: text(&l["name"]),
            color: parse_color(l["color"].as_str().unwrap_or_default()),
            description: opt_text(&l["description"]),
        })
        .collect()
}

/// Labels of a REST response (array of `{ name, color, description }`).
pub(crate) fn labels_from_rest(array: &Value) -> Vec<Label> {
    labels(&json!({ "nodes": array }))
}

pub(crate) fn pr_state(v: &Value) -> PrState {
    match v.as_str() {
        Some("MERGED") => PrState::Merged,
        Some("CLOSED") => PrState::Closed,
        _ => PrState::Open,
    }
}

pub(crate) fn checks_state(rollup: &Value) -> ChecksState {
    match rollup["state"].as_str() {
        Some("SUCCESS") => ChecksState::Success,
        Some("FAILURE" | "ERROR") => ChecksState::Failure,
        Some("PENDING" | "EXPECTED") => ChecksState::Pending,
        _ => ChecksState::None,
    }
}

fn review_decision(v: &Value) -> ReviewDecision {
    match v.as_str() {
        Some("APPROVED") => ReviewDecision::Approved,
        Some("CHANGES_REQUESTED") => ReviewDecision::ChangesRequested,
        Some("REVIEW_REQUIRED") => ReviewDecision::ReviewRequired,
        _ => ReviewDecision::None,
    }
}

/// Rollup of the last commit (`commits(last: 1)` connection).
pub(crate) fn last_rollup(commits: &Value) -> &Value {
    &commits["nodes"][0]["commit"]["statusCheckRollup"]
}

fn summary(n: &Value) -> PrSummary {
    PrSummary {
        number: n["number"].as_u64().unwrap_or_default(),
        title: text(&n["title"]),
        url: text(&n["url"]),
        author: login(&n["author"]),
        head: text(&n["headRefName"]),
        base: text(&n["baseRefName"]),
        draft: n["isDraft"].as_bool().unwrap_or(false),
        state: pr_state(&n["state"]),
        labels: labels(&n["labels"]),
        checks: checks_state(last_rollup(&n["commits"])),
        review_decision: review_decision(&n["reviewDecision"]),
        updated_at: text(&n["updatedAt"]),
    }
}

fn check_run(c: &Value) -> CheckRun {
    if c["__typename"] == "StatusContext" {
        let status = match c["state"].as_str() {
            Some("SUCCESS") => CheckStatus::Success,
            Some("FAILURE" | "ERROR") => CheckStatus::Failure,
            _ => CheckStatus::Pending,
        };
        return CheckRun {
            name: text(&c["context"]),
            status,
            url: opt_text(&c["targetUrl"]),
            started_at: None,
            completed_at: None,
        };
    }
    let status = match (c["status"].as_str(), c["conclusion"].as_str()) {
        (Some("COMPLETED"), Some("SUCCESS")) => CheckStatus::Success,
        (
            Some("COMPLETED"),
            Some("FAILURE" | "TIMED_OUT" | "ACTION_REQUIRED" | "STARTUP_FAILURE"),
        ) => CheckStatus::Failure,
        (Some("COMPLETED"), _) => CheckStatus::Neutral,
        _ => CheckStatus::Pending,
    };
    CheckRun {
        name: text(&c["name"]),
        status,
        url: opt_text(&c["detailsUrl"]),
        started_at: opt_text(&c["startedAt"]),
        completed_at: opt_text(&c["completedAt"]),
    }
}

fn review_state(v: &Value) -> Option<ReviewState> {
    match v.as_str()? {
        "APPROVED" => Some(ReviewState::Approved),
        "CHANGES_REQUESTED" => Some(ReviewState::ChangesRequested),
        "COMMENTED" => Some(ReviewState::Commented),
        "DISMISSED" => Some(ReviewState::Dismissed),
        // PENDING: the viewer's own unsubmitted review.
        _ => None,
    }
}

fn thread(t: &Value) -> ReviewThread {
    let line = |v: &Value| v.as_u64().map(|n| n as u32);
    ReviewThread {
        id: text(&t["id"]),
        can_resolve: t["viewerCanResolve"].as_bool().unwrap_or(false),
        can_unresolve: t["viewerCanUnresolve"].as_bool().unwrap_or(false),
        path: text(&t["path"]),
        line: line(&t["line"]),
        original_line: line(&t["originalLine"]),
        side: if t["diffSide"] == "LEFT" {
            DiffSide::Left
        } else {
            DiffSide::Right
        },
        start_line: line(&t["startLine"]),
        start_side: match t["startDiffSide"].as_str() {
            Some("LEFT") => Some(DiffSide::Left),
            Some("RIGHT") => Some(DiffSide::Right),
            _ => None,
        },
        outdated: t["isOutdated"].as_bool().unwrap_or(false),
        resolved: t["isResolved"].as_bool().unwrap_or(false),
        comments: nodes(&t["comments"])
            .map(|c| ThreadComment {
                id: c["databaseId"].as_u64().unwrap_or_default(),
                author: login(&c["author"]),
                author_id: c["author"]["databaseId"].as_u64(),
                body: text(&c["body"]),
                at: text(&c["createdAt"]),
            })
            .collect(),
    }
}

fn parse_detail(data: &Value) -> Result<PrDetail, GithubError> {
    let repo = &data["repository"];
    let p = &repo["pullRequest"];
    if !p.is_object() {
        return Err(GithubError::Decode("pull request not found".into()));
    }
    let mut summary = summary(p);
    summary.checks = checks_state(last_rollup(&p["lastCommit"]));
    let commits = nodes(&p["commits"])
        .map(|n| {
            let c = &n["commit"];
            let author = c["author"]["user"]["login"]
                .as_str()
                .map(str::to_string)
                .unwrap_or_else(|| text(&c["author"]["name"]));
            PrCommit {
                oid: text(&c["oid"]),
                short_oid: text(&c["abbreviatedOid"]),
                headline: text(&c["messageHeadline"]),
                author,
                date: text(&c["committedDate"]),
            }
        })
        .collect();
    let check_runs = nodes(&last_rollup(&p["lastCommit"])["contexts"])
        .map(check_run)
        .collect();
    let mut timeline: Vec<TimelineItem> = nodes(&p["comments"])
        .map(|c| TimelineItem::Comment {
            author: login(&c["author"]),
            body: text(&c["body"]),
            at: text(&c["createdAt"]),
        })
        .collect();
    timeline.extend(nodes(&p["reviews"]).filter_map(|r| {
        Some(TimelineItem::Review {
            state: review_state(&r["state"])?,
            author: login(&r["author"]),
            body: text(&r["body"]),
            at: text(&r["submittedAt"]),
        })
    }));
    timeline.sort_by(|a, b| a.at().cmp(b.at()));
    let mut allowed_methods = Vec::new();
    for (field, m) in [
        ("mergeCommitAllowed", MergeMethod::Merge),
        ("squashMergeAllowed", MergeMethod::Squash),
        ("rebaseMergeAllowed", MergeMethod::Rebase),
    ] {
        if repo[field].as_bool() == Some(true) {
            allowed_methods.push(m);
        }
    }
    let head_repo = p["headRepository"]["owner"]["login"]
        .as_str()
        .map(|o| (o.to_string(), text(&p["headRepository"]["name"])));
    Ok(PrDetail {
        summary,
        body: text(&p["body"]),
        head_sha: text(&p["headRefOid"]),
        head_repo,
        cross_repository: p["isCrossRepository"].as_bool().unwrap_or(false),
        commits,
        commit_count: count(&p["commits"]["totalCount"]),
        comments_total: count(&p["comments"]["totalCount"]),
        reviews_total: count(&p["reviews"]["totalCount"]),
        threads_total: count(&p["reviewThreads"]["totalCount"]),
        check_runs,
        timeline,
        threads: nodes(&p["reviewThreads"]).map(thread).collect(),
        mergeable: match p["mergeable"].as_str() {
            Some("MERGEABLE") => Mergeable::Mergeable,
            Some("CONFLICTING") => Mergeable::Conflicting,
            _ => Mergeable::Unknown,
        },
        merge_state: p["mergeStateStatus"]
            .as_str()
            .unwrap_or("UNKNOWN")
            .to_string(),
        allowed_methods,
        viewer: login(&data["viewer"]),
        viewer_is_author: p["viewerDidAuthor"].as_bool().unwrap_or(false),
        viewer_can_write: matches!(
            repo["viewerPermission"].as_str(),
            Some("ADMIN" | "MAINTAIN" | "WRITE")
        ),
        repo_labels: labels(&repo["labels"]),
        id: text(&p["id"]),
        viewer_can_update: p["viewerCanUpdate"].as_bool().unwrap_or(false),
        reviewers: reviewers(p),
        assignees: nodes(&p["assignees"])
            .filter_map(|a| a["login"].as_str().map(str::to_string))
            .collect(),
    })
}

fn reviewers(p: &Value) -> Vec<Reviewer> {
    let mut out: Vec<Reviewer> = nodes(&p["reviewRequests"])
        .filter_map(|r| r["requestedReviewer"]["login"].as_str())
        .map(|login| Reviewer {
            login: login.to_string(),
            state: None,
        })
        .collect();
    for r in nodes(&p["latestReviews"]) {
        let login = login(&r["author"]);
        if out.iter().any(|o| o.login.eq_ignore_ascii_case(&login)) {
            continue;
        }
        out.push(Reviewer {
            login,
            state: review_state(&r["state"]),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors_parse_and_fall_back_to_gray() {
        assert_eq!(parse_color("d73a4a"), [0xd7, 0x3a, 0x4a]);
        assert_eq!(parse_color("#0075CA"), [0x00, 0x75, 0xca]);
        assert_eq!(parse_color("zz"), [0xC0, 0xC0, 0xC0]);
        assert_eq!(parse_color(""), [0xC0, 0xC0, 0xC0]);
    }
}
