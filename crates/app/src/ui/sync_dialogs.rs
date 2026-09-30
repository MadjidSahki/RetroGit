//! Dialogs of sub-project 3 and the network progress box.

use gitcore::{PullMode, PushMode};
use win95::{Button95, Dialog, Icon, ProgressBar95, checkbox, combo_box, text_field};

use super::Ctx;
use crate::protocol::{Command, SyncOp};
use crate::state::{PendingDialog, branch_name_error};
use crate::strings as s;

/// What the user decided in a dialog this frame.
#[derive(Clone)]
enum Outcome {
    Keep(PendingDialog),
    Close,
    Send(Command),
    /// Replace the dialog by another one (e.g. a second confirmation).
    Next(PendingDialog),
}

pub fn show(egui_ctx: &egui::Context, cx: &mut Ctx<'_>) {
    progress(egui_ctx, cx);
    let Some(dialog) = cx.state.dialog.take() else {
        return;
    };
    let outcome = match dialog.clone() {
        PendingDialog::NewBranch { name, switch } => {
            name_dialog(egui_ctx, cx, s::NEW_BRANCH_TITLE, None, name, switch)
        }
        PendingDialog::RenameBranch { old, name } => {
            name_dialog(egui_ctx, cx, s::RENAME_BRANCH_TITLE, Some(old), name, false)
        }
        PendingDialog::DeleteBranch { name } => delete_dialog(egui_ctx, cx, name),
        PendingDialog::DeleteNotMerged { name } => choice(
            egui_ctx,
            s::DELETE_BRANCH_TITLE,
            s::NOT_MERGED_QUESTION,
            dialog,
            vec![(
                s::DELETE_ANYWAY,
                Outcome::Send(Command::DeleteBranch { name, force: true }),
            )],
        ),
        PendingDialog::Diverged { ahead, behind } => {
            let q = format!(
                "Your branch and its upstream have diverged (↑{ahead} ↓{behind}). How do you want to integrate the remote changes?"
            );
            let buttons = vec![
                (s::MERGE, Outcome::Send(Command::Pull(PullMode::Merge))),
                (s::REBASE, Outcome::Send(Command::Pull(PullMode::Rebase))),
            ];
            choice(egui_ctx, s::DIVERGED_TITLE, &q, dialog, buttons)
        }
        PendingDialog::PushRejected { can_force } => {
            let mut buttons = vec![(
                s::PULL,
                Outcome::Send(Command::Pull(PullMode::FastForwardOnly)),
            )];
            if can_force {
                buttons.push((
                    s::FORCE_PUSH,
                    Outcome::Next(PendingDialog::ConfirmForcePush),
                ));
            }
            choice(
                egui_ctx,
                s::PUSH_REJECTED_TITLE,
                s::ERR_PUSH_REJECTED,
                dialog,
                buttons,
            )
        }
        PendingDialog::ConfirmForcePush => choice(
            egui_ctx,
            s::PUSH_REJECTED_TITLE,
            s::FORCE_PUSH_CONFIRM,
            dialog,
            vec![(
                s::FORCE,
                Outcome::Send(Command::Push(PushMode::ForceWithLease)),
            )],
        ),
        PendingDialog::WouldOverwrite { branch, files } => {
            let q = format!("{}\n\n{}", s::ERR_WOULD_OVERWRITE, files.join("\n"));
            let buttons = vec![(
                s::STASH_SWITCH,
                Outcome::Send(Command::SwitchBranch {
                    name: branch,
                    stash: true,
                }),
            )];
            choice(egui_ctx, s::SWITCH_TITLE, &q, dialog, buttons)
        }
    };
    match outcome {
        Outcome::Keep(d) | Outcome::Next(d) => cx.state.dialog = Some(d),
        Outcome::Close => {}
        Outcome::Send(cmd) => cx.worker.send(cmd),
    }
}

