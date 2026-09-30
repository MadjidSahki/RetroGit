use egui::{Color32, InnerResponse, Margin, Ui};

use crate::bevel::{self, Bevel};

/// Lay out `add` inside a filled, bevelled box.
/// `margin` is the space between the bevel and the content (the bevel is added on top).
pub fn bevel_frame<R>(
    ui: &mut Ui,
    kind: Bevel,
    fill: Color32,
    margin: i8,
    add: impl FnOnce(&mut Ui) -> R,
) -> InnerResponse<R> {
    let edge = bevel::thickness(kind) as i8;
    let inner = egui::Frame::NONE
        .fill(fill)
        .inner_margin(Margin::same(margin + edge))
        .show(ui, add);
    bevel::paint(ui.painter(), inner.response.rect, kind);
    inner
}
