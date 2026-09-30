use egui::{CursorIcon, Rect, ResizeDirection, Sense, Ui, ViewportCommand, pos2};

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
