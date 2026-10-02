//! The 3D edges that make Win95 look like Win95.

use egui::{Color32, Painter, Rect, Vec2, pos2};

use crate::palette::Palette;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bevel {
    /// Button at rest.
    Raised,
    /// Button held down.
    Pressed,
    /// Text fields, list views: white well.
    Field,
    /// Dialog / main window border.
    Window,
    /// Thin 1-ring well (status bar cells, progress bar).
    Shallow,
    /// Group or panel slightly sunk.
    Sunken,
}

/// Edge colors as rings from outermost to innermost: `(top_left, bottom_right)`.
pub fn rings(kind: Bevel, p: &Palette) -> Vec<(Color32, Color32)> {
    let (hi, light, shadow, dark) = (p.highlight, p.light, p.shadow, p.dark_shadow);
    match kind {
        Bevel::Raised => vec![(hi, dark), (light, shadow)],
        Bevel::Pressed => vec![(dark, hi), (shadow, light)],
        Bevel::Field => vec![(shadow, hi), (dark, light)],
        Bevel::Window => vec![(light, dark), (hi, shadow)],
        Bevel::Shallow => vec![(shadow, hi)],
        Bevel::Sunken => vec![(shadow, hi), (dark, light)],
    }
}

/// Width in points of the whole bevel.
pub fn thickness(kind: Bevel) -> f32 {
    match kind {
        Bevel::Shallow => 1.0,
        _ => 2.0,
    }
}

/// Paint the bevel along the inside of `rect`, in the current palette.
pub fn paint(painter: &Painter, rect: Rect, kind: Bevel) {
    let palette = crate::theme::palette(painter.ctx());
    let mut r = rect;
    for (tl, br) in rings(kind, &palette) {
        if r.width() < 2.0 || r.height() < 2.0 {
            return;
        }
        // 1-point filled strips: crisp at any pixels_per_point, no anti-aliased lines.
        painter.rect_filled(
            Rect::from_min_max(r.min, pos2(r.max.x, r.min.y + 1.0)),
            0.0,
            tl,
        );
        painter.rect_filled(
            Rect::from_min_max(r.min, pos2(r.min.x + 1.0, r.max.y)),
            0.0,
            tl,
        );
        painter.rect_filled(
            Rect::from_min_max(pos2(r.min.x, r.max.y - 1.0), r.max),
            0.0,
            br,
        );
        painter.rect_filled(
            Rect::from_min_max(pos2(r.max.x - 1.0, r.min.y), r.max),
            0.0,
            br,
        );
        r = r.shrink2(Vec2::splat(1.0));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::palette::STANDARD;

    #[test]
    fn pressed_is_raised_inverted() {
        let raised = rings(Bevel::Raised, &STANDARD);
        let pressed = rings(Bevel::Pressed, &STANDARD);
        for (r, p) in raised.iter().zip(&pressed) {
            assert_eq!((r.1, r.0), (p.0, p.1));
        }
    }

    #[test]
    fn raised_is_lit_from_top_left() {
        assert_eq!(rings(Bevel::Raised, &STANDARD)[0].0, STANDARD.highlight);
        assert_eq!(rings(Bevel::Field, &STANDARD)[0].1, STANDARD.highlight);
        for kind in [Bevel::Raised, Bevel::Field, Bevel::Shallow, Bevel::Window] {
            assert_eq!(rings(kind, &STANDARD).len() as f32, thickness(kind));
        }
        assert_eq!(thickness(Bevel::Shallow), 1.0);
        assert_eq!(thickness(Bevel::Window), 2.0);
    }
}
