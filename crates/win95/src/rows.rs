//! Scrolled lists whose rows touch (diffs, code, lists): egui's `show_rows` counts the item
//! spacing between rows, so with none it showed too few rows (a blank band at the bottom)
//! and scrolled past the end.

use std::ops::Range;

use egui::scroll_area::ScrollAreaOutput;
use egui::{ScrollArea, Ui};

pub trait FlatRows {
    /// [`ScrollArea::show_rows`] for rows of exactly `row_height` with no space between.
    fn show_rows_flat<R>(
        self,
        ui: &mut Ui,
        row_height: f32,
        total_rows: usize,
        add_contents: impl FnOnce(&mut Ui, Range<usize>) -> R,
    ) -> ScrollAreaOutput<R>;
}

impl FlatRows for ScrollArea {
    fn show_rows_flat<R>(
        self,
        ui: &mut Ui,
        row_height: f32,
        total_rows: usize,
        add_contents: impl FnOnce(&mut Ui, Range<usize>) -> R,
    ) -> ScrollAreaOutput<R> {
        ui.scope(|ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            self.show_rows(ui, row_height, total_rows, add_contents)
        })
        .inner
    }
}
