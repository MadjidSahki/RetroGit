use egui::{Align2, Rect, Sense, Ui, WidgetInfo, WidgetType, pos2, vec2};

use crate::theme;

/// Row of Win95 tabs. Returns `true` if the selection changed.
/// Draw the tab page right below with `bevel_frame(ui, Bevel::Window, ...)`.
pub fn tabs(ui: &mut Ui, selected: &mut usize, labels: &[&str]) -> bool {
    let mut changed = false;
    let font = theme::font(theme::FONT_SIZE);
    let pal = theme::palette(ui.ctx());
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        for (i, label) in labels.iter().enumerate() {
            let galley = ui
                .painter()
                .layout_no_wrap(label.to_string(), font.clone(), pal.text);
            let size = vec2(galley.size().x + 16.0, 20.0);
            let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
            let is_sel = *selected == i;
            let owned = label.to_string();
            resp.widget_info(|| {
                WidgetInfo::selected(WidgetType::SelectableLabel, true, is_sel, &owned)
            });
            if resp.clicked() && !is_sel {
                *selected = i;
                changed = true;
            }
            // Selected tab is 2px taller and merges with the page below.
            let r = if is_sel {
                rect
            } else {
                Rect::from_min_max(pos2(rect.left(), rect.top() + 2.0), rect.max)
            };
            let p = ui.painter();
            p.rect_filled(r, 0.0, pal.face);
            p.rect_filled(
                Rect::from_min_size(r.min, vec2(r.width(), 1.0)),
                0.0,
                pal.highlight,
            );
            p.rect_filled(
                Rect::from_min_size(r.min, vec2(1.0, r.height())),
                0.0,
                pal.highlight,
            );
            p.rect_filled(
                Rect::from_min_size(pos2(r.max.x - 1.0, r.min.y), vec2(1.0, r.height())),
                0.0,
                pal.dark_shadow,
            );
            p.rect_filled(
                Rect::from_min_size(
                    pos2(r.max.x - 2.0, r.min.y + 1.0),
                    vec2(1.0, r.height() - 1.0),
                ),
                0.0,
                pal.shadow,
            );
            if !is_sel {
                p.rect_filled(
                    Rect::from_min_size(pos2(r.min.x, r.max.y - 1.0), vec2(r.width(), 1.0)),
                    0.0,
                    pal.light,
                );
            }
            p.text(
                r.center(),
                Align2::CENTER_CENTER,
                *label,
                font.clone(),
                pal.text,
            );
        }
    });
    changed
}

#[cfg(test)]
mod tests {
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    #[test]
    fn clicking_a_tab_selects_it() {
        let mut h = Harness::new_ui_state(
            |ui, sel: &mut usize| {
                super::tabs(ui, sel, &["Standard", "Advanced"]);
            },
            0usize,
        );
        h.get_by_label("Advanced").click();
        h.run();
        assert_eq!(*h.state(), 1);
    }
}
