use win95::{Button95, Dialog};

use super::Ctx;
use crate::strings as s;

pub fn show(egui_ctx: &egui::Context, cx: &mut Ctx<'_>) {
    if !cx.state.about {
        return;
    }
    let r = Dialog::new("about", s::ABOUT_TITLE)
        .width(440.0)
        .show(egui_ctx, |ui| {
            ui.horizontal(|ui| {
                let logo = super::logo::logo_texture(ui.ctx());
                ui.add(egui::Image::new(&logo).fit_to_exact_size(egui::vec2(96.0, 96.0)));
                ui.vertical(|ui| {
                    ui.label(format!("{} {}", s::APP_NAME, env!("CARGO_PKG_VERSION")));
                    ui.label(s::ABOUT_TAGLINE);
                    ui.add(egui::Label::new(s::ABOUT_FONT).wrap());
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
