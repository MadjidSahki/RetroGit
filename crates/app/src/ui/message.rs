use win95::{Button95, Dialog, Icon};

use super::Ctx;
use crate::protocol::Severity;
use crate::strings as s;

/// Show the oldest queued message box, if any.
pub fn show(egui_ctx: &egui::Context, cx: &mut Ctx<'_>) {
    let Some(msg) = cx.state.messages.front().cloned() else {
        return;
    };
    let icon = match msg.severity {
        Severity::Error => Icon::Error,
        Severity::Warning => Icon::Warning,
        Severity::Info => Icon::Info,
    };
    let r = Dialog::new("message", s::ERR_TITLE)
        .width(380.0)
        .show(egui_ctx, |ui| {
            ui.horizontal(|ui| {
                win95::icon::icon(ui, icon);
                ui.vertical(|ui| {
                    ui.add(egui::Label::new(&msg.message).wrap());
                    if let Some(link) = &msg.link {
                        ui.hyperlink(link);
                    }
                    if let Some(detail) = &msg.detail {
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(detail)
                                    .color(win95::theme::palette(ui.ctx()).gray_text),
                            )
                            .wrap(),
                        );
                    }
                });
            });
            ui.add_space(8.0);
            ui.vertical_centered(|ui| ui.add(Button95::new(s::OK)).clicked())
                .inner
        });
    if r.inner || r.close_requested {
        cx.state.messages.pop_front();
    }
}
