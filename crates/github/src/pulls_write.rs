//! Changing pull requests through the REST API.

use serde::Deserialize;
use serde_json::{Value, json};

use crate::client::{Client, header};
use crate::pulls::{DiffSide, Label, MergeMethod, labels_from_rest};
use crate::{GithubError, next_link};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewPull {
    pub title: String,
    pub body: String,
    /// Source branch (in the same repository).
    pub head: String,
    /// Target branch.
    pub base: String,
    pub draft: bool,
    pub labels: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewEvent {
    Comment,
    Approve,
    RequestChanges,
}

impl ReviewEvent {
    fn api_name(self) -> &'static str {
        match self {
            ReviewEvent::Comment => "COMMENT",
            ReviewEvent::Approve => "APPROVE",
            ReviewEvent::RequestChanges => "REQUEST_CHANGES",
        }
    }
}

/// A comment on one line of the diff, sent with a review.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineComment {
    pub path: String,
    /// Line number in the file on `side` (old file for `Left`, new file for `Right`).
    pub line: u32,
    pub side: DiffSide,
    /// First line and side of a comment on several lines (`line` is the last one).
    pub start: Option<(u32, DiffSide)>,
    pub body: String,
}

fn side_name(side: DiffSide) -> &'static str {
    match side {
        DiffSide::Left => "LEFT",
        DiffSide::Right => "RIGHT",
    }
}

impl LineComment {
    /// Fields of the comment in GitHub's REST payloads.
    fn to_json(&self) -> Value {
        let mut v = json!({
            "path": self.path,
            "line": self.line,
            "side": side_name(self.side),
            "body": self.body,
        });
        if let Some((line, side)) = self.start.filter(|(l, _)| *l < self.line) {
            v["start_line"] = json!(line);
            v["start_side"] = json!(side_name(side));
        }
        v
    }
}

/// A review to submit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Review {
    /// Commit the reviewer looked at (comments are anchored to it).
    pub commit_id: String,
    pub event: ReviewEvent,
    pub body: String,
    pub comments: Vec<LineComment>,
}

/// How to merge, and the head commit the user saw (GitHub refuses if it moved).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Merge {
    pub method: MergeMethod,
    pub title: String,
    pub message: String,
    pub sha: String,
}

/// Default branch and labels of a repository (for the "New pull request" dialog).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoMeta {
    pub default_branch: String,
    pub labels: Vec<Label>,
}

#[derive(Deserialize)]
struct Created {
    number: u64,
}

