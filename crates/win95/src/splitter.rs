use egui::{CursorIcon, Id, Rect, Sense, Ui, UiBuilder, pos2, vec2};

use crate::bevel::{self, Bevel};
use crate::theme;

/// Horizontal split: `add_top` above, `add_bottom` below, a draggable 6 px bar in between.
/// `fraction` (0.1..=0.9) is the share of the height given to the top part. `state` is
/// handed to both halves in turn (so they can share mutable state).
pub fn splitter<S>(
    ui: &mut Ui,
    id: impl egui::AsId,
    fraction: &mut f32,
    state: &mut S,
    add_top: impl FnOnce(&mut Ui, &mut S),
    add_bottom: impl FnOnce(&mut Ui, &mut S),
) {
    let id = Id::new(id);
    let rect = ui.available_rect_before_wrap();
    let bar_h = 6.0;
    *fraction = fraction.clamp(0.1, 0.9);
    let top_h = ((rect.height() - bar_h) * *fraction).max(0.0);
    let top = Rect::from_min_size(rect.min, vec2(rect.width(), top_h));
    let bar = Rect::from_min_size(pos2(rect.left(), top.bottom()), vec2(rect.width(), bar_h));
    let bottom = Rect::from_min_max(pos2(rect.left(), bar.bottom()), rect.max);

    let resp = ui
        .interact(bar, id.with("bar"), Sense::drag())
        .on_hover_cursor(CursorIcon::ResizeVertical);
    if resp.dragged() && rect.height() > bar_h {
        *fraction = ((top_h + resp.drag_delta().y) / (rect.height() - bar_h)).clamp(0.1, 0.9);
    }
    ui.painter()
        .rect_filled(bar, 0.0, theme::palette(ui.ctx()).face);
    bevel::paint(ui.painter(), bar.shrink2(vec2(0.0, 1.0)), Bevel::Raised);

    ui.scope_builder(
        UiBuilder::new().max_rect(top).id_salt(id.with("top")),
        |ui| {
            ui.set_clip_rect(top);
            add_top(ui, state);
        },
    );
    ui.scope_builder(
        UiBuilder::new().max_rect(bottom).id_salt(id.with("bottom")),
        |ui| {
            ui.set_clip_rect(bottom);
            add_bottom(ui, state);
        },
    );
    ui.allocate_rect(rect, Sense::hover());
}

#[cfg(test)]
mod tests {
    use egui_kittest::Harness;

    #[test]
    fn dragging_the_bar_changes_the_split() {
        let mut h = Harness::builder()
            .with_size(egui::vec2(300.0, 406.0))
            .build_ui_state(
                |ui, f: &mut f32| {
                    super::splitter(
                        ui,
                        "split",
                        f,
                        &mut (),
                        |ui, _| {
                            ui.label("top");
                        },
                        |ui, _| {
                            ui.label("bottom");
                        },
                    );
                },
                0.5f32,
            );
        h.run();
        // Bar sits at y = 200..206 (half of 400 + margin); drag it down by 100 px.
        let start = egui::pos2(150.0, 208.0);
        h.drag_at(start);
        h.run();
        h.hover_at(start + egui::vec2(0.0, 100.0));
        h.run();
        h.drop_at(start + egui::vec2(0.0, 100.0));
        h.run();
        assert!(*h.state() > 0.6, "fraction {}", h.state());
        assert!(*h.state() <= 0.9);
    }
}
