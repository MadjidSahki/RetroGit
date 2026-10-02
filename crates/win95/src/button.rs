use egui::{Align2, Response, Sense, Ui, Vec2, Widget, WidgetInfo, WidgetType, vec2};

use crate::bevel::{self, Bevel};
use crate::theme;

/// Classic push button: 75×23 minimum, pressed look while held, embossed text when disabled.
pub struct Button95 {
    text: String,
    enabled: bool,
    min_size: Vec2,
}

impl Button95 {
    pub fn new(text: impl Into<String>) -> Button95 {
        Button95 {
            text: text.into(),
            enabled: true,
            min_size: vec2(75.0, 23.0),
        }
    }

    pub fn enabled(mut self, enabled: bool) -> Button95 {
        self.enabled = enabled;
        self
    }

    pub fn min_size(mut self, size: Vec2) -> Button95 {
        self.min_size = size;
        self
    }
}

impl Widget for Button95 {
    fn ui(self, ui: &mut Ui) -> Response {
        let pal = theme::palette(ui.ctx());
        let font = theme::font(theme::FONT_SIZE);
        let galley = ui
            .painter()
            .layout_no_wrap(self.text.clone(), font.clone(), pal.text);
        let size = (galley.size() + vec2(16.0, 8.0)).max(self.min_size);
        let sense = if self.enabled {
            Sense::click()
        } else {
            Sense::hover()
        };
        let (rect, response) = ui.allocate_exact_size(size, sense);
        let (enabled, text) = (self.enabled, self.text.clone());
        response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, enabled, &text));

        if ui.is_rect_visible(rect) {
            let pressed = self.enabled && response.is_pointer_button_down_on();
            let p = ui.painter();
            p.rect_filled(rect, 0.0, pal.face);
            bevel::paint(
                p,
                rect,
                if pressed {
                    Bevel::Pressed
                } else {
                    Bevel::Raised
                },
            );
            let center = rect.center() + if pressed { vec2(1.0, 1.0) } else { Vec2::ZERO };
            if self.enabled {
                p.text(center, Align2::CENTER_CENTER, &self.text, font, pal.text);
            } else {
                // Win95 embosses disabled text with the highlight color, when it is lighter
                // than the face (not in High Contrast White, where it is black).
                if crate::palette::luminance(pal.highlight) > crate::palette::luminance(pal.face) {
                    p.text(
                        center + vec2(1.0, 1.0),
                        Align2::CENTER_CENTER,
                        &self.text,
                        font.clone(),
                        pal.highlight,
                    );
                }
                p.text(
                    center,
                    Align2::CENTER_CENTER,
                    &self.text,
                    font,
                    pal.gray_text,
                );
            }
        }
        response
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    use super::Button95;

    #[test]
    fn enabled_button_reports_click() {
        let mut h = Harness::new_ui_state(
            |ui, clicks: &mut u32| {
                if ui.add(Button95::new("OK")).clicked() {
                    *clicks += 1;
                }
            },
            0,
        );
        h.get_by_label("OK").click();
        h.run();
        assert_eq!(*h.state(), 1);
    }

    #[test]
    fn disabled_button_ignores_clicks() {
        let mut h = Harness::new_ui_state(
            |ui, clicks: &mut u32| {
                if ui.add(Button95::new("Push").enabled(false)).clicked() {
                    *clicks += 1;
                }
            },
            0,
        );
        h.get_by_label("Push").click();
        h.run();
        assert_eq!(*h.state(), 0);
    }
}
