//! The Repositories side panel: recent repositories, and the open one's branches as a
//! folder tree (arrow to unfold, double-click to check out, right-click for a new branch).

use std::path::PathBuf;

use egui::{Align2, Rect, ScrollArea, Sense, Stroke, pos2, vec2};
use win95::{Bevel, bevel_frame};

use super::Ctx;
use crate::protocol::Command;
use crate::state::PendingDialog;
use crate::state::repo_tree::{Row, RowKind, sidebar_rows};
use crate::strings as s;

const ROW: f32 = 18.0;
const INDENT: f32 = 16.0;
/// Width of the unfold arrow of a repository row.
const ARROW: f32 = 14.0;

/// What a click asked for this frame (applied after drawing).
enum Action {
    OpenRepo(PathBuf),
    ToggleRepo(PathBuf),
    RemoveRepo(PathBuf),
    ToggleFolder(String),
    Select(String),
    Checkout(String),
    NewBranchFrom(String),
}

pub fn show(ui: &mut egui::Ui, cx: &mut Ctx<'_>) {
    ui.label(s::REPOSITORIES);
    let pal = win95::theme::palette(ui.ctx());
    let current = cx.state.current.as_ref().map(|c| c.path.clone());
    let rows = sidebar_rows(
        &cx.state.recents_sorted(),
        current.as_deref(),
        &cx.state.repo_tree,
        &cx.state.branches,
    );
    let idle = cx.state.sync.running.is_none();
    let mut action: Option<Action> = None;
    bevel_frame(ui, Bevel::Field, pal.window, 2, |ui| {
        ui.set_min_size(ui.available_size());
        // `show_rows` sizes rows with the spacing of this `ui`: zero it first, as ListView.
        ui.spacing_mut().item_spacing.y = 0.0;
        ScrollArea::both()
            .id_salt("repo_tree_scroll")
            .auto_shrink([false, false])
            .show_rows(ui, ROW, rows.len(), |ui, range| {
                for (i, row) in rows[range.clone()].iter().enumerate() {
                    if let Some(a) = row_ui(ui, cx, row, range.start + i, idle) {
                        action = Some(a);
                    }
                }
            });
    });
    if let Some(a) = action {
        apply(cx, a);
    }
}

fn row_ui(ui: &mut egui::Ui, cx: &Ctx<'_>, row: &Row, index: usize, idle: bool) -> Option<Action> {
    let pal = win95::theme::palette(ui.ctx());
    let font = win95::theme::font(win95::theme::FONT_SIZE);
    let indent = 2.0 + row.depth as f32 * INDENT;
    let text_width = row.label.chars().count() as f32 * 8.0;
    let width = (indent + ARROW + 22.0 + text_width).max(ui.available_width());
    let (rect, resp) = ui.allocate_exact_size(vec2(width, ROW), Sense::click());
    let mut action = None;
    match &row.kind {
        RowKind::Repo {
            path,
            current,
            expanded,
        } => {
            let label = row.label.clone();
            resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &label));
            let arrow =
                Rect::from_min_size(pos2(rect.left() + indent, rect.top()), vec2(ARROW, ROW));
            let arrow_resp =
                ui.interact(arrow, ui.id().with(("repo_arrow", index)), Sense::click());
            let arrow_label = if *expanded {
                s::hide_branches(&row.label)
            } else {
                s::show_branches(&row.label)
            };
            arrow_resp.widget_info(|| {
                egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &arrow_label)
            });
            let missing = cx.state.missing.contains(path);
            let color = if *current {
                ui.painter().rect_filled(
                    Rect::from_min_max(pos2(arrow.right(), rect.top()), rect.max),
                    0.0,
                    pal.selection,
                );
                pal.selection_text
            } else if missing {
                pal.gray_text
            } else {
                pal.window_text
            };
            paint_arrow(ui.painter(), arrow, *expanded, pal.window_text);
            ui.painter().text(
                pos2(arrow.right() + 4.0, rect.center().y),
                Align2::LEFT_CENTER,
                &row.label,
                font,
                color,
            );
            if single_click(&arrow_resp) {
                action = Some(Action::ToggleRepo(path.clone()));
            } else if single_click(&resp) {
                action = Some(Action::OpenRepo(path.clone()));
            }
            resp.context_menu(|ui| {
                if ui.button(s::REMOVE_FROM_LIST).clicked() {
                    action = Some(Action::RemoveRepo(path.clone()));
                }
            });
        }
        RowKind::Folder {
            key,
            path,
            remote,
            open,
        } => {
            let label = s::folder_label(path, *remote);
            resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &label));
            let icon = Rect::from_min_size(
                pos2(rect.left() + indent + ARROW, rect.top()),
                vec2(18.0, ROW),
            );
            win95::icon::folder(ui.painter(), icon, *open);
            ui.painter().text(
                pos2(icon.right() + 4.0, rect.center().y),
                Align2::LEFT_CENTER,
                &row.label,
                font,
                pal.window_text,
            );
            if single_click(&resp) {
                action = Some(Action::ToggleFolder(key.clone()));
            }
        }
        RowKind::Branch { name, current, .. } => {
            // Git refuses a checkout during a merge or rebase: not offered (as Pull/Push).
            let can_checkout = !*current && idle && cx.state.operation.is_none();
            let a11y = if *current {
                s::current_branch_label(name)
            } else {
                name.clone()
            };
            resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &a11y));
            let selected = cx.state.repo_tree.selected.as_deref() == Some(name.as_str());
            let text_left = rect.left() + indent + ARROW + 4.0;
            let color = if selected {
                ui.painter().rect_filled(
                    Rect::from_min_max(pos2(text_left - 2.0, rect.top()), rect.max),
                    0.0,
                    pal.selection,
                );
                pal.selection_text
            } else if *current {
                pal.link
            } else {
                pal.window_text
            };
            let at = pos2(text_left, rect.center().y);
            ui.painter()
                .text(at, Align2::LEFT_CENTER, &row.label, font.clone(), color);
            if *current {
                // No bold face in the Win95 font: a second pass one pixel right.
                let at = at + vec2(1.0, 0.0);
                ui.painter()
                    .text(at, Align2::LEFT_CENTER, &row.label, font, color);
            }
            // egui counts any click of the last 0.6 s: right after the arrow, a double-click
            // arrives as a triple-click.
            if resp.double_clicked() || resp.triple_clicked() {
                if can_checkout {
                    action = Some(Action::Checkout(name.clone()));
                }
            } else if resp.clicked() {
                action = Some(Action::Select(name.clone()));
            }
            // Flat menu items, like the menu bar (not raised buttons).
            resp.context_menu(|ui| {
                let checkout = egui::Button::new(s::CHECKOUT_BRANCH_MENU);
                if ui.add_enabled(can_checkout, checkout).clicked() {
                    action = Some(Action::Checkout(name.clone()));
                    ui.close();
                }
                let new_branch = egui::Button::new(s::NEW_BRANCH_FROM_MENU);
                if ui.add_enabled(idle, new_branch).clicked() {
                    action = Some(Action::NewBranchFrom(name.clone()));
                    ui.close();
                }
            });
        }
    }
    action
}

