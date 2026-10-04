//! The update window: what is new, and Install and restart / Download.

use egui::ScrollArea;
use win95::{Bevel, Button95, Dialog, bevel_frame, checkbox, markdown_view};

use super::Ctx;
use crate::strings as s;
use crate::update::installable;

const BUTTON: egui::Vec2 = egui::vec2(130.0, 23.0);

/// "Update available (x.y.z)" in the toolbar, when there is one.
pub fn toolbar_button(ui: &mut egui::Ui, cx: &mut Ctx<'_>) {
    let Some(r) = &cx.state.update.available else {
        return;
    };
    let label = s::UPDATE_AVAILABLE.replace("{version}", &r.version);
    if ui
        .add(Button95::new(label).min_size(egui::vec2(60.0, 22.0)))
        .clicked()
    {
        cx.state.update.open = true;
    }
}

pub fn show(egui_ctx: &egui::Context, cx: &mut Ctx<'_>) {
    if !cx.state.update.open {
        return;
    }
    let Some(r) = cx.state.update.available.clone() else {
        cx.state.update.open = false;
        return;
    };
    let can_install = cx.state.update.can_replace && installable(&cx.state.update.kind, &r);
    let mut auto = cx.state.config.updates.check;
    let (mut install, mut download, mut later, mut skip) = (false, false, false, false);
    let mut cancel = false;
    let title = s::UPDATE_TITLE.replace("{version}", &r.version);
    let resp = Dialog::new("update", &title)
        .width(520.0)
        .show(egui_ctx, |ui| {
            ui.label(
                s::UPDATE_INTRO
                    .replace("{version}", &r.version)
                    .replace("{current}", crate::version::version()),
            );
            ui.add_space(4.0);
            bevel_frame(
                ui,
                Bevel::Field,
                win95::theme::palette(ui.ctx()).window,
                4,
                |ui| {
                    ui.set_width(ui.available_width());
                    ScrollArea::vertical()
                        .id_salt("update_notes")
                        .max_height(260.0)
                        .show(ui, |ui| markdown_view(ui, &r.notes));
                },
            );
            if !can_install {
                ui.add(egui::Label::new(s::DOWNLOAD_HELP).wrap());
            }
            checkbox(ui, &mut auto, s::CHECK_AUTOMATICALLY);
            let u = &cx.state.update;
            if let Some(err) = &u.error {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(s::UPDATE_FAILED.replace("{why}", err))
                            .color(win95::theme::palette(ui.ctx()).error),
                    )
                    .wrap(),
                );
            }
            if u.waiting {
                ui.label(s::UPDATE_WAITS);
            }
            if let Some(note) = &u.notice {
                ui.label(note);
            }
            if u.installing {
                if u.downloaded {
                    ui.label(s::UPDATE_INSTALLING);
                    return;
                }
                let (done, total) = u.progress.unwrap_or((0, None));
                let mb = |b: u64| format!("{:.1}", b as f64 / (1024.0 * 1024.0));
                let text = match total {
                    Some(t) => s::UPDATE_SIZE
                        .replace("{done}", &mb(done))
                        .replace("{total}", &mb(t)),
                    None => s::UPDATE_SIZE_UNKNOWN.replace("{done}", &mb(done)),
                };
                ui.add(win95::ProgressBar95::new(
                    total.map(|t| done as f32 / t.max(1) as f32),
                ));
                ui.label(s::UPDATE_DOWNLOADING.replace("{progress}", &text));
                ui.add_space(6.0);
                if ui.add(Button95::new(s::CANCEL).min_size(BUTTON)).clicked() {
                    cancel = true;
                }
                return;
            }
            if cx.state.update.relaunch.is_some() {
                ui.label(s::UPDATE_WAITS);
                return;
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if can_install {
                    install = ui
                        .add(Button95::new(s::INSTALL_AND_RESTART).min_size(BUTTON))
                        .clicked();
                } else {
                    download = ui
                        .add(Button95::new(s::DOWNLOAD_UPDATE).min_size(BUTTON))
                        .clicked();
                }
                later = ui.add(Button95::new(s::LATER).min_size(BUTTON)).clicked();
                skip = ui
                    .add(Button95::new(s::SKIP_VERSION).min_size(BUTTON))
                    .clicked();
            });
        });
    cx.state.set_update_checks(auto);
    if download {
        egui_ctx.open_url(egui::OpenUrl::new_tab(&r.url));
        cx.state.update.open = false;
    }
    if install {
        cx.state.update.install_requested = true;
    }
    if cancel {
        cx.state.update.cancel_requested = true;
    }
    if later || (resp.close_requested && !cx.state.update.installing) {
        cx.state.close_update_window();
    }
    if skip {
        cx.state.skip_update();
    }
}
