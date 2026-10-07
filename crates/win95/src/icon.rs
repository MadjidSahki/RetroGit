use egui::{Align2, Color32, Pos2, Stroke, Ui, pos2, vec2};

use crate::theme;

const BLACK: Color32 = Color32::BLACK;
const WHITE: Color32 = Color32::WHITE;
const NAVY: Color32 = Color32::from_rgb(0x00, 0x00, 0x80);

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

/// Small closed or open folder pictogram, centred in `rect` (tree rows).
pub fn folder(p: &egui::Painter, rect: egui::Rect, open: bool) {
    let r = egui::Rect::from_center_size(rect.center(), vec2(14.0, 11.0));
    let edge = Stroke::new(1.0, BLACK);
    let yellow = Color32::from_rgb(0xFF, 0xE0, 0x60);
    let tab = egui::Rect::from_min_size(r.min, vec2(6.0, 3.0));
    p.rect_filled(tab, 0.0, yellow);
    p.rect_stroke(tab, 0.0, edge, egui::StrokeKind::Inside);
    let body = egui::Rect::from_min_max(pos2(r.min.x, r.min.y + 2.0), r.max);
    p.rect_filled(body, 0.0, yellow);
    p.rect_stroke(body, 0.0, edge, egui::StrokeKind::Inside);
    if open {
        let flap = egui::Rect::from_min_max(pos2(r.min.x + 2.0, r.min.y + 5.0), r.max);
        p.rect_filled(flap, 0.0, Color32::from_rgb(0xFF, 0xF0, 0xA0));
        p.rect_stroke(flap, 0.0, edge, egui::StrokeKind::Inside);
    }
}
