//! Dialogs of sub-project 6c: revert of a merge, reset, interactive rebase, tags, stashes.

use gitcore::{ResetMode, TodoAction, validate_todo};
use win95::{Button95, Dialog, checkbox, combo_box, text_area, text_field};

use super::Ctx;
use crate::protocol::Command;
use crate::state::{GitDialog, move_item};
use crate::strings as s;

const BUTTON: egui::Vec2 = egui::vec2(110.0, 23.0);

/// Label of an action in the rebase list.
pub fn action_label(a: &TodoAction) -> &'static str {
    match a {
        TodoAction::Pick => s::TODO_PICK,
        TodoAction::Reword(_) => s::TODO_REWORD,
        TodoAction::Squash(_) => s::TODO_SQUASH,
        TodoAction::Fixup => s::TODO_FIXUP,
        TodoAction::Drop => s::TODO_DROP,
    }
}

pub fn show(egui_ctx: &egui::Context, cx: &mut Ctx<'_>) {
    let Some(dialog) = cx.state.git_dialog.take() else {
        return;
    };
    // `Some(d)`: keep the dialog (possibly changed); `None`: close it.
    let keep: Option<GitDialog> = match dialog {
        GitDialog::RevertMerge { id, parent } => revert_merge(egui_ctx, cx, id, parent),
        GitDialog::Reset {
            id,
            mode,
            hard_confirmed,
            drops_pushed,
        } => reset(egui_ctx, cx, id, mode, hard_confirmed, drops_pushed),
        GitDialog::Rebase {
            base,
            items,
            pushed,
        } => rebase(egui_ctx, cx, base, items, pushed),
        GitDialog::CreateTag {
            id,
            name,
            message,
            annotated,
        } => create_tag(egui_ctx, cx, id, name, message, annotated),
        GitDialog::DeleteTag { name, remote } => delete_tag(egui_ctx, cx, name, remote),
        GitDialog::Tags { filter, selected } => tags(egui_ctx, cx, filter, selected),
        GitDialog::StashSave { message, untracked } => stash_save(egui_ctx, cx, message, untracked),
        GitDialog::StashRetry { retry, files } => {
            let question = format!("{}\n\n{}", s::STASH_RETRY_QUESTION, files.join("\n"));
            match confirm_with(
                egui_ctx,
                s::STASH_RETRY_TITLE,
                &question,
                s::STASH_AND_RETRY,
            ) {
                Some(true) => {
                    cx.worker.send(Command::StashAndRetry(retry));
                    None
                }
                Some(false) => None,
                None => Some(GitDialog::StashRetry { retry, files }),
            }
        }
        GitDialog::StashDrop { index } => {
            match confirm(
                egui_ctx,
                s::STASH_DROP_TITLE,
                &s::STASH_DROP_CONFIRM.replace("{n}", &index.to_string()),
            ) {
                Some(true) => {
                    cx.worker.send(Command::StashDrop(index));
                    None
                }
                Some(false) => None,
                None => Some(GitDialog::StashDrop { index }),
            }
        }
    };
    // A new dialog opened meanwhile (e.g. Tags -> New tag) wins.
    if cx.state.git_dialog.is_none() {
        cx.state.git_dialog = keep;
    }
}

/// A question with OK / Cancel: `Some(true)` OK, `Some(false)` cancelled, `None` open.
fn confirm(egui_ctx: &egui::Context, title: &str, question: &str) -> Option<bool> {
    confirm_with(egui_ctx, title, question, s::OK)
}

/// [`confirm`] with a named OK button.
fn confirm_with(egui_ctx: &egui::Context, title: &str, question: &str, ok: &str) -> Option<bool> {
    let mut result = None;
    let r = Dialog::new(("git_confirm", title), title)
        .width(400.0)
        .show(egui_ctx, |ui| {
            ui.add(egui::Label::new(question).wrap());
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.add(Button95::new(ok).min_size(BUTTON)).clicked() {
                    result = Some(true);
                }
                if ui.add(Button95::new(s::CANCEL).min_size(BUTTON)).clicked() {
                    result = Some(false);
                }
            });
        });
    if r.close_requested {
        return Some(false);
    }
    result
}