/// Percent-encode a path segment (branch or label names: spaces, `#`, `%`...). `/` is
/// kept when `keep_slash` (Git ref paths).
pub fn encode_segment(s: &str, keep_slash: bool) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        let plain = b.is_ascii_alphanumeric()
            || matches!(b, b'-' | b'.' | b'_' | b'~')
            || (keep_slash && b == b'/');
        if plain {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

impl Client {
    /// Open a pull request, without its labels (`set_labels` adds them, so a failure there
    /// keeps the number). `Rejected` with GitHub's reason on 422
    /// (e.g. "A pull request already exists for o:branch.").
    pub fn create_pull(
        &self,
        token: &str,
        owner: &str,
        repo: &str,
        pull: &NewPull,
    ) -> Result<u64, GithubError> {
        let body = json!({
            "title": pull.title,
            "body": pull.body,
            "head": pull.head,
            "base": pull.base,
            "draft": pull.draft,
        });
        let mut resp = self.api_send(
            "POST",
            &format!("/repos/{owner}/{repo}/pulls"),
            token,
            Some(&body),
        )?;
        Ok(resp.body_mut().read_json::<Created>()?.number)
    }

    /// Number of the open pull request whose head is `head_owner:branch`, if any.
    pub fn find_open_pull(
        &self,
        token: &str,
        owner: &str,
        repo: &str,
        head_owner: &str,
        branch: &str,
    ) -> Result<Option<u64>, GithubError> {
        let url = format!(
            "{}/repos/{owner}/{repo}/pulls?state=open&head={}:{}",
            self.api_base,
            encode_segment(head_owner, false),
            encode_segment(branch, true)
        );
        let mut resp = self.api_get(&url, token)?;
        let list: Vec<Created> = resp.body_mut().read_json()?;
        Ok(list.first().map(|p| p.number))
    }

    /// Submit a review on commit `commit_id` (the one displayed), with its line comments.
    pub fn submit_review(
        &self,
        token: &str,
        owner: &str,
        repo: &str,
        number: u64,
        review: &Review,
    ) -> Result<(), GithubError> {
        let comments: Vec<Value> = review.comments.iter().map(LineComment::to_json).collect();
        let mut payload = json!({
            "commit_id": review.commit_id,
            "event": review.event.api_name(),
            "comments": comments,
        });
        if !review.body.trim().is_empty() {
            payload["body"] = json!(review.body);
        }
        self.api_send(
            "POST",
            &format!("/repos/{owner}/{repo}/pulls/{number}/reviews"),
            token,
            Some(&payload),
        )
        .map(|_| ())
    }

    /// Answer in an existing line-comment thread (`comment_id`: any comment of the thread).
    pub fn reply_to_thread(
        &self,
        token: &str,
        owner: &str,
        repo: &str,
        number: u64,
        comment_id: u64,
        body: &str,
    ) -> Result<(), GithubError> {
        self.api_send(
            "POST",
            &format!("/repos/{owner}/{repo}/pulls/{number}/comments/{comment_id}/replies"),
            token,
            Some(&json!({ "body": body })),
        )
        .map(|_| ())
    }

    /// A line comment posted on its own (not part of a review), on commit `commit_id`.
    pub fn add_line_comment(
        &self,
        token: &str,
        owner: &str,
        repo: &str,
        number: u64,
        commit_id: &str,
        comment: &LineComment,
    ) -> Result<(), GithubError> {
        let mut payload = comment.to_json();
        payload["commit_id"] = json!(commit_id);
        self.api_send(
            "POST",
            &format!("/repos/{owner}/{repo}/pulls/{number}/comments"),
            token,
            Some(&payload),
        )
        .map(|_| ())
    }

    /// New title and description.
    pub fn update_pull(
        &self,
        token: &str,
        owner: &str,
        repo: &str,
        number: u64,
        title: &str,
        body: &str,
    ) -> Result<(), GithubError> {
        self.api_send(
            "PATCH",
            &format!("/repos/{owner}/{repo}/pulls/{number}"),
            token,
            Some(&json!({ "title": title, "body": body })),
        )
        .map(|_| ())
    }

    /// Request reviews from `add`, withdraw the requests of `remove`.
    pub fn set_reviewers(
        &self,
        token: &str,
        owner: &str,
        repo: &str,
        number: u64,
        add: &[String],
        remove: &[String],
    ) -> Result<(), GithubError> {
        let path = format!("/repos/{owner}/{repo}/pulls/{number}/requested_reviewers");
        if !add.is_empty() {
            self.api_send("POST", &path, token, Some(&json!({ "reviewers": add })))?;
        }
        if !remove.is_empty() {
            self.api_send(
                "DELETE",
                &path,
                token,
                Some(&json!({ "reviewers": remove })),
            )?;
        }
        Ok(())
    }

    pub fn set_assignees(
        &self,
        token: &str,
        owner: &str,
        repo: &str,
        number: u64,
        add: &[String],
        remove: &[String],
    ) -> Result<(), GithubError> {
        let path = format!("/repos/{owner}/{repo}/issues/{number}/assignees");
        if !add.is_empty() {
            self.api_send("POST", &path, token, Some(&json!({ "assignees": add })))?;
        }
        if !remove.is_empty() {
            self.api_send(
                "DELETE",
                &path,
                token,
                Some(&json!({ "assignees": remove })),
            )?;
        }
        Ok(())
    }

    /// Turn the pull request (GraphQL id `pull_id`) into a draft, or mark it ready.
    pub fn set_draft(&self, token: &str, pull_id: &str, draft: bool) -> Result<(), GithubError> {
        let mutation = if draft {
            "mutation($id: ID!) { convertPullRequestToDraft(input: { pullRequestId: $id }) { pullRequest { isDraft } } }"
        } else {
            "mutation($id: ID!) { markPullRequestReadyForReview(input: { pullRequestId: $id }) { pullRequest { isDraft } } }"
        };
        self.graphql(token, mutation, json!({ "id": pull_id }))
            .map(|_| ())
    }

    /// People who can be asked for a review or assigned (matching `query`, first 100).
    pub fn assignable_users(
        &self,
        token: &str,
        owner: &str,
        repo: &str,
        query: &str,
    ) -> Result<Vec<String>, GithubError> {
        let data = self.graphql(
            token,
            include_str!("queries/assignable.graphql"),
            json!({ "owner": owner, "name": repo, "q": query }),
        )?;
        Ok(crate::pulls::nodes(&data["repository"]["assignableUsers"])
            .filter_map(|n| n["login"].as_str().map(str::to_string))
            .collect())
    }

    /// Mark a line-comment thread resolved (`true`) or not (GraphQL only).
    pub fn set_thread_resolved(
        &self,
        token: &str,
        thread_id: &str,
        resolved: bool,
    ) -> Result<(), GithubError> {
        let mutation = if resolved {
            "mutation($id: ID!) { resolveReviewThread(input: { threadId: $id }) { thread { isResolved } } }"
        } else {
            "mutation($id: ID!) { unresolveReviewThread(input: { threadId: $id }) { thread { isResolved } } }"
        };
        self.graphql(token, mutation, json!({ "id": thread_id }))
            .map(|_| ())
    }

    /// Comment in the conversation.
    pub fn add_issue_comment(
        &self,
        token: &str,
        owner: &str,
        repo: &str,
        number: u64,
        body: &str,
    ) -> Result<(), GithubError> {
        self.api_send(
            "POST",
            &format!("/repos/{owner}/{repo}/issues/{number}/comments"),
            token,
            Some(&json!({ "body": body })),
        )
        .map(|_| ())
    }

    /// Merge, only if the head is still `sha`. GitHub answers 405 (not mergeable) or 409
    /// (head moved) as `Rejected { status, message }`.
    pub fn merge_pull(
        &self,
        token: &str,
        owner: &str,
        repo: &str,
        number: u64,
        merge: &Merge,
    ) -> Result<(), GithubError> {
        let mut payload = json!({ "merge_method": merge.method.api_name(), "sha": merge.sha });
        // Rebase merges have no merge commit: no title or message to send.
        if merge.method != MergeMethod::Rebase {
            payload["commit_title"] = json!(merge.title);
            payload["commit_message"] = json!(merge.message);
        }
        self.api_send(
            "PUT",
            &format!("/repos/{owner}/{repo}/pulls/{number}/merge"),
            token,
            Some(&payload),
        )
        .map(|_| ())
    }

    /// Delete a branch of `owner/repo` (after merging).
    pub fn delete_branch(
        &self,
        token: &str,
        owner: &str,
        repo: &str,
        branch: &str,
    ) -> Result<(), GithubError> {
        self.api_send(
            "DELETE",
            &format!(
                "/repos/{owner}/{repo}/git/refs/heads/{}",
                encode_segment(branch, true)
            ),
            token,
            None,
        )
        .map(|_| ())
    }

    /// Change the labels of pull request `number` from `old` to `new`: adds what `new`
    /// has, removes what it dropped, and leaves alone labels neither list knows (set by
    /// someone else meanwhile, or not shown).
    pub fn set_labels(
        &self,
        token: &str,
        owner: &str,
        repo: &str,
        number: u64,
        old: &[String],
        new: &[String],
    ) -> Result<(), GithubError> {
        let (add, remove) = diff_lists(old, new);
        if !add.is_empty() {
            self.api_send(
                "POST",
                &format!("/repos/{owner}/{repo}/issues/{number}/labels"),
                token,
                Some(&json!({ "labels": add })),
            )?;
        }
        for name in remove {
            self.api_send(
                "DELETE",
                &format!(
                    "/repos/{owner}/{repo}/issues/{number}/labels/{}",
                    encode_segment(&name, false)
                ),
                token,
                None,
            )?;
        }
        Ok(())
    }

    /// Default branch and every label of the repository.
    pub fn repo_meta(&self, token: &str, owner: &str, repo: &str) -> Result<RepoMeta, GithubError> {
        let mut resp = self.api_get(&format!("{}/repos/{owner}/{repo}", self.api_base), token)?;
        let info: Value = resp.body_mut().read_json()?;
        let mut url = format!("{}/repos/{owner}/{repo}/labels?per_page=100", self.api_base);
        let mut labels = Vec::new();
        loop {
            let mut resp = self.api_get(&url, token)?;
            let next = header(&resp, "link").and_then(next_link);
            let page: Value = resp.body_mut().read_json()?;
            labels.extend(labels_from_rest(&page));
            match next {
                Some(n) => url = n,
                None => break,
            }
        }
        Ok(RepoMeta {
            default_branch: info["default_branch"]
                .as_str()
                .unwrap_or("main")
                .to_string(),
            labels,
        })
    }
}

/// The code of each ```suggestion block of a comment (lines end with `\n`), following
/// Markdown fences: ``` or ~~~, possibly longer; only a fence of the same character, at
/// least as long and with nothing after it closes the block; an unclosed block runs to
/// the end of the comment.
pub fn suggestions(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    // (fence character, fence length, code so far)
    let mut open: Option<(char, usize, String)> = None;
    for line in body.lines() {
        let trimmed = line.trim();
        let fence = |t: &str| -> Option<(char, usize)> {
            let c = t.chars().next().filter(|c| *c == '`' || *c == '~')?;
            let n = t.chars().take_while(|x| *x == c).count();
            (n >= 3).then_some((c, n))
        };
        match open.as_mut() {
            Some((c, n, code)) => {
                let closes = fence(trimmed).is_some_and(|(fc, fnn)| {
                    fc == *c && fnn >= *n && trimmed.chars().all(|x| x == fc)
                });
                if closes {
                    out.push(std::mem::take(code));
                    open = None;
                } else {
                    code.push_str(line.trim_end_matches('\r'));
                    code.push('\n');
                }
            }
            None => {
                if let Some((c, n)) = fence(trimmed)
                    && trimmed[n..].trim() == "suggestion"
                {
                    open = Some((c, n, String::new()));
                }
            }
        }
    }
    if let Some((_, _, code)) = open {
        out.push(code);
    }
    out
}

/// A comment body proposing to replace `lines` (prefilled with them, ready to edit).
pub fn suggestion_block(lines: &[String]) -> String {
    let mut out = String::from("```suggestion\n");
    for l in lines {
        out.push_str(l.trim_end_matches(['\n', '\r']));
        out.push('\n');
    }
    out.push_str("```\n");
    out
}

/// `(to add, to remove)` to go from `old` to `new` (logins, case-insensitive).
pub fn diff_lists(old: &[String], new: &[String]) -> (Vec<String>, Vec<String>) {
    let has = |list: &[String], x: &str| list.iter().any(|l| l.eq_ignore_ascii_case(x));
    (
        new.iter().filter(|n| !has(old, n)).cloned().collect(),
        old.iter().filter(|o| !has(new, o)).cloned().collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::encode_segment;

    #[test]
    fn segments_are_encoded() {
        assert_eq!(
            encode_segment("good first issue", false),
            "good%20first%20issue"
        );
        assert_eq!(encode_segment("feat/x#1", true), "feat/x%231");
        assert_eq!(encode_segment("a/b", false), "a%2Fb");
        assert_eq!(encode_segment("été", false), "%C3%A9t%C3%A9");
    }
}
