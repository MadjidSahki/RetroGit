use egui::{Align2, Rect, Ui, pos2, vec2};

use crate::bevel::{self, Bevel};
use crate::theme::{self, BLACK};

/// Status bar made of sunken cells. `None` width = take the remaining space.
pub fn status_bar(ui: &mut Ui, cells: &[(&str, Option<f32>)]) {
    let height = 20.0;
    let (rect, _) =
        ui.allocate_exact_size(vec2(ui.available_width(), height), egui::Sense::hover());
    let fixed: f32 = cells.iter().filter_map(|c| c.1).sum();
    let flexible = cells.iter().filter(|c| c.1.is_none()).count().max(1) as f32;
    let gap = 2.0;
    let spare =
        (rect.width() - fixed - gap * (cells.len().saturating_sub(1)) as f32).max(0.0) / flexible;
    let mut x = rect.left();
    for (text, width) in cells {
        let w = width.unwrap_or(spare);
        let cell = Rect::from_min_size(pos2(x, rect.top() + 2.0), vec2(w, height - 2.0));
        bevel::paint(ui.painter(), cell, Bevel::Shallow);
        ui.painter().with_clip_rect(cell.shrink(2.0)).text(
            cell.left_center() + vec2(4.0, 0.0),
            Align2::LEFT_CENTER,
            *text,
            theme::font(theme::FONT_SIZE),
            BLACK,
        );
        x += w + gap;
    }
}