fn revert_merge(
    egui_ctx: &egui::Context,
    cx: &mut Ctx<'_>,
    id: String,
    mut parent: u32,
) -> Option<GitDialog> {
    let (mut ok, mut cancel) = (false, false);
    let r = Dialog::new("revert_merge", s::REVERT_MERGE_TITLE)
        .width(420.0)
        .show(egui_ctx, |ui| {
            ui.add(egui::Label::new(s::REVERT_MERGE_HELP).wrap());
            for p in [1, 2] {
                let label = if p == 1 {
                    s::KEEP_PARENT_1
                } else {
                    s::KEEP_PARENT_2
                };
                if ui.selectable_label(parent == p, label).clicked() {
                    parent = p;
                }
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ok = ui.add(Button95::new(s::REVERT).min_size(BUTTON)).clicked();
                cancel = ui.add(Button95::new(s::CANCEL).min_size(BUTTON)).clicked();
            });
        });
    if cancel || r.close_requested {
        return None;
    }
    if ok {
        cx.worker.send(Command::Revert {
            id,
            mainline: Some(parent),
        });
        return None;
    }
    Some(GitDialog::RevertMerge { id, parent })
}

fn reset(
    egui_ctx: &egui::Context,
    cx: &mut Ctx<'_>,
    id: String,
    mut mode: ResetMode,
    mut hard_confirmed: bool,
    drops_pushed: bool,
) -> Option<GitDialog> {
    let (mut ok, mut cancel) = (false, false);
    let short: String = id.chars().take(7).collect();
    let title = s::RESET_TITLE.replace("{commit}", &short);
    let r = Dialog::new("reset", &title)
        .width(460.0)
        .show(egui_ctx, |ui| {
            for (m, label) in [
                (ResetMode::Soft, s::RESET_SOFT),
                (ResetMode::Mixed, s::RESET_MIXED),
                (ResetMode::Hard, s::RESET_HARD),
            ] {
                if ui.selectable_label(mode == m, label).clicked() {
                    mode = m;
                }
            }
            if mode == ResetMode::Hard {
                ui.label(egui::RichText::new(s::RESET_HARD_UNTRACKED).color(win95::theme::GRAY));
                checkbox(ui, &mut hard_confirmed, s::RESET_HARD_CONFIRM);
            }
            if drops_pushed {
                ui.label(
                    egui::RichText::new(s::RESET_DROPS_PUSHED)
                        .color(egui::Color32::from_rgb(0xA0, 0, 0)),
                );
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let ready = mode != ResetMode::Hard || hard_confirmed;
                ok = ui
                    .add(Button95::new(s::RESET).min_size(BUTTON).enabled(ready))
                    .clicked();
                cancel = ui.add(Button95::new(s::CANCEL).min_size(BUTTON)).clicked();
            });
        });
    if cancel || r.close_requested {
        return None;
    }
    if ok {
        cx.worker.send(Command::Reset { id, mode });
        return None;
    }
    Some(GitDialog::Reset {
        id,
        mode,
        hard_confirmed,
        drops_pushed,
    })
}

