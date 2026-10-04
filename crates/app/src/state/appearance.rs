//! View > Appearance: the dialog's pending choice, zoom steps, syntax colors to redo.

use super::AppState;
use crate::config::{AppearanceConfig, MAX_ZOOM, MIN_ZOOM};
use crate::highlight::Colors;
use crate::strings as s;

/// The Appearance window: what is selected, and what was saved when it opened.
#[derive(Debug, Clone, PartialEq)]
pub struct AppearanceDialog {
    pub scheme: win95::Scheme,
    pub font: win95::theme::Font,
    pub zoom: f32,
    pub before: AppearanceConfig,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZoomStep {
    In,
    Out,
    Reset,
}

/// Small / Medium / Large of the Size list.
pub const SIZES: [f32; 3] = [1.0, 1.25, 1.5];

/// Zoom after Cmd/Ctrl +, - or 0: steps of 10 %, within 80-200 %.
pub fn next_zoom(current: f32, step: ZoomStep) -> f32 {
    let z = match step {
        ZoomStep::In => current + 0.1,
        ZoomStep::Out => current - 0.1,
        ZoomStep::Reset => 1.0,
    };
    ((z * 100.0).round() / 100.0).clamp(MIN_ZOOM, MAX_ZOOM)
}

/// "Small (100%)", ... or "Custom (110%)" for a zoom set with the keyboard.
pub fn size_label(zoom: f32) -> String {
    let near = |z: f32| (zoom - z).abs() < 0.001;
    if near(SIZES[0]) {
        s::SIZE_SMALL.into()
    } else if near(SIZES[1]) {
        s::SIZE_MEDIUM.into()
    } else if near(SIZES[2]) {
        s::SIZE_LARGE.into()
    } else {
        s::SIZE_CUSTOM.replace("{n}", &format!("{}", (zoom * 100.0).round()))
    }
}

impl AppState {
    pub fn open_appearance(&mut self) {
        let saved = self.config.appearance.clone();
        let a = saved.appearance();
        self.appearance_dialog = Some(AppearanceDialog {
            scheme: a.scheme,
            font: a.font,
            zoom: saved.zoom(),
            before: saved,
        });
    }

    /// Save the dialog's choice (the app applies the saved appearance every frame).
    pub fn appearance_apply(&mut self) {
        if let Some(d) = &self.appearance_dialog {
            let chosen = AppearanceConfig::from_choice(
                win95::theme::Appearance {
                    scheme: d.scheme,
                    font: d.font,
                },
                d.zoom,
            );
            self.set_appearance(chosen);
        }
    }

    pub fn appearance_ok(&mut self) {
        self.appearance_apply();
        self.appearance_dialog = None;
    }

    /// Back to the appearance saved when the window opened.
    pub fn appearance_cancel(&mut self) {
        if let Some(d) = self.appearance_dialog.take() {
            self.set_appearance(d.before);
        }
    }

    pub fn zoom_key(&mut self, step: ZoomStep) {
        let mut a = self.config.appearance.clone();
        a.zoom = next_zoom(a.zoom(), step);
        self.set_appearance(a);
    }

    fn set_appearance(&mut self, a: AppearanceConfig) {
        if self.config.appearance != a {
            self.config.appearance = a;
            self.config_dirty = true;
        }
    }

    /// Syntax colors were computed for another scheme (light or dark theme): ask again.
    pub fn forget_colors(&mut self, dark: bool) {
        self.colors_dark = dark;
        self.changes.diff_colors = Colors::NotRequested;
        self.history.detail_colors = Colors::NotRequested;
        self.stashes.colors = Colors::NotRequested;
        self.pulls.file_colors = Colors::NotRequested;
        if let Some(ed) = self.changes.conflict.as_mut() {
            ed.mine_colors = Colors::NotRequested;
            ed.theirs_colors = Colors::NotRequested;
            ed.result_colors = Colors::NotRequested;
        }
    }
}