/// Question, one button per choice, and Cancel.
fn choice(
    egui_ctx: &egui::Context,
    title: &str,
    question: &str,
    dialog: PendingDialog,
    buttons: Vec<(&'static str, Outcome)>,
) -> Outcome {
    let mut picked: Option<Outcome> = None;
    let mut cancel = false;
    let r = Dialog::new(("sync_choice", title), title)
        .width(420.0)
        .show(egui_ctx, |ui| {
            ui.horizontal(|ui| {
                win95::icon::icon(ui, Icon::Warning);
                ui.add(egui::Label::new(question).wrap());
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                for (label, outcome) in &buttons {
                    if ui
                        .add(Button95::new(*label).min_size(egui::vec2(90.0, 23.0)))
                        .clicked()
                    {
                        picked = Some(outcome.clone());
                    }
                }
                cancel = ui.add(Button95::new(s::CANCEL)).clicked();
            });
        });
    match picked {
        Some(o) => o,
        None if cancel || r.close_requested => Outcome::Close,
        None => Outcome::Keep(dialog),
    }
}

fn name_dialog(
    egui_ctx: &egui::Context,
    cx: &mut Ctx<'_>,
    title: &str,
    old: Option<String>,
    mut name: String,
    mut switch: bool,
) -> Outcome {
    let error = branch_name_error(&name, &cx.state.branches);
    let mut ok = false;
    let mut cancel = false;
    let r = Dialog::new(("branch_name", title), title)
        .width(360.0)
        .show(egui_ctx, |ui| {
            ui.label(s::BRANCH_NAME);
            let resp = text_field(ui, &mut name, 320.0, false);
            if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                ok = true;
            }
            if !name.is_empty()
                && let Some(e) = &error
            {
                ui.label(egui::RichText::new(e).color(egui::Color32::from_rgb(0x80, 0, 0)));
            }
            if old.is_none() {
                checkbox(ui, &mut switch, s::SWITCH_TO_IT);
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ok |= ui
                    .add(Button95::new(s::OK).enabled(error.is_none()))
                    .clicked();
                cancel = ui.add(Button95::new(s::CANCEL)).clicked();
            });
        });
    if cancel || r.close_requested {
        return Outcome::Close;
    }
    if ok && branch_name_error(&name, &cx.state.branches).is_none() {
        let name = name.trim().to_string();
        return Outcome::Send(match old {
            Some(old) => Command::RenameBranch { old, new: name },
            None => Command::CreateBranch { name, switch },
        });
    }
    Outcome::Keep(match old {
        Some(old) => PendingDialog::RenameBranch { old, name },
        None => PendingDialog::NewBranch { name, switch },
    })
}

fn delete_dialog(egui_ctx: &egui::Context, cx: &mut Ctx<'_>, mut name: String) -> Outcome {
    let candidates: Vec<String> = cx
        .state
        .branches
        .iter()
        .filter(|b| !b.remote && !b.is_head)
        .map(|b| b.name.clone())
        .collect();
    let mut delete = false;
    let mut cancel = false;
    let r = Dialog::new("delete_branch", s::DELETE_BRANCH_TITLE)
        .width(360.0)
        .show(egui_ctx, |ui| {
            if candidates.is_empty() {
                ui.label(s::CANNOT_DELETE_CURRENT);
            } else {
                if !candidates.contains(&name) {
                    name = candidates[0].clone();
                }
                let shown = name.clone();
                combo_box(ui, "delete_branch_pick", &shown, 320.0, |ui| {
                    for c in &candidates {
                        if ui.button(c).clicked() {
                            name = c.clone();
                        }
                    }
                });
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                delete = ui
                    .add(Button95::new(s::DELETE).enabled(!candidates.is_empty()))
                    .clicked();
                cancel = ui.add(Button95::new(s::CANCEL)).clicked();
            });
        });
    if cancel || r.close_requested {
        Outcome::Close
    } else if delete {
        Outcome::Send(Command::DeleteBranch { name, force: false })
    } else {
        Outcome::Keep(PendingDialog::DeleteBranch { name })
    }
}

/// Modal progress for user-started network operations (background fetch: status bar only).
fn progress(egui_ctx: &egui::Context, cx: &mut Ctx<'_>) {
    let sync = &cx.state.sync;
    let Some(op) = sync.running else { return };
    if sync.background {
        return;
    }
    let title = match op {
        SyncOp::Fetch => s::FETCHING,
        SyncOp::Pull => s::PULLING,
        SyncOp::Push => s::PUSHING,
    };
    let (phase, pct) = match &sync.progress {
        Some(p) => (
            format!("{}: {}%", p.phase, p.percent.unwrap_or(0)),
            p.percent.map(|v| v as f32 / 100.0),
        ),
        None => (title.to_string(), None),
    };
    let r = Dialog::new("sync_progress", title)
        .width(340.0)
        .show(egui_ctx, |ui| {
            ui.label(phase);
            ui.add(ProgressBar95::new(pct).width(ui.available_width()));
            ui.add_space(8.0);
            ui.vertical_centered(|ui| ui.add(Button95::new(s::CANCEL)).clicked())
                .inner
        });
    if r.inner || r.close_requested {
        cx.worker.cancel_network();
    }
}
