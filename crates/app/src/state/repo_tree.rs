//! The Repositories side panel as a tree: each recent repository, and under the open one
//! its branches in virtual folders (`feat/login` sits in a `feat` folder).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use gitcore::Branch;

use crate::config::RecentRepo;

/// Prefix of the folder keys of remote branches, so `origin/x` never shares a key with a
/// local folder named `origin`.
const REMOTE_KEY: &str = "remote:";

/// What the user unfolded. Only the open repository can be expanded.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RepoTree {
    /// The open repository shows its branches.
    pub expanded: bool,
    /// Expand this repository once it is open (its arrow was clicked while closed).
    pub expand_after_open: Option<PathBuf>,
    /// Open folder keys: `feat/deep` for local folders, `remote:origin/feat` for remotes.
    pub open: BTreeSet<String>,
    /// The branch clicked last (highlighted).
    pub selected: Option<String>,
    /// Open the current branch's folders once the branches are loaded.
    pub reveal_pending: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowKind {
    Repo {
        path: PathBuf,
        current: bool,
        expanded: bool,
    },
    Folder {
        key: String,
        /// `feat/deep`, or `origin/feat` for remote branches.
        path: String,
        remote: bool,
        open: bool,
    },
    Branch {
        /// Full name, as `SwitchBranch` wants it (`feat/login`, `origin/x`).
        name: String,
        current: bool,
        remote: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub depth: usize,
    pub label: String,
    pub kind: RowKind,
}

impl RepoTree {
    /// Open every folder leading to the current local branch.
    pub fn reveal_current(&mut self, branches: &[Branch]) {
        let Some(head) = branches.iter().find(|b| b.is_head && !b.remote) else {
            return;
        };
        let parts: Vec<&str> = head.name.split('/').collect();
        for end in 1..parts.len() {
            self.open.insert(parts[..end].join("/"));
        }
    }

    pub fn toggle_folder(&mut self, key: &str) {
        if !self.open.remove(key) {
            self.open.insert(key.to_string());
        }
    }
}

/// Remote branches worth offering (no local branch with the same short name).
pub fn remote_only(branches: &[Branch]) -> Vec<&Branch> {
    branches
        .iter()
        .filter(|b| b.remote)
        .filter(|r| {
            let short = r.name.split_once('/').map(|x| x.1).unwrap_or(&r.name);
            !branches.iter().any(|l| !l.remote && l.name == short)
        })
        .collect()
}

/// The rows of the side panel, top to bottom.
pub fn sidebar_rows(
    recents: &[RecentRepo],
    current: Option<&Path>,
    tree: &RepoTree,
    branches: &[Branch],
) -> Vec<Row> {
    let mut rows = Vec::new();
    for r in recents {
        let is_current = current == Some(r.path.as_path());
        let expanded = is_current && tree.expanded;
        rows.push(Row {
            depth: 0,
            label: r.name.clone(),
            kind: RowKind::Repo {
                path: r.path.clone(),
                current: is_current,
                expanded,
            },
        });
        if expanded {
            let locals: Vec<&Branch> = branches.iter().filter(|b| !b.remote).collect();
            push_level(&mut rows, &locals, "", "", 1, tree);
            push_level(&mut rows, &remote_only(branches), REMOTE_KEY, "", 1, tree);
        }
    }
    rows
}

/// Folders, then branches, of the branches under `prefix` (a `/`-ended path or "").
fn push_level(
    rows: &mut Vec<Row>,
    branches: &[&Branch],
    key_ns: &str,
    prefix: &str,
    depth: usize,
    tree: &RepoTree,
) {
    let mut folders: BTreeMap<(String, String), Vec<&Branch>> = BTreeMap::new();
    let mut leaves: Vec<(&str, &Branch)> = Vec::new();
    for b in branches {
        let Some(rest) = b.name.strip_prefix(prefix) else {
            continue;
        };
        match rest.split_once('/') {
            Some((folder, _)) => folders
                .entry((folder.to_lowercase(), folder.to_string()))
                .or_default()
                .push(b),
            None => leaves.push((rest, b)),
        }
    }
    for ((_, folder), inside) in folders {
        let path = format!("{prefix}{folder}");
        let key = format!("{key_ns}{path}");
        let open = tree.open.contains(&key);
        rows.push(Row {
            depth,
            label: folder,
            kind: RowKind::Folder {
                key,
                path: path.clone(),
                remote: !key_ns.is_empty(),
                open,
            },
        });
        if open {
            push_level(rows, &inside, key_ns, &format!("{path}/"), depth + 1, tree);
        }
    }
    leaves.sort_by(|a, b| {
        a.0.to_lowercase()
            .cmp(&b.0.to_lowercase())
            .then(a.0.cmp(b.0))
    });
    for (label, b) in leaves {
        rows.push(Row {
            depth,
            label: label.to_string(),
            kind: RowKind::Branch {
                name: b.name.clone(),
                current: b.is_head && !b.remote,
                remote: b.remote,
            },
        });
    }
}
