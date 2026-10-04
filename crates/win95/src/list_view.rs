use egui::{Align2, Id, Rect, ScrollArea, Sense, Ui, WidgetInfo, WidgetType, pos2, vec2};

use crate::bevel::{self, Bevel};
use crate::rows::FlatRows;
use crate::theme;

pub struct Column {
    pub title: &'static str,
    pub width: f32,
}

pub struct Cell {
    pub text: String,
    /// Greyed out (e.g. a missing folder).
    pub dimmed: bool,
}

impl From<String> for Cell {
    fn from(text: String) -> Cell {
        Cell {
            text,
            dimmed: false,
        }
    }
}

impl From<&str> for Cell {
    fn from(text: &str) -> Cell {
        Cell {
            text: text.to_string(),
            dimmed: false,
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ListResponse {
    pub clicked: Option<usize>,
    pub double_clicked: Option<usize>,
    pub secondary_clicked: Option<usize>,
}

/// Virtualised multi-column list in a white sunken well. Only visible rows are laid out,
/// so thousands of rows cost the same as twenty.
pub struct ListView<'a> {
    id: Id,
    columns: &'a [Column],
    row_count: usize,
    show_header: bool,
    height: f32,
    context_menu: Option<ContextMenuFn<'a>>,
}

type ContextMenuFn<'a> = Box<dyn FnMut(usize, &mut Ui) + 'a>;

pub const ROW_HEIGHT: f32 = 18.0;

impl<'a> ListView<'a> {
    pub fn new(id: impl egui::AsId, columns: &'a [Column], row_count: usize) -> ListView<'a> {
        ListView {
            id: Id::new(id),
            columns,
            row_count,
            show_header: true,
            height: 200.0,
            context_menu: None,
        }
    }

    pub fn header(mut self, show: bool) -> Self {
        self.show_header = show;
        self
    }

    pub fn height(mut self, height: f32) -> Self {
        self.height = height;
        self
    }

    /// Right-click menu for a row.
    pub fn context_menu(mut self, menu: impl FnMut(usize, &mut Ui) + 'a) -> Self {
        self.context_menu = Some(Box::new(menu));
        self
    }

    pub fn show(
        mut self,
        ui: &mut Ui,
        selected: Option<usize>,
        mut cell: impl FnMut(usize, usize) -> Cell,
    ) -> ListResponse {
        let mut out = ListResponse::default();
        let width = ui.available_width();
        let font = theme::font(theme::FONT_SIZE);
        let pal = theme::palette(ui.ctx());
        let (outer, _) = ui.allocate_exact_size(vec2(width, self.height), Sense::hover());
        ui.painter().rect_filled(outer, 0.0, pal.window);
        bevel::paint(ui.painter(), outer, Bevel::Field);
        let inner = outer.shrink(2.0);
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(inner).id_salt(self.id));
        child.set_clip_rect(inner);
        let ui = &mut child;
        ui.spacing_mut().item_spacing = vec2(0.0, 0.0);

        if self.show_header {
            let (hdr, _) =
                ui.allocate_exact_size(vec2(inner.width(), ROW_HEIGHT + 2.0), Sense::hover());
            let mut x = hdr.left();
            for col in self.columns {
                let r = Rect::from_min_size(pos2(x, hdr.top()), vec2(col.width, hdr.height()));
                ui.painter().rect_filled(r, 0.0, pal.face);
                bevel::paint(ui.painter(), r, Bevel::Raised);
                ui.painter().with_clip_rect(r.shrink(2.0)).text(
                    r.left_center() + vec2(4.0, 0.0),
                    Align2::LEFT_CENTER,
                    col.title,
                    font.clone(),
                    pal.window_text,
                );
                x += col.width;
            }
        }

        ScrollArea::vertical()
            .id_salt(self.id.with("scroll"))
            .auto_shrink([false, false])
            .show_rows_flat(ui, ROW_HEIGHT, self.row_count, |ui, range| {
                for row in range {
                    let (rect, resp) = ui.allocate_exact_size(
                        vec2(ui.available_width(), ROW_HEIGHT),
                        Sense::click(),
                    );
                    let is_sel = selected == Some(row);
                    let cells: Vec<Cell> = (0..self.columns.len()).map(|c| cell(row, c)).collect();
                    let label = cells.first().map(|c| c.text.clone()).unwrap_or_default();
                    resp.widget_info(|| {
                        WidgetInfo::selected(WidgetType::SelectableLabel, true, is_sel, &label)
                    });
                    if is_sel {
                        ui.painter().rect_filled(rect, 0.0, pal.selection);
                    }
                    let mut x = rect.left();
                    for (col, c) in self.columns.iter().zip(&cells) {
                        let r =
                            Rect::from_min_size(pos2(x, rect.top()), vec2(col.width, ROW_HEIGHT));
                        let color = match (is_sel, c.dimmed) {
                            (true, _) => pal.selection_text,
                            (false, true) => pal.gray_text,
                            (false, false) => pal.window_text,
                        };
                        ui.painter().with_clip_rect(r.shrink(1.0)).text(
                            r.left_center() + vec2(4.0, 0.0),
                            Align2::LEFT_CENTER,
                            &c.text,
                            font.clone(),
                            color,
                        );
                        x += col.width;
                    }
                    if resp.clicked() {
                        out.clicked = Some(row);
                    }
                    if resp.double_clicked() {
                        out.double_clicked = Some(row);
                    }
                    if resp.secondary_clicked() {
                        out.secondary_clicked = Some(row);
                    }
                    if let Some(menu) = self.context_menu.as_mut() {
                        resp.context_menu(|ui| menu(row, ui));
                    }
                }
            });
        out
    }
}

#[cfg(test)]
mod tests {
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    use super::{Column, ListView};

    const COLS: &[Column] = &[
        Column {
            title: "Name",
            width: 120.0,
        },
        Column {
            title: "Owner",
            width: 120.0,
        },
    ];

    #[test]
    fn click_selects_row_and_only_visible_rows_are_built() {
        let mut h = Harness::new_ui_state(
            |ui, sel: &mut Option<usize>| {
                let r = ListView::new("repos", COLS, 10_000).height(200.0).show(
                    ui,
                    *sel,
                    |row, col| {
                        if col == 0 {
                            format!("repo{row}").into()
                        } else {
                            "ada".into()
                        }
                    },
                );
                if let Some(i) = r.clicked {
                    *sel = Some(i);
                }
            },
            None,
        );
        h.run();
        h.get_by_label("repo3").click();
        h.run();
        assert_eq!(*h.state(), Some(3));
        // Virtualisation: row 5000 is not laid out.
        assert!(h.query_by_label("repo5000").is_none());
    }

    #[test]
    fn context_menu_runs_for_the_right_clicked_row() {
        let mut h = Harness::new_ui_state(
            |ui, removed: &mut Option<usize>| {
                ListView::new("recents", COLS, 3)
                    .context_menu(|row, ui| {
                        if ui.button("Remove from list").clicked() {
                            *removed = Some(row);
                        }
                    })
                    .show(ui, None, |row, _| format!("repo{row}").into());
            },
            None,
        );
        h.run();
        h.get_by_label("repo1").click_secondary();
        h.run();
        h.get_by_label("Remove from list").click();
        h.run();
        assert_eq!(*h.state(), Some(1));
    }
}
