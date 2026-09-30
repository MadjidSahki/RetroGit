//! Toolbar and menu entries for branches and fetch / pull / push.

use gitcore::{Branch, PullMode, PushMode};
use win95::{Button95, combo_box};

use super::Ctx;
use crate::protocol::Command;
use crate::state::{AppState, PendingDialog};
use crate::strings as s;

/// What the sync buttons can do right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncButtons {
    pub fetch: bool,
    pub pull: bool,
    pub push: bool,
    /// The current branch has no upstream: Push becomes "Publish".
    pub publish: bool,
    pub pull_label: String,
    pub push_label: String,
}

/// Pure: button states from the app state.
pub fn sync_buttons(state: &AppState) -> SyncButtons {
    let idle = state.current.is_some() && state.sync.running.is_none() && state.operation.is_none();
    let has_origin = state
        .current
        .as_ref()
        .is_some_and(|c| c.origin_url.is_some());
    let head = current_branch(&state.branches);
    let publish = head.is_some_and(|b| b.upstream.is_none());
    let (ahead, behind) = head.map(|b| (b.ahead, b.behind)).unwrap_or((0, 0));
    SyncButtons {
        fetch: idle && has_origin,
        pull: idle && has_origin && head.is_some_and(|b| b.upstream.is_some()),
        push: idle && has_origin && head.is_some() && (publish || ahead > 0),
        publish,
        pull_label: if behind > 0 {
            format!("{} ({behind})", s::PULL)
        } else {
            s::PULL.to_string()
        },
        push_label: if publish {
            s::PUBLISH.to_string()
        } else if ahead > 0 {
            format!("{} ({ahead})", s::PUSH)
        } else {
            s::PUSH.to_string()
        },
    }
}