/// A click that is not the second (or third) of a double-click: toggles must not undo
/// themselves when users double-click folders like they do branches.
fn single_click(r: &egui::Response) -> bool {
    r.clicked() && !r.double_clicked() && !r.triple_clicked()
}

/// Win95 tree arrow: a small black triangle pointing right (folded) or down.
fn paint_arrow(p: &egui::Painter, rect: Rect, expanded: bool, color: egui::Color32) {
    let c = rect.center();
    let points = if expanded {
        vec![
            c + vec2(-4.0, -2.0),
            c + vec2(4.0, -2.0),
            c + vec2(0.0, 2.0),
        ]
    } else {
        vec![
            c + vec2(-2.0, -4.0),
            c + vec2(-2.0, 4.0),
            c + vec2(2.0, 0.0),
        ]
    };
    p.add(egui::Shape::convex_polygon(points, color, Stroke::NONE));
}

fn apply(cx: &mut Ctx<'_>, action: Action) {
    match action {
        Action::OpenRepo(path) => {
            cx.state.repo_tree.expand_after_open = None;
            open_repo(cx, path);
        }
        Action::ToggleRepo(path) => {
            if !cx.state.toggle_repo_branches(&path) {
                open_repo(cx, path);
            }
        }
        Action::RemoveRepo(path) => cx.state.remove_recent(&path),
        Action::ToggleFolder(key) => cx.state.repo_tree.toggle_folder(&key),
        Action::Select(name) => cx.state.repo_tree.selected = Some(name),
        Action::Checkout(name) => {
            cx.state.repo_tree.selected = Some(name.clone());
            cx.worker.send(Command::SwitchBranch { name, stash: false });
        }
        Action::NewBranchFrom(from) => {
            cx.state.dialog = Some(PendingDialog::NewBranch {
                name: String::new(),
                from: Some(from),
                switch: true,
            });
        }
    }
}

/// Same path as a click in the list: edited conflicts and pending comments ask first.
fn open_repo(cx: &mut Ctx<'_>, path: PathBuf) {
    if cx.state.changes.request_open_repo(&path)
        && let Some(cmd) = cx.state.request_repo_switch(Command::OpenRepo(path))
    {
        cx.worker.send(cmd);
    }
}
