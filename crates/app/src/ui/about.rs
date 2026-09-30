use win95::{Button95, Dialog, Icon};

use super::Ctx;
use crate::strings as s;

pub fn show(egui_ctx: &egui::Context, cx: &mut Ctx<'_>) {
    if !cx.state.about {
        return;
    }
    let r = Dialog::new("about", s::ABOUT_TITLE)
        .width(320.0)
        .show(egui_ctx, |ui| {
            ui.horizontal(|ui| {
                win95::icon::icon(ui, Icon::Info);
                ui.vertical(|ui| {
                    ui.label(format!("{} {}", s::APP_NAME, env!("CARGO_PKG_VERSION")));
                    ui.label(s::ABOUT_TAGLINE);
                    ui.label(s::ABOUT_FONT);
                });
            });
            ui.add_space(8.0);
            ui.vertical_centered(|ui| ui.add(Button95::new(s::OK)).clicked())
                .inner
        });
    if r.inner || r.close_requested {
        cx.state.about = false;
    }
}
