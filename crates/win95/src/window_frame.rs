use egui::{CursorIcon, Rect, ResizeDirection, Sense, Ui, ViewportCommand, pos2, vec2};

/// Reserve a full-width strip of `height` in `ui` and draw `add` in a layer above modal
/// dialogs, so the main window's own chrome (title bar) keeps working while a dialog is open.
pub fn above_dialogs<R>(
    ui: &mut Ui,
    id: impl egui::AsId,
    height: f32,
    add: impl FnOnce(&mut Ui) -> R,
) -> R {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
    egui::Area::new(egui::Id::new(id))
        .order(egui::Order::Tooltip)
        .fixed_pos(rect.min)
        .show(ui.ctx(), |ui| {
            ui.set_width(rect.width());
            ui.set_max_width(rect.width());
            add(ui)
        })
        .inner
}

/// Invisible resize handles along the edges of `rect` for an undecorated window.
pub fn resize_edges(ui: &mut Ui, rect: Rect) {
    let t = 4.0;
    let (l, r, top, b) = (rect.left(), rect.right(), rect.top(), rect.bottom());
    let zones = [
        (
            Rect::from_min_max(pos2(l, top), pos2(l + t, top + t)),
            ResizeDirection::NorthWest,
            CursorIcon::ResizeNorthWest,
        ),
        (
            Rect::from_min_max(pos2(r - t, top), pos2(r, top + t)),
            ResizeDirection::NorthEast,
            CursorIcon::ResizeNorthEast,
        ),
        (
            Rect::from_min_max(pos2(l, b - t), pos2(l + t, b)),
            ResizeDirection::SouthWest,
            CursorIcon::ResizeSouthWest,
        ),
        (
            Rect::from_min_max(pos2(r - t, b - t), pos2(r, b)),
            ResizeDirection::SouthEast,
            CursorIcon::ResizeSouthEast,
        ),
        (
            Rect::from_min_max(pos2(l + t, top), pos2(r - t, top + t)),
            ResizeDirection::North,
            CursorIcon::ResizeNorth,
        ),
        (
            Rect::from_min_max(pos2(l + t, b - t), pos2(r - t, b)),
            ResizeDirection::South,
            CursorIcon::ResizeSouth,
        ),
        (
            Rect::from_min_max(pos2(l, top + t), pos2(l + t, b - t)),
            ResizeDirection::West,
            CursorIcon::ResizeWest,
        ),
        (
            Rect::from_min_max(pos2(r - t, top + t), pos2(r, b - t)),
            ResizeDirection::East,
            CursorIcon::ResizeEast,
        ),
    ];
    for (i, (zone, dir, cursor)) in zones.into_iter().enumerate() {
        let resp = ui
            .interact(zone, ui.id().with(("resize", i)), Sense::drag())
            .on_hover_cursor(cursor);
        if resp.drag_started() {
            ui.ctx()
                .send_viewport_cmd(ViewportCommand::BeginResize(dir));
        }
    }
}

#[cfg(test)]
mod tests {
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    use crate::{Dialog, TitleAction, TitleBar};

    #[test]
    fn chrome_above_dialogs_stays_clickable_while_a_dialog_is_open() {
        let mut h = Harness::new_ui_state(
            |ui, got: &mut TitleAction| {
                let a = super::above_dialogs(ui, "main_title", crate::title_bar::HEIGHT, |ui| {
                    TitleBar::new("Main").show(ui)
                });
                if a != TitleAction::None {
                    *got = a;
                }
                let ctx = ui.ctx().clone();
                Dialog::new("d", "Dialog").show(&ctx, |ui| ui.label("x"));
            },
            TitleAction::None,
        );
        h.run();
        h.get_by_label("Minimize").click();
        h.run();
        assert_eq!(*h.state(), TitleAction::Minimize);
    }
}
