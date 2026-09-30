use egui::{Context, Id, Key, Modal};

use crate::bevel::Bevel;
use crate::panel::bevel_frame;
use crate::theme::SILVER;
use crate::title_bar::{TitleAction, TitleBar};

/// A modal Win95 dialog drawn inside the main window.
pub struct Dialog<'a> {
    id: Id,
    title: &'a str,
    width: f32,
}

pub struct DialogResponse<R> {
    pub inner: R,
    /// Close button or Escape.
    pub close_requested: bool,
}

impl<'a> Dialog<'a> {
    pub fn new(id: impl egui::AsId, title: &'a str) -> Dialog<'a> {
        Dialog {
            id: Id::new(id),
            title,
            width: 360.0,
        }
    }

    pub fn width(mut self, width: f32) -> Self {
        self.width = width;
        self
    }

    pub fn show<R>(self, ctx: &Context, add: impl FnOnce(&mut egui::Ui) -> R) -> DialogResponse<R> {
        let mut close = false;
        let resp = Modal::new(self.id)
            .backdrop_color(egui::Color32::TRANSPARENT)
            .frame(egui::Frame::NONE)
            .show(ctx, |ui| {
                bevel_frame(ui, Bevel::Window, SILVER, 1, |ui| {
                    ui.set_width(self.width);
                    if TitleBar::new(self.title).close_only().show(ui) == TitleAction::Close {
                        close = true;
                    }
                    egui::Frame::NONE
                        .inner_margin(egui::Margin::same(8))
                        .show(ui, add)
                        .inner
                })
                .inner
            });
        if ctx.input(|i| i.key_pressed(Key::Escape)) {
            close = true;
        }
        DialogResponse {
            inner: resp.inner,
            close_requested: close,
        }
    }
}

#[cfg(test)]
mod tests {
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    use super::Dialog;

    #[test]
    fn close_button_requests_close_and_content_is_shown() {
        let mut h = Harness::new_ui_state(
            |ui, closed: &mut bool| {
                let ctx = ui.ctx().clone();
                let r = Dialog::new("about", "About RetroGit").show(&ctx, |ui| {
                    ui.label("RetroGit 0.1.0");
                });
                if r.close_requested {
                    *closed = true;
                }
            },
            false,
        );
        h.run();
        assert!(h.query_by_label("RetroGit 0.1.0").is_some());
        h.get_by_label("Close").click();
        h.run();
        assert!(*h.state());
    }

    #[test]
    fn escape_requests_close() {
        let mut h = Harness::new_ui_state(
            |ui, closed: &mut bool| {
                let ctx = ui.ctx().clone();
                if Dialog::new("d", "D")
                    .show(&ctx, |ui| ui.label("x"))
                    .close_requested
                {
                    *closed = true;
                }
            },
            false,
        );
        h.run();
        h.key_press(egui::Key::Escape);
        h.run();
        assert!(*h.state());
    }
}
