use egui::{Rect, Response, Sense, Ui, Widget, pos2, vec2};

use crate::bevel::{self, Bevel};

const BLOCK: f32 = 8.0;
const GAP: f32 = 2.0;

/// Segmented blue progress bar. `fraction = None` shows an animated marquee.
pub struct ProgressBar95 {
    fraction: Option<f32>,
    width: f32,
}

impl ProgressBar95 {
    pub fn new(fraction: Option<f32>) -> ProgressBar95 {
        ProgressBar95 {
            fraction,
            width: 260.0,
        }
    }

    pub fn width(mut self, width: f32) -> ProgressBar95 {
        self.width = width;
        self
    }
}

/// Number of blocks drawn for `fraction` in a bar whose inner width is `inner_width`.
pub fn block_count(fraction: f32, inner_width: f32) -> usize {
    let total = ((inner_width + GAP) / (BLOCK + GAP)).floor().max(0.0) as usize;
    ((fraction.clamp(0.0, 1.0) * total as f32).round() as usize).min(total)
}

impl Widget for ProgressBar95 {
    fn ui(self, ui: &mut Ui) -> Response {
        let pal = crate::theme::palette(ui.ctx());
        let (rect, resp) = ui.allocate_exact_size(vec2(self.width, 20.0), Sense::hover());
        let p = ui.painter();
        p.rect_filled(rect, 0.0, pal.face);
        bevel::paint(p, rect, Bevel::Shallow);
        let inner = rect.shrink(3.0);
        let total = block_count(1.0, inner.width());
        let block_at = |i: usize| {
            let x = inner.left() + i as f32 * (BLOCK + GAP);
            Rect::from_min_max(
                pos2(x, inner.top()),
                pos2((x + BLOCK).min(inner.right()), inner.bottom()),
            )
        };
        match self.fraction {
            Some(f) => {
                for i in 0..block_count(f, inner.width()) {
                    p.rect_filled(block_at(i), 0.0, pal.selection);
                }
            }
            None => {
                // Marquee: 5 blocks sliding, repaint only while visible.
                let step = (ui.input(|i| i.time) * 10.0) as usize;
                let span = total + 5;
                for k in 0..5 {
                    let i = (step + k) % span.max(1);
                    if i >= 5 && i - 5 < total {
                        p.rect_filled(block_at(i - 5), 0.0, pal.selection);
                    }
                }
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_millis(100));
            }
        }
        resp
    }
}

#[cfg(test)]
mod tests {
    use super::block_count;

    #[test]
    fn block_count_scales_and_clamps() {
        // 98 px inner width => (98 + 2) / 10 = 10 blocks.
        assert_eq!(block_count(0.0, 98.0), 0);
        assert_eq!(block_count(0.5, 98.0), 5);
        assert_eq!(block_count(1.0, 98.0), 10);
        assert_eq!(block_count(1.7, 98.0), 10);
        assert_eq!(block_count(-1.0, 98.0), 0);
        assert_eq!(block_count(1.0, 0.0), 0);
    }
}
