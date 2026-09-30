use egui::{
    Align2, Id, Popup, Rect, Response, Sense, Stroke, Ui, WidgetInfo, WidgetType, pos2, vec2,
};

use crate::bevel::{self, Bevel};
use crate::theme::{self, BLACK, SILVER, WHITE};

/// Win95 drop-down list: white sunken field with the current value and an arrow button.
/// `add_items` fills the popup; clicking an item closes it.
pub fn combo_box(
    ui: &mut Ui,
    id: impl egui::AsId,
    selected_text: &str,
    width: f32,
    add_items: impl FnOnce(&mut Ui),
) -> Response {
    let id = Id::new(id);
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 21.0), Sense::click());
    let owned = selected_text.to_string();
    resp.widget_info(|| WidgetInfo::labeled(WidgetType::ComboBox, true, &owned));
    let p = ui.painter();
    p.rect_filled(rect, 0.0, WHITE);
    bevel::paint(p, rect, Bevel::Field);
    let arrow = Rect::from_min_max(
        pos2(rect.right() - 18.0, rect.top() + 2.0),
        rect.max - vec2(2.0, 2.0),
    );
    p.rect_filled(arrow, 0.0, SILVER);
    bevel::paint(p, arrow, Bevel::Raised);
    let c = arrow.center();
    p.add(egui::Shape::convex_polygon(
        vec![
            c + vec2(-4.0, -2.0),
            c + vec2(4.0, -2.0),
            c + vec2(0.0, 2.0),
        ],
        BLACK,
        Stroke::NONE,
    ));
    p.with_clip_rect(Rect::from_min_max(
        rect.min,
        pos2(arrow.left() - 2.0, rect.bottom()),
    ))
    .text(
        rect.left_center() + vec2(5.0, 0.0),
        Align2::LEFT_CENTER,
        selected_text,
        theme::font(theme::FONT_SIZE),
        BLACK,
    );
    Popup::from_toggle_button_response(&resp)
        .id(id.with("popup"))
        .width(width)
        .show(|ui| {
            ui.set_min_width(width - 8.0);
            add_items(ui);
        });
    resp
}

#[cfg(test)]
mod tests {
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    #[test]
    fn opening_and_picking_an_item() {
        let mut h = Harness::new_ui_state(
            |ui, picked: &mut String| {
                super::combo_box(ui, "branches", "main", 180.0, |ui| {
                    for name in ["main", "feature"] {
                        if ui.button(name).clicked() {
                            *picked = name.to_string();
                        }
                    }
                });
            },
            String::new(),
        );
        h.run();
        assert!(h.query_by_label("feature").is_none(), "closed at first");
        h.get_by_role(egui::accesskit::Role::ComboBox).click();
        h.run();
        h.get_by_label("feature").click();
        h.run();
        assert_eq!(h.state(), "feature");
    }
}
