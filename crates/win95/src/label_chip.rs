use egui::{Color32, Response, Sense, Stroke, Ui, WidgetInfo, WidgetType, vec2};

use crate::theme;

/// Black or white, whichever reads better on `bg` (perceived luminance).
pub fn text_color_on(bg: [u8; 3]) -> Color32 {
    let [r, g, b] = bg.map(f32::from);
    let luminance = 0.299 * r + 0.587 * g + 0.114 * b;
    if luminance > 150.0 {
        theme::BLACK
    } else {
        theme::WHITE
    }
}

/// A GitHub label: its name on its color, with a thin darker border.
pub fn label_chip(ui: &mut Ui, text: &str, color: [u8; 3]) -> Response {
    let font = theme::font(theme::FONT_SIZE);
    let fg = text_color_on(color);
    let galley = ui.painter().layout_no_wrap(text.to_string(), font, fg);
    let size = vec2(galley.size().x + 10.0, 16.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let owned = text.to_string();
    resp.widget_info(|| WidgetInfo::labeled(WidgetType::Label, true, &owned));
    let bg = Color32::from_rgb(color[0], color[1], color[2]);
    let p = ui.painter();
    p.rect_filled(rect, 3.0, bg);
    p.rect_stroke(
        rect,
        3.0,
        Stroke::new(1.0, bg.gamma_multiply(0.7)),
        egui::StrokeKind::Inside,
    );
    p.galley(rect.center() - galley.size() / 2.0, galley, fg);
    resp
}

#[cfg(test)]
mod tests {
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    use super::*;

    #[test]
    fn text_is_readable_on_light_and_dark_labels() {
        assert_eq!(
            text_color_on([0xa2, 0xee, 0xef]),
            theme::BLACK,
            "enhancement"
        );
        assert_eq!(text_color_on([0xcf, 0xd3, 0xd7]), theme::BLACK, "duplicate");
        assert_eq!(text_color_on([0xd7, 0x3a, 0x4a]), theme::WHITE, "bug");
        assert_eq!(
            text_color_on([0x00, 0x75, 0xca]),
            theme::WHITE,
            "documentation"
        );
    }

    #[test]
    fn label_chip_is_accessible_by_its_text() {
        let mut h = Harness::new_ui_state(
            |ui, clicks: &mut u32| {
                if label_chip(ui, "good first issue", [0x70, 0x57, 0xff]).clicked() {
                    *clicks += 1;
                }
            },
            0,
        );
        h.get_by_label("good first issue").click();
        h.run();
        assert_eq!(*h.state(), 1);
    }
}
