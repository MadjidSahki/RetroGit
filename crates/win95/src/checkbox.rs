use egui::{Align2, Rect, Response, Sense, Stroke, Ui, WidgetInfo, WidgetType, pos2, vec2};

use crate::bevel::{self, Bevel};
use crate::theme::{self, BLACK, GRAY, WHITE};

/// Win95 check box: 13×13 white well with a black tick, label on the right.
/// An empty `label` draws the box alone (used on diff lines).
pub fn checkbox(ui: &mut Ui, checked: &mut bool, label: &str) -> Response {
    let font = theme::font(theme::FONT_SIZE);
    let text_w = if label.is_empty() {
        0.0
    } else {
        ui.painter()
            .layout_no_wrap(label.to_string(), font.clone(), BLACK)
            .size()
            .x
            + 6.0
    };
    let (rect, mut resp) = ui.allocate_exact_size(vec2(13.0 + text_w, 16.0), Sense::click());
    if resp.clicked() {
        *checked = !*checked;
        resp.mark_changed();
    }
    let value = *checked;
    let owned = label.to_string();
    resp.widget_info(|| WidgetInfo::selected(WidgetType::Checkbox, true, value, &owned));
    let bx = Rect::from_min_size(pos2(rect.left(), rect.center().y - 6.5), vec2(13.0, 13.0));
    let p = ui.painter();
    p.rect_filled(bx, 0.0, WHITE);
    bevel::paint(p, bx, Bevel::Field);
    if value {
        let s = Stroke::new(2.0, BLACK);
        let (a, b, c) = (
            bx.left_top() + vec2(3.0, 6.0),
            bx.left_top() + vec2(5.5, 9.0),
            bx.left_top() + vec2(10.0, 3.5),
        );
        p.line_segment([a, b], s);
        p.line_segment([b, c], s);
    }
    if !label.is_empty() {
        p.text(
            pos2(bx.right() + 5.0, rect.center().y),
            Align2::LEFT_CENTER,
            label,
            font,
            BLACK,
        );
    }
    if resp.has_focus() {
        p.rect_stroke(
            rect.expand(1.0),
            0.0,
            Stroke::new(1.0, GRAY),
            egui::StrokeKind::Outside,
        );
    }
    resp
}

#[cfg(test)]
mod tests {
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    #[test]
    fn clicking_toggles_and_reports_state() {
        let mut h = Harness::new_ui_state(
            |ui, on: &mut bool| {
                super::checkbox(ui, on, "Amend last commit");
            },
            false,
        );
        h.get_by_label("Amend last commit").click();
        h.run();
        assert!(*h.state());
        h.get_by_label("Amend last commit").click();
        h.run();
        assert!(!*h.state());
    }
}