fn rebase(
    egui_ctx: &egui::Context,
    cx: &mut Ctx<'_>,
    base: String,
    mut items: Vec<gitcore::TodoItem>,
    pushed: usize,
) -> Option<GitDialog> {
    let (mut start, mut cancel) = (false, false);
    let short: String = base.chars().take(7).collect();
    let title = s::REBASE_TITLE.replace("{base}", &short);
    let r = Dialog::new("interactive_rebase", &title)
        .width(640.0)
        .show(egui_ctx, |ui| {
            egui::ScrollArea::vertical()
                .max_height(380.0)
                .show(ui, |ui| {
                    let mut moves: Option<(usize, bool)> = None;
                    for (i, item) in items.iter_mut().enumerate() {
                        ui.horizontal(|ui| {
                            let mut picked = None;
                            combo_box(
                                ui,
                                ("todo_action", i),
                                action_label(&item.action),
                                90.0,
                                |ui| {
                                    for a in [
                                        TodoAction::Pick,
                                        TodoAction::Reword(item.summary.clone()),
                                        TodoAction::Squash(None),
                                        TodoAction::Fixup,
                                        TodoAction::Drop,
                                    ] {
                                        if ui.selectable_label(false, action_label(&a)).clicked() {
                                            picked = Some(a);
                                        }
                                    }
                                },
                            );
                            if let Some(a) = picked {
                                item.action = a;
                            }
                            let short: String = item.id.chars().take(7).collect();
                            ui.label(egui::RichText::new(short).monospace());
                            ui.allocate_ui_with_layout(
                                egui::vec2(280.0, 18.0),
                                egui::Layout::left_to_right(egui::Align::Center),
                                |ui| {
                                    ui.set_min_width(280.0);
                                    ui.add(egui::Label::new(&item.summary).truncate());
                                },
                            );
                            if ui.add(Button95::new(s::MOVE_UP)).clicked() {
                                moves = Some((i, true));
                            }
                            if ui.add(Button95::new(s::MOVE_DOWN)).clicked() {
                                moves = Some((i, false));
                            }
                        });
                        match &mut item.action {
                            TodoAction::Reword(m) => {
                                ui.horizontal(|ui| {
                                    ui.add_space(100.0);
                                    text_field(ui, m, 480.0, false);
                                });
                            }
                            TodoAction::Squash(m) => {
                                ui.horizontal(|ui| {
                                    ui.add_space(100.0);
                                    let mut text = m.clone().unwrap_or_default();
                                    ui.label(s::SQUASH_MESSAGE);
                                    text_area(ui, &mut text, 380.0, 2);
                                    *m = (!text.trim().is_empty()).then_some(text);
                                });
                            }
                            _ => {}
                        }
                    }
                    if let Some((i, up)) = moves {
                        move_item(&mut items, i, up);
                    }
                });
            if pushed > 0 {
                ui.label(
                    egui::RichText::new(s::REBASE_PUSHED.replace("{n}", &pushed.to_string()))
                        .color(egui::Color32::from_rgb(0xA0, 0, 0)),
                );
            }
            let check = validate_todo(&items);
            if let Err(why) = check {
                ui.label(egui::RichText::new(why).color(win95::theme::GRAY));
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                start = ui
                    .add(
                        Button95::new(s::START_REBASE)
                            .min_size(BUTTON)
                            .enabled(check.is_ok()),
                    )
                    .clicked();
                cancel = ui.add(Button95::new(s::CANCEL).min_size(BUTTON)).clicked();
            });
        });
    if cancel || r.close_requested {
        return None;
    }
    if start {
        cx.worker.send(Command::InteractiveRebase { base, items });
        return None;
    }
    Some(GitDialog::Rebase {
        base,
        items,
        pushed,
    })
}

fn create_tag(
    egui_ctx: &egui::Context,
    cx: &mut Ctx<'_>,
    id: String,
    mut name: String,
    mut message: String,
    mut annotated: bool,
) -> Option<GitDialog> {
    let (mut ok, mut cancel) = (false, false);
    let error = crate::state::tag_name_error(&name, &cx.state.tags);
    let r = Dialog::new("create_tag", s::CREATE_TAG_TITLE)
        .width(420.0)
        .show(egui_ctx, |ui| {
            ui.label(s::TAG_NAME);
            text_field(ui, &mut name, 400.0, false);
            checkbox(ui, &mut annotated, s::TAG_ANNOTATED);
            if annotated {
                ui.label(s::TAG_MESSAGE);
                text_area(ui, &mut message, 400.0, 3);
            }
            if let Some(e) = &error
                && !name.trim().is_empty()
            {
                ui.label(egui::RichText::new(e).color(win95::theme::GRAY));
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let ready = error.is_none() && (!annotated || !message.trim().is_empty());
                ok = ui
                    .add(Button95::new(s::CREATE).min_size(BUTTON).enabled(ready))
                    .clicked();
                cancel = ui.add(Button95::new(s::CANCEL).min_size(BUTTON)).clicked();
            });
        });
    if cancel || r.close_requested {
        return None;
    }
    if ok {
        cx.worker.send(Command::CreateTag {
            name: name.trim().to_string(),
            id,
            message: annotated.then(|| message.trim().to_string()),
        });
        return None;
    }
    Some(GitDialog::CreateTag {
        id,
        name,
        message,
        annotated,
    })
}

fn delete_tag(
    egui_ctx: &egui::Context,
    cx: &mut Ctx<'_>,
    name: String,
    mut remote: bool,
) -> Option<GitDialog> {
    let (mut ok, mut cancel) = (false, false);
    let r = Dialog::new("delete_tag", s::DELETE_TAG_TITLE)
        .width(380.0)
        .show(egui_ctx, |ui| {
            ui.label(s::DELETE_TAG_CONFIRM.replace("{name}", &name));
            checkbox(ui, &mut remote, s::DELETE_TAG_REMOTE);
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ok = ui.add(Button95::new(s::DELETE).min_size(BUTTON)).clicked();
                cancel = ui.add(Button95::new(s::CANCEL).min_size(BUTTON)).clicked();
            });
        });
    if cancel || r.close_requested {
        return None;
    }
    if ok {
        cx.worker.send(Command::DeleteTag { name, remote });
        return None;
    }
    Some(GitDialog::DeleteTag { name, remote })
}

