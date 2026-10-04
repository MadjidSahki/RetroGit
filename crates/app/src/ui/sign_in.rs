use egui::RichText;
use win95::{Bevel, Button95, Dialog, ProgressBar95, bevel_frame, tabs, text_field};

use super::Ctx;
use crate::protocol::Command;
use crate::state::Auth;
use crate::strings as s;

pub fn show(egui_ctx: &egui::Context, cx: &mut Ctx<'_>) {
    if cx.state.sign_in.is_none() {
        return;
    }
    let mut close = false;
    let r = Dialog::new("sign_in", s::SIGN_IN_TITLE)
        .width(380.0)
        .show(egui_ctx, |ui| {
            let Some(dialog) = cx.state.sign_in.as_mut() else {
                return;
            };
            tabs(ui, &mut dialog.tab, &[s::TAB_STANDARD, s::TAB_ADVANCED]);
            let tab = dialog.tab;
            bevel_frame(
                ui,
                Bevel::Window,
                win95::theme::palette(ui.ctx()).face,
                8,
                |ui| {
                    ui.set_min_height(150.0);
                    ui.set_width(ui.available_width());
                    if tab == 0 {
                        standard_tab(ui, cx, &mut close);
                    } else {
                        advanced_tab(ui, cx);
                    }
                },
            );
        });
    if r.close_requested || close {
        cx.worker.cancel_device_flow();
        cx.state.sign_in = None;
    }
}

fn standard_tab(ui: &mut egui::Ui, cx: &mut Ctx<'_>, close: &mut bool) {
    match cx.state.auth.clone() {
        Auth::Waiting {
            user_code,
            verification_uri,
        } => {
            ui.label(s::DEVICE_GO_TO);
            ui.hyperlink(&verification_uri);
            ui.label(s::DEVICE_ENTER_CODE);
            ui.vertical_centered(|ui| {
                ui.label(
                    RichText::new(&user_code)
                        .font(win95::theme::font(26.0))
                        .color(win95::theme::palette(ui.ctx()).text),
                );
            });
            ui.horizontal(|ui| {
                if ui
                    .add(Button95::new(s::COPY_CODE).min_size(egui::vec2(90.0, 23.0)))
                    .clicked()
                {
                    ui.ctx().copy_text(user_code.clone());
                }
                if ui
                    .add(Button95::new(s::OPEN_BROWSER).min_size(egui::vec2(100.0, 23.0)))
                    .clicked()
                {
                    ui.ctx().open_url(egui::OpenUrl::new_tab(&verification_uri));
                }
            });
            ui.add_space(6.0);
            ui.label(s::WAITING_AUTH);
            ui.add(ProgressBar95::new(None).width(ui.available_width()));
            ui.add_space(6.0);
            if ui.add(Button95::new(s::CANCEL)).clicked() {
                cx.worker.cancel_device_flow();
                *close = true;
            }
        }
        auth => {
            ui.add(egui::Label::new(s::SIGN_IN_INTRO).wrap());
            ui.add_space(12.0);
            let busy = matches!(auth, Auth::Starting | Auth::Checking);
            if ui
                .add(Button95::new(s::SIGN_IN_BUTTON).enabled(!busy))
                .clicked()
            {
                cx.state.auth = Auth::Starting;
                cx.worker.send(Command::StartDeviceFlow);
            }
        }
    }
}

fn advanced_tab(ui: &mut egui::Ui, cx: &mut Ctx<'_>) {
    let Some(dialog) = cx.state.sign_in.as_mut() else {
        return;
    };
    ui.label(s::PAT_LABEL);
    let resp = text_field(ui, &mut dialog.pat, ui.available_width() - 8.0, true);
    ui.add(
        egui::Label::new(
            RichText::new(s::PAT_HELP).color(win95::theme::palette(ui.ctx()).gray_text),
        )
        .wrap(),
    );
    ui.add_space(8.0);
    let can_submit = !dialog.pat.trim().is_empty() && !dialog.pat_submitted;
    let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
    if (ui.add(Button95::new(s::OK).enabled(can_submit)).clicked() || enter) && can_submit {
        dialog.pat_submitted = true;
        let pat = std::mem::take(&mut dialog.pat);
        cx.worker.send(Command::SavePat(pat));
    }
}