pub fn current_branch(branches: &[Branch]) -> Option<&Branch> {
    branches.iter().find(|b| b.is_head && !b.remote)
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

pub fn toolbar(ui: &mut egui::Ui, cx: &mut Ctx<'_>) {
    let b = sync_buttons(cx.state);
    let size = egui::vec2(60.0, 22.0);
    if ui
        .add(Button95::new(s::FETCH).min_size(size).enabled(b.fetch))
        .clicked()
    {
        cx.worker.send(Command::Fetch { background: false });
    }
    if ui
        .add(Button95::new(&b.pull_label).min_size(size).enabled(b.pull))
        .clicked()
    {
        cx.worker.send(Command::Pull(PullMode::FastForwardOnly));
    }
    if ui
        .add(Button95::new(&b.push_label).min_size(size).enabled(b.push))
        .clicked()
    {
        cx.worker.send(Command::Push(if b.publish {
            PushMode::SetUpstream
        } else {
            PushMode::Normal
        }));
    }
    // The automatic fetch has no progress box: offer its Cancel here.
    if cx.state.sync.running.is_some()
        && cx.state.sync.background
        && ui.add(Button95::new(s::CANCEL).min_size(size)).clicked()
    {
        cx.worker.cancel_network();
    }
    if cx.state.current.is_none() {
        return;
    }
    ui.separator();
    ui.label(s::BRANCH_LABEL);
    let head_name = current_branch(&cx.state.branches)
        .map(|b| b.name.clone())
        .unwrap_or_else(|| s::DETACHED_AT.to_string());
    let mut pick: Option<String> = None;
    let locals: Vec<Branch> = cx
        .state
        .branches
        .iter()
        .filter(|b| !b.remote)
        .cloned()
        .collect();
    let remotes: Vec<Branch> = remote_only(&cx.state.branches)
        .into_iter()
        .cloned()
        .collect();
    combo_box(ui, "branch_picker", &head_name, 200.0, |ui| {
        for l in &locals {
            let label = if l.is_head {
                format!("* {}", l.name)
            } else {
                format!("   {}", l.name)
            };
            if ui.button(label).clicked() && !l.is_head {
                pick = Some(l.name.clone());
            }
        }
        if !remotes.is_empty() {
            ui.separator();
            ui.label(egui::RichText::new(s::REMOTE_BRANCHES).color(win95::theme::GRAY));
            for r in &remotes {
                if ui.button(format!("   {}", r.name)).clicked() {
                    pick = Some(r.name.clone());
                }
            }
        }
    });
    if let Some(name) = pick {
        cx.worker.send(Command::SwitchBranch { name, stash: false });
    }
    let idle = cx.state.sync.running.is_none();
    if ui
        .add(
            Button95::new(s::NEW_BRANCH)
                .min_size(egui::vec2(90.0, 22.0))
                .enabled(idle),
        )
        .clicked()
    {
        cx.state.dialog = Some(PendingDialog::NewBranch {
            name: String::new(),
            switch: true,
        });
    }
}

/// Repository menu entries for sub-project 3.
pub fn menu_entries(ui: &mut egui::Ui, cx: &mut Ctx<'_>) {
    let b = sync_buttons(cx.state);
    if ui
        .add_enabled(b.fetch, egui::Button::new(s::FETCH))
        .clicked()
    {
        cx.worker.send(Command::Fetch { background: false });
    }
    if ui
        .add_enabled(b.pull, egui::Button::new(b.pull_label.clone()))
        .clicked()
    {
        cx.worker.send(Command::Pull(PullMode::FastForwardOnly));
    }
    if ui
        .add_enabled(b.push, egui::Button::new(b.push_label.clone()))
        .clicked()
    {
        cx.worker.send(Command::Push(if b.publish {
            PushMode::SetUpstream
        } else {
            PushMode::Normal
        }));
    }
    ui.separator();
    let open = cx.state.current.is_some();
    let head = current_branch(&cx.state.branches).map(|b| b.name.clone());
    if ui
        .add_enabled(open, egui::Button::new(s::NEW_BRANCH_MENU))
        .clicked()
    {
        cx.state.dialog = Some(PendingDialog::NewBranch {
            name: String::new(),
            switch: true,
        });
    }
    if ui
        .add_enabled(head.is_some(), egui::Button::new(s::RENAME_BRANCH_MENU))
        .clicked()
        && let Some(old) = head.clone()
    {
        cx.state.dialog = Some(PendingDialog::RenameBranch {
            name: old.clone(),
            old,
        });
    }
    if ui
        .add_enabled(open, egui::Button::new(s::DELETE_BRANCH_MENU))
        .clicked()
    {
        cx.state.dialog = Some(PendingDialog::DeleteBranch {
            name: String::new(),
        });
    }
    if let Some(op) = cx.state.operation {
        ui.separator();
        let label = match op {
            gitcore::Operation::Merge => s::ABORT_MERGE,
            gitcore::Operation::Rebase => s::ABORT_REBASE,
        };
        if ui.button(label).clicked() {
            cx.worker.send(Command::AbortOperation);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use gitcore::{Head, RepoSummary};

    fn branch(
        name: &str,
        remote: bool,
        head: bool,
        upstream: Option<&str>,
        ahead: usize,
        behind: usize,
    ) -> Branch {
        Branch {
            name: name.into(),
            remote,
            is_head: head,
            upstream: upstream.map(Into::into),
            ahead,
            behind,
        }
    }

    fn state_with(branches: Vec<Branch>) -> AppState {
        let mut s = AppState::new(Config::default());
        s.current = Some(RepoSummary {
            name: "r".into(),
            path: "/r".into(),
            head: Head::Branch("main".into()),
            origin_url: Some("https://github.com/o/r.git".into()),
            last_commit: None,
        });
        s.branches = branches;
        s
    }

    #[test]
    fn buttons_follow_ahead_behind_and_upstream() {
        let s = state_with(vec![
            branch("main", false, true, Some("origin/main"), 2, 3),
            branch("origin/main", true, false, None, 0, 0),
        ]);
        let b = sync_buttons(&s);
        assert!(b.fetch && b.pull && b.push && !b.publish);
        assert_eq!(
            (b.pull_label.as_str(), b.push_label.as_str()),
            ("Pull (3)", "Push (2)")
        );

        let s = state_with(vec![branch("main", false, true, Some("origin/main"), 0, 0)]);
        assert!(!sync_buttons(&s).push, "nothing to push");

        let s = state_with(vec![branch("topic", false, true, None, 0, 0)]);
        let b = sync_buttons(&s);
        assert!(b.publish && b.push && !b.pull);
        assert_eq!(b.push_label, "Publish");
    }

    #[test]
    fn nothing_while_busy_or_detached_or_merging() {
        let mut s = state_with(vec![branch("main", false, true, Some("origin/main"), 1, 1)]);
        s.sync.running = Some(crate::protocol::SyncOp::Fetch);
        let b = sync_buttons(&s);
        assert!(!b.fetch && !b.pull && !b.push);
        s.sync.running = None;
        s.operation = Some(gitcore::Operation::Merge);
        assert!(!sync_buttons(&s).pull);
        let s = state_with(vec![]);
        let b = sync_buttons(&s);
        assert!(!b.pull && !b.push, "detached HEAD");
    }

    #[test]
    fn remote_branches_already_checked_out_are_hidden() {
        let b = vec![
            branch("main", false, true, None, 0, 0),
            branch("origin/main", true, false, None, 0, 0),
            branch("origin/topic", true, false, None, 0, 0),
        ];
        let names: Vec<_> = remote_only(&b)
            .into_iter()
            .map(|b| b.name.as_str())
            .collect();
        assert_eq!(names, ["origin/topic"]);
    }
}
