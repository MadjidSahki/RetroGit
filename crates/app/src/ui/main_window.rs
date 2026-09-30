use egui::{Panel, UiBuilder, ViewportCommand};
use win95::{
    Bevel, Button95, Cell, Column, ListView, TitleAction, TitleBar, bevel_frame, status_bar,
};

use super::{Ctx, clone_dialog};
use crate::protocol::Command;
use crate::state::Auth;
use crate::strings as s;

pub fn show(ui: &mut egui::Ui, cx: &mut Ctx<'_>) {
    let full = ui.max_rect();
    ui.painter().rect_filled(full, 0.0, win95::theme::SILVER);
    win95::bevel::paint(ui.painter(), full, Bevel::Window);
    let inner = full.shrink(3.0);
    ui.scope_builder(UiBuilder::new().max_rect(inner), |ui| {
        Panel::top("chrome")
            .frame(egui::Frame::NONE)
            .show(ui, |ui| {
                title(ui, cx);
                menu(ui, cx);
                toolbar(ui, cx);
                ui.add_space(2.0);
            });
        Panel::bottom("status")
            .frame(egui::Frame::NONE)
            .show(ui, |ui| status(ui, cx));
        Panel::left("recents")
            .frame(egui::Frame::NONE.inner_margin(egui::Margin::same(2)))
            .resizable(true)
            .default_size(200.0)
            .show(ui, |ui| recents(ui, cx));
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.inner_margin(egui::Margin::same(2)))
            .show(ui, |ui| {
                if cx.state.current.is_some() {
                    let mut tab = match cx.state.tab {
                        crate::state::Tab::Changes => 0,
                        crate::state::Tab::History => 1,
                    };
                    win95::tabs(ui, &mut tab, &[s::TAB_CHANGES, s::TAB_HISTORY]);
                    cx.state.tab = if tab == 0 {
                        crate::state::Tab::Changes
                    } else {
                        crate::state::Tab::History
                    };
                    match cx.state.tab {
                        crate::state::Tab::Changes => super::changes::show(ui, cx),
                        crate::state::Tab::History => super::history::show(ui, cx),
                    }
                } else {
                    bevel_frame(ui, Bevel::Field, win95::theme::WHITE, 8, |ui| {
                        ui.set_min_size(ui.available_size());
                        ui.label(s::NO_REPO);
                    });
                }
            });
    });
    win95::resize_edges(ui, full);
}

fn title(ui: &mut egui::Ui, cx: &mut Ctx<'_>) {
    let text = match &cx.state.current {
        Some(c) => format!("{} - {}", s::APP_NAME, c.name),
        None => s::APP_NAME.to_string(),
    };
    let focused = ui.input(|i| i.viewport().focused.unwrap_or(true));
    let maximized = ui.input(|i| i.viewport().maximized.unwrap_or(false));
    // Drawn above dialogs so the window can still be moved/minimized/closed while one is open.
    let action = win95::above_dialogs(ui, "main_title", win95::title_bar::HEIGHT, |ui| {
        TitleBar::new(&text).active(focused).show(ui)
    });
    match action {
        TitleAction::StartDrag => ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag),
        TitleAction::Minimize => ui.ctx().send_viewport_cmd(ViewportCommand::Minimized(true)),
        TitleAction::ToggleMaximize => ui
            .ctx()
            .send_viewport_cmd(ViewportCommand::Maximized(!maximized)),
        TitleAction::Close => ui.ctx().send_viewport_cmd(ViewportCommand::Close),
        TitleAction::None => {}
    }
}

fn open_folder(cx: &mut Ctx<'_>) {
    if let Some(folder) = rfd::FileDialog::new().pick_folder() {
        cx.worker.send(Command::OpenRepo(folder));
    }
}

