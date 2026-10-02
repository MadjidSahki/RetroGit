//! Interactive rebase: the list of commits, what to do with each, and `git rebase -i` fed
//! with a prepared list (no editor shown to the user).

use std::path::Path;

use crate::{GitError, OpOutcome, Operation, Repo};

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
}

/// Why the list cannot be run: something to meld into is needed above each squash or
/// fixup, and at least one commit must be kept.
pub fn validate_todo(items: &[TodoItem]) -> Result<(), &'static str> {
    let mut kept = false;
    for i in items {
        match i.action {
            TodoAction::Squash(_) | TodoAction::Fixup if !kept => {
                return Err("A squash or fixup needs a kept commit above it.");
            }
            TodoAction::Drop => {}
            _ => kept = true,
        }
    }
    if kept {
        Ok(())
    } else {
        Err("Keep at least one commit.")
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
/// after its commit (or after the squash that ends a group).
pub fn todo_text(items: &[TodoItem], dir: &Path) -> TodoText {
    let mut lines = Vec::new();
    let mut messages: Vec<String> = Vec::new();
    let mut amend = |lines: &mut Vec<String>, msg: &str| {
        let file = dir.join(format!("{}.txt", messages.len()));
        messages.push(msg.to_string());
        lines.push(format!(
            "exec git commit --amend --allow-empty -F {}",
            quote(&file)
        ));
    };
    for i in items {
        let (verb, msg) = match &i.action {
            TodoAction::Pick => ("pick", None),
            TodoAction::Reword(m) => ("pick", Some(m.as_str())),
            TodoAction::Squash(m) => ("squash", m.as_deref()),
            TodoAction::Fixup => ("fixup", None),
            TodoAction::Drop => continue,
        };
        lines.push(format!("{verb} {} {}", i.id, i.summary));
        if let Some(m) = msg {
            amend(&mut lines, m);
        }
    }
    TodoText { lines, messages }
}

impl Repo {
    /// Commits from `base` (excluded) to HEAD, oldest first, all picked. Merges in the
    /// range are not supported.
    pub fn rebase_list(&self, base: &str) -> Result<Vec<TodoItem>, GitError> {
        let range = format!("{base}..HEAD");
        let out = self.git_ok(&["log", "--reverse", "--format=%H%x00%P%x00%s", &range])?;
        let mut items = Vec::new();
        for line in out.stdout.lines().filter(|l| !l.is_empty()) {
            let mut parts = line.split('\0');
            let (id, parents, summary) = (
                parts.next().unwrap_or_default(),
                parts.next().unwrap_or_default(),
                parts.next().unwrap_or_default(),
            );
            if parents.split(' ').count() > 1 {
                return Err(GitError::Unsupported(
                    "the range contains merge commits: interactive rebase of merges is not supported"
                        .into(),
                ));
            }
            items.push(TodoItem {
                action: TodoAction::Pick,
                id: id.to_string(),
                summary: summary.to_string(),
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
        validate_todo(items).map_err(|e| GitError::Unsupported(e.to_string()))?;
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
        let editor = format!("sequence.editor=cp {}", quote(&todo));
        let out = self.run_git(&["-c", &editor, "rebase", "-i", base])?;
        self.outcome(out, Operation::Rebase)
    }
}
