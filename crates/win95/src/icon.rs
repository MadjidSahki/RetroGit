use egui::{Align2, Color32, Pos2, Stroke, Ui, pos2, vec2};

use crate::theme::{self, BLACK, NAVY, WHITE};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Error,
    Warning,
    Info,
}

const RED: Color32 = Color32::from_rgb(0xFF, 0x00, 0x00);
const YELLOW: Color32 = Color32::from_rgb(0xFF, 0xFF, 0x00);

/// Paint a 32×32 message-box icon at the current cursor position.
pub fn icon(ui: &mut Ui, kind: Icon) {
    let (rect, _) = ui.allocate_exact_size(vec2(32.0, 32.0), egui::Sense::hover());
    let p = ui.painter();
    let c = rect.center();
    let bold = theme::font(20.0);
    match kind {
        Icon::Error => {
            p.circle_filled(c, 15.0, RED);
            p.circle_stroke(c, 15.0, Stroke::new(1.0, BLACK));
            let d = 6.0;
            let s = Stroke::new(3.0, WHITE);
            p.line_segment([c + vec2(-d, -d), c + vec2(d, d)], s);
            p.line_segment([c + vec2(d, -d), c + vec2(-d, d)], s);
        }
        Icon::Warning => {
            let pts: Vec<Pos2> = vec![
                pos2(c.x, rect.top() + 1.0),
                rect.right_bottom() - vec2(1.0, 2.0),
                rect.left_bottom() + vec2(1.0, -2.0),
            ];
            p.add(egui::Shape::convex_polygon(
                pts,
                YELLOW,
                Stroke::new(1.0, BLACK),
            ));
            p.text(c + vec2(0.0, 4.0), Align2::CENTER_CENTER, "!", bold, BLACK);
        }
        Icon::Info => {
            p.circle_filled(c, 15.0, WHITE);
            p.circle_stroke(c, 15.0, Stroke::new(1.0, BLACK));
            p.text(c, Align2::CENTER_CENTER, "i", bold, NAVY);
        }
    }
}