fn menu(ui: &mut egui::Ui, cx: &mut Ctx<'_>) {
    egui::MenuBar::new().ui(ui, |ui| {
        ui.menu_button(s::MENU_FILE, |ui| {
            if matches!(cx.state.auth, Auth::SignedIn(_)) {
                if ui.button(s::SIGN_OUT).clicked() {
                    cx.worker.send(Command::SignOut);
                }
            } else if ui.button(s::SIGN_IN_MENU).clicked() {
                if cx.state.auth == Auth::Offline {
                    cx.state.auth = Auth::Checking;
                    cx.worker.send(Command::ValidateToken);
                } else {
                    cx.state.sign_in.get_or_insert_with(Default::default);
                }
            }
            ui.separator();
            if ui.button(s::CLONE_MENU).clicked() {
                clone_dialog::open(cx);
            }
            if ui.button(s::OPEN_MENU).clicked() {
                open_folder(cx);
            }
            ui.separator();
            if ui.button(s::EXIT).clicked() {
                ui.ctx().send_viewport_cmd(ViewportCommand::Close);
            }
        });
        ui.menu_button(s::MENU_REPOSITORY, |ui| {
            let open = cx.state.current.is_some();
            if ui
                .add_enabled(open, egui::Button::new(s::COMMIT_MENU))
                .clicked()
            {
                cx.state.changes.focus_summary = true;
            }
            if ui
                .add_enabled(open, egui::Button::new(s::STAGE_ALL))
                .clicked()
            {
                let files: Vec<_> = cx.state.changes.unstaged().cloned().collect();
                cx.worker.send(super::changes::all_files_command(
                    &files,
                    gitcore::Side::Unstaged,
                ));
            }
            if ui
                .add_enabled(open, egui::Button::new(s::UNSTAGE_ALL))
                .clicked()
            {
                let files: Vec<_> = cx.state.changes.staged().cloned().collect();
                cx.worker.send(super::changes::all_files_command(
                    &files,
                    gitcore::Side::Staged,
                ));
            }
            ui.separator();
            super::sync_toolbar::menu_entries(ui, cx);
        });
        ui.menu_button(s::MENU_VIEW, |ui| {
            let current = cx.state.current.as_ref().map(|c| c.path.clone());
            if ui
                .add_enabled(current.is_some(), egui::Button::new(s::REFRESH_REPO))
                .clicked()
                && let Some(path) = current
            {
                cx.worker.send(Command::OpenRepo(path));
            }
        });
        ui.menu_button(s::MENU_HELP, |ui| {
            if ui.button(s::ABOUT_MENU).clicked() {
                cx.state.about = true;
            }
        });
    });
}

fn toolbar(ui: &mut egui::Ui, cx: &mut Ctx<'_>) {
    let size = egui::vec2(60.0, 22.0);
    ui.horizontal(|ui| {
        if ui.add(Button95::new(s::CLONE).min_size(size)).clicked() {
            clone_dialog::open(cx);
        }
        if ui.add(Button95::new(s::OPEN).min_size(size)).clicked() {
            open_folder(cx);
        }
        ui.separator();
        super::sync_toolbar::toolbar(ui, cx);
    });
}

const RECENT_COLUMNS: &[Column] = &[Column {
    title: s::REPOSITORIES,
    width: 400.0,
}];

fn recents(ui: &mut egui::Ui, cx: &mut Ctx<'_>) {
    ui.label(s::REPOSITORIES);
    let recent = cx.state.recents_sorted();
    let selected = cx.state.selected_recent();
    let missing = &cx.state.missing;
    let mut remove: Option<usize> = None;
    let height = ui.available_height();
    let resp = ListView::new("recents", RECENT_COLUMNS, recent.len())
        .header(false)
        .height(height)
        .context_menu(|row, ui| {
            if ui.button(s::REMOVE_FROM_LIST).clicked() {
                remove = Some(row);
            }
        })
        .show(ui, selected, |row, _| Cell {
            text: recent[row].name.clone(),
            dimmed: missing.contains(&recent[row].path),
        });
    if let Some(row) = resp.clicked {
        cx.worker.send(Command::OpenRepo(recent[row].path.clone()));
    }
    if let Some(row) = remove {
        cx.state.remove_recent(&recent[row].path);
    }
}

fn status(ui: &mut egui::Ui, cx: &mut Ctx<'_>) {
    let who = match &cx.state.auth {
        Auth::SignedIn(u) => format!("Signed in: @{}", u.login),
        Auth::Checking => s::CHECKING.to_string(),
        Auth::Offline => s::OFFLINE.to_string(),
        _ => s::NOT_SIGNED_IN.to_string(),
    };
    let activity = if cx.state.repos_loading {
        s::LOADING
    } else if let Some(op) = cx.state.sync.running {
        match op {
            crate::protocol::SyncOp::Fetch => s::FETCHING,
            crate::protocol::SyncOp::Pull => s::PULLING,
            crate::protocol::SyncOp::Push => s::PUSHING,
        }
    } else if let Some(note) = cx.state.sync.note.as_deref() {
        note
    } else if cx.state.changes.committing {
        s::COMMITTING
    } else if let Some(note) = cx.state.changes.last_commit_note.as_deref() {
        note
    } else {
        s::READY
    };
    status_bar(ui, &[(&who, Some(260.0)), (activity, None)]);
}