fn tags(
    egui_ctx: &egui::Context,
    cx: &mut Ctx<'_>,
    mut filter: String,
    mut selected: Option<String>,
) -> Option<GitDialog> {
    let mut close = false;
    let mut next: Option<GitDialog> = None;
    let r = Dialog::new("tags", s::TAGS_TITLE)
        .width(520.0)
        .show(egui_ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(s::FILTER);
                text_field(ui, &mut filter, 300.0, false);
            });
            let f = filter.trim().to_lowercase();
            win95::bevel_frame(ui, win95::Bevel::Field, win95::theme::WHITE, 2, |ui| {
                ui.set_min_size(egui::vec2(ui.available_width(), 160.0));
                egui::ScrollArea::vertical()
                    .max_height(300.0)
                    .show(ui, |ui| {
                        if cx.state.tags.is_empty() {
                            ui.label(s::NO_TAGS);
                        }
                        for t in cx
                            .state
                            .tags
                            .iter()
                            .filter(|t| f.is_empty() || t.name.to_lowercase().contains(&f))
                        {
                            let short: String = t.commit.chars().take(7).collect();
                            let kind = if t.annotated {
                                s::TAG_ANNOTATED_SHORT
                            } else {
                                s::TAG_LIGHT_SHORT
                            };
                            let text = format!("{}  {short}  {kind}  {}", t.name, t.message);
                            if ui
                                .selectable_label(
                                    selected.as_deref() == Some(t.name.as_str()),
                                    text,
                                )
                                .clicked()
                            {
                                selected = Some(t.name.clone());
                            }
                        }
                    });
            });
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.add(Button95::new(s::NEW_TAG).min_size(BUTTON)).clicked() {
                    let head = cx
                        .state
                        .history
                        .entries
                        .first()
                        .map(|e| e.id.clone())
                        .unwrap_or_else(|| "HEAD".into());
                    next = Some(GitDialog::CreateTag {
                        id: head,
                        name: String::new(),
                        message: String::new(),
                        annotated: true,
                    });
                }
                let has = selected.is_some();
                if ui
                    .add(Button95::new(s::DELETE).min_size(BUTTON).enabled(has))
                    .clicked()
                    && let Some(name) = selected.clone()
                {
                    next = Some(GitDialog::DeleteTag {
                        name,
                        remote: false,
                    });
                }
                if ui
                    .add(Button95::new(s::PUSH_TAG).min_size(BUTTON).enabled(has))
                    .clicked()
                {
                    cx.worker.send(Command::PushTags(selected.clone()));
                }
                if ui
                    .add(Button95::new(s::PUSH_ALL_TAGS).min_size(BUTTON))
                    .clicked()
                {
                    cx.worker.send(Command::PushTags(None));
                }
                close = ui.add(Button95::new(s::CLOSE).min_size(BUTTON)).clicked();
            });
        });
    if close || r.close_requested {
        return None;
    }
    if next.is_some() {
        return next;
    }
    Some(GitDialog::Tags { filter, selected })
}

fn stash_save(
    egui_ctx: &egui::Context,
    cx: &mut Ctx<'_>,
    mut message: String,
    mut untracked: bool,
) -> Option<GitDialog> {
    let (mut ok, mut cancel) = (false, false);
    let r = Dialog::new("stash_save", s::STASH_SAVE_TITLE)
        .width(420.0)
        .show(egui_ctx, |ui| {
            ui.label(s::STASH_MESSAGE);
            text_field(ui, &mut message, 400.0, false);
            checkbox(ui, &mut untracked, s::STASH_UNTRACKED);
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ok = ui.add(Button95::new(s::STASH).min_size(BUTTON)).clicked();
                cancel = ui.add(Button95::new(s::CANCEL).min_size(BUTTON)).clicked();
            });
        });
    if cancel || r.close_requested {
        return None;
    }
    if ok {
        cx.worker.send(Command::StashSave {
            message: message.trim().to_string(),
            untracked,
        });
        return None;
    }
    Some(GitDialog::StashSave { message, untracked })
}
