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
    pub body: String,
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
    /// Open a pull request and add its labels. `Rejected` with GitHub's reason on 422
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
        let number = resp.body_mut().read_json::<Created>()?.number;
        if !pull.labels.is_empty() {
            self.set_labels(token, owner, repo, number, &pull.labels)?;
        }
        Ok(number)
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
        let comments: Vec<Value> = review
            .comments
            .iter()
            .map(|c| {
                json!({
                    "path": c.path,
                    "line": c.line,
                    "side": match c.side { DiffSide::Left => "LEFT", DiffSide::Right => "RIGHT" },
                    "body": c.body,
                })
            })
            .collect();
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

    /// Replace the labels of pull request `number` with `labels`.
    pub fn set_labels(
        &self,
        token: &str,
        owner: &str,
        repo: &str,
        number: u64,
        labels: &[String],
    ) -> Result<(), GithubError> {
        self.api_send(
            "PUT",
            &format!("/repos/{owner}/{repo}/issues/{number}/labels"),
            token,
            Some(&json!({ "labels": labels })),
        )
        .map(|_| ())
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
