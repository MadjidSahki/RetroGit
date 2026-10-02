use egui::{Align2, Color32, Mesh, Rect, Sense, Stroke, Ui, WidgetInfo, WidgetType, pos2, vec2};

use crate::bevel::{self, Bevel};
use crate::theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TitleAction {
    None,
    Minimize,
    ToggleMaximize,
    Close,
    /// The user started dragging the title bar.
    StartDrag,
}

/// Blue gradient title bar with caption buttons.
pub struct TitleBar<'a> {
    title: &'a str,
    active: bool,
    close_only: bool,
}

pub const HEIGHT: f32 = 18.0;

impl<'a> TitleBar<'a> {
    pub fn new(title: &'a str) -> TitleBar<'a> {
        TitleBar {
            title,
            active: true,
            close_only: false,
        }
    }

    pub fn active(mut self, active: bool) -> Self {
        self.active = active;
        self
    }

    /// Dialogs only have the close button.
    pub fn close_only(mut self) -> Self {
        self.close_only = true;
        self
    }

    pub fn show(self, ui: &mut Ui) -> TitleAction {
        let pal = theme::palette(ui.ctx());
        let width = ui.available_width();
        let (rect, bar) = ui.allocate_exact_size(vec2(width, HEIGHT), Sense::click_and_drag());
        let (start, end) = if self.active {
            (pal.title, pal.title_end)
        } else {
            (pal.inactive_title, pal.inactive_title_end)
        };
        ui.painter().add(gradient(rect, start, end));
        ui.painter().text(
            rect.left_center() + vec2(4.0, 0.0),
            Align2::LEFT_CENTER,
            self.title,
            theme::font(theme::FONT_SIZE),
            if self.active {
                pal.title_text
            } else {
                pal.inactive_title_text
            },
        );

        let mut action = TitleAction::None;
        let btn = vec2(16.0, 14.0);
        let mut x = rect.right() - 2.0 - btn.x;
        let y = rect.center().y - btn.y / 2.0;
        let mut buttons: Vec<(Glyph, &str, TitleAction)> =
            vec![(Glyph::Close, "Close", TitleAction::Close)];
        if !self.close_only {
            buttons.push((Glyph::Maximize, "Maximize", TitleAction::ToggleMaximize));
            buttons.push((Glyph::Minimize, "Minimize", TitleAction::Minimize));
        }
        for (i, (glyph, label, a)) in buttons.into_iter().enumerate() {
            let r = Rect::from_min_size(pos2(x, y), btn);
            if caption_button(ui, r, glyph, label) {
                action = a;
            }
            // Win95 leaves a 2px gap between Close and the others.
            x -= btn.x + if i == 0 { 2.0 } else { 0.0 };
        }

        if action == TitleAction::None {
            if bar.double_clicked() && !self.close_only {
                action = TitleAction::ToggleMaximize;
            } else if bar.drag_started() {
                action = TitleAction::StartDrag;
            }
        }
        action
    }
}

#[derive(Clone, Copy)]
enum Glyph {
    Minimize,
    Maximize,
    Close,
}

fn caption_button(ui: &mut Ui, rect: Rect, glyph: Glyph, label: &str) -> bool {
    let id = ui.id().with(("caption", label));
    let resp = ui.interact(rect, id, Sense::click());
    let label_owned = label.to_string();
    resp.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, &label_owned));
    let pressed = resp.is_pointer_button_down_on();
    let pal = theme::palette(ui.ctx());
    let p = ui.painter();
    p.rect_filled(rect, 0.0, pal.face);
    bevel::paint(
        p,
        rect,
        if pressed {
            Bevel::Pressed
        } else {
            Bevel::Raised
        },
    );
    let c = rect.center()
        + if pressed {
            vec2(1.0, 1.0)
        } else {
            vec2(0.0, 0.0)
        };
    let s = Stroke::new(1.0, pal.text);
    match glyph {
        Glyph::Minimize => {
            p.rect_filled(
                Rect::from_min_size(c + vec2(-4.0, 2.0), vec2(6.0, 2.0)),
                0.0,
                pal.text,
            );
        }
        Glyph::Maximize => {
            let r = Rect::from_min_size(c + vec2(-4.5, -4.5), vec2(9.0, 8.0));
            p.rect_stroke(r, 0.0, s, egui::StrokeKind::Inside);
            p.rect_filled(Rect::from_min_size(r.min, vec2(9.0, 2.0)), 0.0, pal.text);
        }
        Glyph::Close => {
            let s = Stroke::new(1.5, pal.text);
            p.line_segment([c + vec2(-3.5, -3.0), c + vec2(3.5, 3.0)], s);
            p.line_segment([c + vec2(3.5, -3.0), c + vec2(-3.5, 3.0)], s);
        }
    }
    resp.clicked()
}

fn gradient(rect: Rect, left: Color32, right: Color32) -> Mesh {
    let mut mesh = Mesh::default();
    mesh.colored_vertex(rect.left_top(), left);
    mesh.colored_vertex(rect.right_top(), right);
    mesh.colored_vertex(rect.right_bottom(), right);
    mesh.colored_vertex(rect.left_bottom(), left);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    mesh
}

#[cfg(test)]
mod tests {
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    use super::{TitleAction, TitleBar};

    fn click(label: &str, close_only: bool) -> TitleAction {
        let mut h = Harness::new_ui_state(
            move |ui, last: &mut TitleAction| {
                let mut bar = TitleBar::new("RetroGit");
                if close_only {
                    bar = bar.close_only();
                }
                let a = bar.show(ui);
                if a != TitleAction::None {
                    *last = a;
                }
            },
            TitleAction::None,
        );
        h.get_by_label(label).click();
        h.run();
        *h.state()
    }

    #[test]
    fn caption_buttons_report_their_action() {
        assert_eq!(click("Close", false), TitleAction::Close);
        assert_eq!(click("Minimize", false), TitleAction::Minimize);
        assert_eq!(click("Maximize", false), TitleAction::ToggleMaximize);
    }

    #[test]
    fn dialogs_only_have_close() {
        let mut h = Harness::new_ui(|ui| {
            TitleBar::new("Dialog").close_only().show(ui);
        });
        h.run();
        assert!(h.query_by_label("Minimize").is_none());
        assert!(h.query_by_label("Close").is_some());
    }
}
