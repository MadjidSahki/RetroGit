//! Interactive rebase: the list of commits, what to do with each, and `git rebase -i` fed
//! with a prepared list (no editor shown to the user).

use std::path::Path;

use crate::{GitError, OpOutcome, Operation, Refusal, Repo};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TodoAction {
    Pick,
    /// Keep the commit with a new message.
    Reword(String),
    /// Meld into the commit above; `Some(message)` replaces the combined message.
    Squash(Option<String>),
    /// Meld into the commit above, keeping its message.
    Fixup,
    Drop,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TodoItem {
    pub action: TodoAction,
    /// Full commit id.
    pub id: String,
    pub summary: String,
    /// The whole message (subject and body), to start a reword from.
    pub message: String,
}

/// Why an interactive rebase list cannot be run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum TodoError {
    /// A squash or fixup has no kept commit above it to meld into.
    #[error("a squash or fixup needs a kept commit above it")]
    NoKeptAbove,
    /// Every commit is dropped.
    #[error("keep at least one commit")]
    KeepOne,
}

/// Why the list cannot be run: something to meld into is needed above each squash or
/// fixup, and at least one commit must be kept.
pub fn validate_todo(items: &[TodoItem]) -> Result<(), TodoError> {
    let mut kept = false;
    for i in items {
        match i.action {
            TodoAction::Squash(_) | TodoAction::Fixup if !kept => {
                return Err(TodoError::NoKeptAbove);
            }
            TodoAction::Drop => {}
            _ => kept = true,
        }
    }
    if kept {
        Ok(())
    } else {
        Err(TodoError::KeepOne)
    }
}

/// Lines of `git-rebase-todo`, and the messages they read (numbered files in `dir`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TodoText {
    pub lines: Vec<String>,
    pub messages: Vec<String>,
}

fn quote(path: &Path) -> String {
    // Git runs `exec` lines with its sh, also on Windows: forward slashes, single quotes.
    format!(
        "'{}'",
        path.display()
            .to_string()
            .replace('\\', "/")
            .replace('\'', "'\\''")
    )
}

/// The todo list for `items`: a new message is set by an `exec git commit --amend` right
/// after its commit, or after the last squash / fixup of its group. Dropped commits are
/// written as `drop` (`rebase.missingCommitsCheck` may refuse missing lines).
pub fn todo_text(items: &[TodoItem], dir: &Path) -> TodoText {
    let mut lines = Vec::new();
    let mut messages: Vec<String> = Vec::new();
    // Message of the squash group being built, written when the group ends.
    let mut pending: Option<String> = None;
    let mut amend = |lines: &mut Vec<String>, msg: String| {
        let file = dir.join(format!("{}.txt", messages.len()));
        messages.push(msg);
        lines.push(format!(
            "exec git commit --amend --allow-empty -F {}",
            quote(&file)
        ));
    };
    for i in items {
        let melds = matches!(i.action, TodoAction::Squash(_) | TodoAction::Fixup);
        if !melds && let Some(m) = pending.take() {
            amend(&mut lines, m);
        }
        let verb = match &i.action {
            TodoAction::Pick | TodoAction::Reword(_) => "pick",
            TodoAction::Squash(_) => "squash",
            TodoAction::Fixup => "fixup",
            TodoAction::Drop => "drop",
        };
        lines.push(format!("{verb} {} {}", i.id, i.summary));
        match &i.action {
            TodoAction::Reword(m) => amend(&mut lines, m.clone()),
            TodoAction::Squash(Some(m)) => pending = Some(m.clone()),
            _ => {}
        }
    }
    if let Some(m) = pending {
        amend(&mut lines, m);
    }
    TodoText { lines, messages }
}

impl Repo {
    /// Commits from `base` (excluded) to HEAD, oldest first, all picked. Merges in the
    /// range are not supported.
    pub fn rebase_list(&self, base: &str) -> Result<Vec<TodoItem>, GitError> {
        let range = format!("{base}..HEAD");
        // One record per commit (\x1e), fields separated by \0; the body may span lines.
        let out = self.git_ok(&[
            "log",
            "--reverse",
            "--format=%H%x00%P%x00%s%x00%B%x1e",
            &range,
        ])?;
        let mut items = Vec::new();
        for record in out
            .stdout
            .split('\x1e')
            .map(str::trim)
            .filter(|r| !r.is_empty())
        {
            let mut parts = record.split('\0');
            let (id, parents, summary, message) = (
                parts.next().unwrap_or_default(),
                parts.next().unwrap_or_default(),
                parts.next().unwrap_or_default(),
                parts.next().unwrap_or_default(),
            );
            if parents.split(' ').count() > 1 {
                return Err(GitError::Refused(Refusal::MergesInRange));
            }
            items.push(TodoItem {
                action: TodoAction::Pick,
                id: id.to_string(),
                summary: summary.to_string(),
                message: message.trim_end().to_string(),
            });
        }
        Ok(items)
    }

    /// Run `git rebase -i base` with `items` as the list. Messages are kept in
    /// `.git/retrogit-rebase/` (read again if the rebase stops and is continued).
    pub fn interactive_rebase(
        &self,
        base: &str,
        items: &[TodoItem],
    ) -> Result<OpOutcome, GitError> {
        validate_todo(items).map_err(|e| GitError::Refused(Refusal::Todo(e)))?;
        if self.operation_in_progress().is_some() {
            // Its message files are still needed: never touch them.
            return Err(GitError::Refused(Refusal::OperationInProgress));
        }
        let dir = self.git().path().join("retrogit-rebase");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).map_err(|e| GitError::Other(e.to_string()))?;
        let text = todo_text(items, &dir);
        for (n, m) in text.messages.iter().enumerate() {
            std::fs::write(dir.join(format!("{n}.txt")), m)
                .map_err(|e| GitError::Other(e.to_string()))?;
        }
        let todo = dir.join("todo");
        std::fs::write(&todo, text.lines.join("\n") + "\n")
            .map_err(|e| GitError::Other(e.to_string()))?;
        // The environment variable wins over any user setting (config or environment).
        let editor = format!("cp {}", quote(&todo));
        let out = self.run_git_env(&["rebase", "-i", base], &[("GIT_SEQUENCE_EDITOR", &editor)])?;
        self.outcome(out, Operation::Rebase)
    }
}
