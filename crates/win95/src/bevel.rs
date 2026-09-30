//! The 3D edges that make Win95 look like Win95.

use egui::{Color32, Painter, Rect, Vec2, pos2};

use crate::theme::{BLACK, GRAY, LIGHT, WHITE};

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
pub fn rings(kind: Bevel) -> &'static [(Color32, Color32)] {
    match kind {
        Bevel::Raised => &[(WHITE, BLACK), (LIGHT, GRAY)],
        Bevel::Pressed => &[(BLACK, WHITE), (GRAY, LIGHT)],
        Bevel::Field => &[(GRAY, WHITE), (BLACK, LIGHT)],
        Bevel::Window => &[(LIGHT, BLACK), (WHITE, GRAY)],
        Bevel::Shallow => &[(GRAY, WHITE)],
        Bevel::Sunken => &[(GRAY, WHITE), (BLACK, LIGHT)],
    }
}

/// Width in points of the whole bevel.
pub fn thickness(kind: Bevel) -> f32 {
    rings(kind).len() as f32
}

/// Paint the bevel along the inside of `rect`.
pub fn paint(painter: &Painter, rect: Rect, kind: Bevel) {
    let mut r = rect;
    for &(tl, br) in rings(kind) {
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

    #[test]
    fn pressed_is_raised_inverted() {
        let raised = rings(Bevel::Raised);
        let pressed = rings(Bevel::Pressed);
        for (r, p) in raised.iter().zip(pressed) {
            assert_eq!((r.1, r.0), (p.0, p.1));
        }
    }

    #[test]
    fn raised_is_lit_from_top_left() {
        assert_eq!(rings(Bevel::Raised)[0].0, WHITE);
        assert_eq!(rings(Bevel::Field)[0].1, WHITE);
        assert_eq!(thickness(Bevel::Shallow), 1.0);
        assert_eq!(thickness(Bevel::Window), 2.0);
    }
}
