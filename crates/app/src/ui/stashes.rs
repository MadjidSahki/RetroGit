//! Stashes tab: the list on the left, the selected stash's files and diff on the right.

use egui::{RichText, ScrollArea};
use win95::{Bevel, Button95, bevel_frame};

use super::Ctx;
use crate::format::format_epoch;
use crate::protocol::Command;
use crate::state::GitDialog;
use crate::strings as s;

pub fn show(ui: &mut egui::Ui, cx: &mut Ctx<'_>) {
    if !cx.state.stashes_loaded {
        cx.state.stashes_loaded = true;
        cx.worker.send(Command::LoadStashes);
    }
    ui.horizontal(|ui| {
        let size = egui::vec2(100.0, 22.0);
        if ui
            .add(Button95::new(s::STASH_CHANGES).min_size(size))
            .clicked()
        {
            cx.state.git_dialog = Some(GitDialog::StashSave {
                message: String::new(),
                untracked: false,
            });
        }
        let sel = cx.state.stashes.selected;
        if ui
            .add(
                Button95::new(s::STASH_APPLY)
                    .min_size(size)
                    .enabled(sel.is_some()),
            )
            .clicked()
            && let Some(i) = sel
        {
            cx.worker.send(Command::StashApply(i));
        }
        if ui
            .add(
                Button95::new(s::STASH_POP)
                    .min_size(size)
                    .enabled(sel.is_some()),
            )
            .clicked()
            && let Some(i) = sel
        {
            cx.worker.send(Command::StashPop(i));
        }
        if ui
            .add(
                Button95::new(s::STASH_DROP)
                    .min_size(size)
                    .enabled(sel.is_some()),
            )
            .clicked()
            && let Some(index) = sel
        {
            cx.state.git_dialog = Some(GitDialog::StashDrop { index });
        }
        if ui.add(Button95::new(s::REFRESH).min_size(size)).clicked() {
            cx.worker.send(Command::LoadStashes);
        }
    });
    ui.add_space(2.0);
    egui::Panel::left("stash_list")
        .frame(egui::Frame::NONE)
        .resizable(true)
        .default_size(320.0)
        .show(ui, |ui| list(ui, cx));
    egui::CentralPanel::default()
        .frame(egui::Frame::NONE.inner_margin(egui::Margin {
            left: 4,
            ..Default::default()
        }))
        .show(ui, |ui| detail(ui, cx));
}

fn list(ui: &mut egui::Ui, cx: &mut Ctx<'_>) {
    let mut clicked = None;
    bevel_frame(ui, Bevel::Field, win95::theme::WHITE, 2, |ui| {
        ui.set_min_size(ui.available_size());
        if cx.state.stashes.list.is_empty() {
            ui.label(s::NO_STASHES);
        }
        ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for e in &cx.state.stashes.list {
                    let date = format_epoch(e.time);
                    let text =
                        format!("stash@{{{}}}  {}\n{}  {date}", e.index, e.message, e.branch);
                    if ui
                        .selectable_label(cx.state.stashes.selected == Some(e.index), text)
                        .clicked()
                    {
                        clicked = Some(e.index);
                    }
                }
            });
    });
    if let Some(i) = clicked {
        cx.state.stashes.select(i);
        cx.worker.send(Command::LoadStashFiles(i));
    }
}

fn detail(ui: &mut egui::Ui, cx: &mut Ctx<'_>) {
    let mut open = None;
    {
        let v = &mut cx.state.stashes;
        if let Some(d) = &v.diff
            && v.colors == crate::highlight::Colors::NotRequested
        {
            cx.highlighter
                .request(crate::highlight::Target::History, d.clone());
            v.colors = crate::highlight::Colors::Pending;
        }
    }
    bevel_frame(ui, Bevel::Field, win95::theme::WHITE, 4, |ui| {
        ui.set_min_size(ui.available_size());
        let v = &cx.state.stashes;
        let Some(index) = v.selected else {
            ui.label(s::SELECT_A_STASH);
            return;
        };
        egui::Panel::left("stash_files")
            .frame(egui::Frame::NONE)
            .resizable(true)
            .default_size(220.0)
            .show(ui, |ui| {
                ScrollArea::vertical()
                    .id_salt("stash_files_scroll")
                    .show(ui, |ui| {
                        for f in &v.files {
                            let label = super::changes::describe(&f.path, &f.change);
                            if ui
                                .selectable_label(v.file.as_deref() == Some(f.path.as_str()), label)
                                .clicked()
                            {
                                open = Some(f.path.clone());
                            }
                        }
                    });
            });
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.inner_margin(egui::Margin {
                left: 6,
                ..Default::default()
            }))
            .show(ui, |ui| match &v.diff {
                Some(d) => super::history::diff_rows(ui, d, &v.colors, ("stash", index, &d.path)),
                None if v.file.is_some() => {
                    ui.label(RichText::new(s::LOADING_DIFF).color(win95::theme::GRAY));
                }
                None => {}
            });
        let _ = index;
    });
    if let (Some(path), Some(index)) = (open, cx.state.stashes.selected) {
        let v = &mut cx.state.stashes;
        v.file = Some(path.clone());
        v.diff = None;
        cx.worker.send(Command::LoadStashFileDiff { index, path });
    }
}
