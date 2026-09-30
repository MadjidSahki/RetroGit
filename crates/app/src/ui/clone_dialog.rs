use std::path::PathBuf;

use win95::{Button95, Cell, Column, Dialog, ListView, ProgressBar95, text_field};

use super::Ctx;
use crate::format::format_bytes;
use crate::protocol::Command;
use crate::state::{Auth, CloneDialog, filter_repos};
use crate::strings as s;

const COLUMNS: &[Column] = &[
    Column {
        title: s::COL_NAME,
        width: 190.0,
    },
    Column {
        title: s::COL_OWNER,
        width: 140.0,
    },
    Column {
        title: s::COL_PRIVATE,
        width: 55.0,
    },
    Column {
        title: s::COL_UPDATED,
        width: 85.0,
    },
];

/// Open the clone dialog (or the sign-in dialog if needed) and fetch repos once.
pub fn open(cx: &mut Ctx<'_>) {
    if !matches!(cx.state.auth, Auth::SignedIn(_)) {
        cx.state.sign_in.get_or_insert_with(Default::default);
        return;
    }
    let parent = cx
        .state
        .config
        .last_clone_dir
        .clone()
        .or_else(dirs::home_dir)
        .unwrap_or_default();
    cx.state.clone = Some(CloneDialog {
        dest_parent: parent.display().to_string(),
        ..Default::default()
    });
    if cx.state.repos.is_empty() && !cx.state.repos_loading {
        cx.state.repos_loading = true;
        cx.worker.send(Command::ListRepos);
    }
}

pub fn show(egui_ctx: &egui::Context, cx: &mut Ctx<'_>) {
    let Some(dialog) = cx.state.clone.as_ref() else {
        return;
    };
    if dialog.progress.is_some() {
        progress(egui_ctx, cx);
    } else {
        picker(egui_ctx, cx);
    }
}

fn progress(egui_ctx: &egui::Context, cx: &mut Ctx<'_>) {
    let Some(dialog) = cx.state.clone.as_ref() else {
        return;
    };
    let p = dialog.progress.unwrap_or_default();
    let title = format!("{} {}", s::CLONING_TITLE, dialog.cloning_name);
    let r = Dialog::new("clone_progress", &title)
        .width(340.0)
        .show(egui_ctx, |ui| {
            ui.label(format!(
                "{} {} / {}  ({})",
                s::RECEIVING,
                p.received_objects,
                p.total_objects,
                format_bytes(p.received_bytes)
            ));
            ui.add_space(4.0);
            ui.add(ProgressBar95::new(Some(p.fraction())).width(ui.available_width()));
            ui.add_space(8.0);
            ui.vertical_centered(|ui| ui.add(Button95::new(s::CANCEL)).clicked())
                .inner
        });
    if r.inner || r.close_requested {
        cx.worker.cancel_clone();
    }
}

fn picker(egui_ctx: &egui::Context, cx: &mut Ctx<'_>) {
    let mut start: Option<(String, PathBuf, String)> = None;
    let mut refresh = false;
    let mut close = false;
    let loading = cx.state.repos_loading;
    let repos = &cx.state.repos;
    let Some(dialog) = cx.state.clone.as_mut() else {
        return;
    };

    let r = Dialog::new("clone", s::CLONE_TITLE)
        .width(500.0)
        .show(egui_ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(s::FILTER);
                text_field(ui, &mut dialog.filter, 330.0, false);
                refresh = ui
                    .add(Button95::new(s::REFRESH).enabled(!loading))
                    .clicked();
            });
            ui.add_space(4.0);
            let visible = filter_repos(repos, &dialog.filter);
            let selected_row = visible
                .iter()
                .position(|&i| Some(&repos[i].full_name) == dialog.selected.as_ref());
            let list = ListView::new("clone_list", COLUMNS, visible.len())
                .height(240.0)
                .show(ui, selected_row, |row, col| {
                    let r = &repos[visible[row]];
                    match col {
                        0 => Cell::from(r.name.as_str()),
                        1 => Cell::from(r.owner.as_str()),
                        2 => Cell::from(if r.private { s::YES } else { "" }),
                        _ => Cell::from(r.updated_at.get(..10).unwrap_or("")),
                    }
                });
            if loading {
                ui.label(s::LOADING);
            }
            if let Some(row) = list.clicked.or(list.double_clicked) {
                dialog.selected = Some(repos[visible[row]].full_name.clone());
            }
            ui.add_space(6.0);
            ui.label(s::DEST_FOLDER);
            ui.horizontal(|ui| {
                text_field(ui, &mut dialog.dest_parent, 380.0, false);
                if ui.add(Button95::new(s::BROWSE)).clicked() {
                    let mut picker = rfd::FileDialog::new();
                    if !dialog.dest_parent.is_empty() {
                        picker = picker.set_directory(&dialog.dest_parent);
                    }
                    if let Some(folder) = picker.pick_folder() {
                        dialog.dest_parent = folder.display().to_string();
                    }
                }
            });
            let chosen = dialog
                .selected
                .as_ref()
                .and_then(|f| repos.iter().find(|r| &r.full_name == f));
            let parent = dialog.dest_parent.trim();
            if let Some(repo) = chosen
                && !parent.is_empty()
            {
                let dest = PathBuf::from(parent).join(&repo.name);
                ui.label(format!("{} {}", s::WILL_CLONE_INTO, dest.display()));
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let can_clone = chosen.is_some() && !parent.is_empty();
                let clicked = ui.add(Button95::new(s::CLONE).enabled(can_clone)).clicked();
                let double = list.double_clicked.is_some() && !parent.is_empty();
                if (clicked || double)
                    && let Some(repo) = dialog
                        .selected
                        .as_ref()
                        .and_then(|f| repos.iter().find(|r| &r.full_name == f))
                {
                    start = Some((
                        repo.clone_url.clone(),
                        PathBuf::from(parent).join(&repo.name),
                        repo.name.clone(),
                    ));
                }
                close = ui.add(Button95::new(s::CANCEL)).clicked();
            });
        });

    if let Some((url, dest, name)) = start {
        let parent = dialog.dest_parent.trim().to_string();
        dialog.progress = Some(Default::default());
        dialog.cloning_name = name;
        cx.state.config.last_clone_dir = Some(PathBuf::from(parent));
        cx.state.config_dirty = true;
        cx.worker.send(Command::Clone { url, dest });
    } else if r.close_requested || close {
        cx.state.clone = None;
    }
    if refresh {
        cx.state.repos_loading = true;
        cx.worker.send(Command::ListRepos);
    }
}
