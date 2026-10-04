//! Win95 palette, font and egui style.

use std::sync::Arc;

use egui::{
    CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Id, Margin, Shadow, Stroke,
    TextStyle, Vec2,
};

use crate::palette::{Palette, Scheme};

/// Base font size in points. W95FA is a pixel font: keep this a whole number.
pub const FONT_SIZE: f32 = 13.0;
pub const FONT_NAME: &str = "W95FA";

static W95FA: &[u8] = include_bytes!("../assets/W95FA.otf");
static ATKINSON: &[u8] = include_bytes!("../assets/AtkinsonHyperlegible-Regular.ttf");
pub const ATKINSON_NAME: &str = "Atkinson Hyperlegible";

/// Interface font (code and diffs always use egui's monospace font).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Font {
    #[default]
    W95fa,
    Atkinson,
}

impl Font {
    pub const ALL: [Font; 2] = [Font::W95fa, Font::Atkinson];

    /// Name shown to the user, stored in the configuration, and egui font family name.
    pub fn name(self) -> &'static str {
        match self {
            Font::W95fa => FONT_NAME,
            Font::Atkinson => ATKINSON_NAME,
        }
    }

    pub fn from_name(name: &str) -> Option<Font> {
        Font::ALL.into_iter().find(|f| f.name() == name)
    }

    /// egui family with this font first and only emoji fallbacks: previews, glyph checks.
    pub fn family(self) -> FontFamily {
        FontFamily::Name(self.name().into())
    }

    /// [`Font::family`] if the fonts are loaded on `ctx` (they are from the second frame
    /// after `install`), else the default family.
    pub fn family_on(self, ctx: &egui::Context) -> FontFamily {
        let family = self.family();
        if ctx.fonts(|f| f.families().contains(&family)) {
            family
        } else {
            FontFamily::Proportional
        }
    }
}

/// What the user chose in View > Appearance (the size is egui's zoom factor).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Appearance {
    pub scheme: Scheme,
    pub font: Font,
}

fn appearance_id() -> Id {
    Id::new("win95_appearance")
}

fn override_id() -> Id {
    Id::new("win95_palette_override")
}

pub fn font(size: f32) -> FontId {
    FontId::new(size, FontFamily::Proportional)
}

/// Install the fonts and the Windows Standard look on `ctx`. Call once at startup.
pub fn install(ctx: &egui::Context) {
    ctx.set_theme(egui::Theme::Light);
    set_fonts(ctx, Font::W95fa);
    store(ctx, Appearance::default());
}

/// Switch color scheme and font (fonts are reloaded only when the font changes).
pub fn apply(ctx: &egui::Context, a: Appearance) {
    let before = appearance(ctx);
    if before.font != a.font {
        set_fonts(ctx, a.font);
    }
    store(ctx, a);
}

fn store(ctx: &egui::Context, a: Appearance) {
    ctx.data_mut(|d| d.insert_temp(appearance_id(), a));
    let palette = a.scheme.palette();
    ctx.all_styles_mut(|style| apply_style(style, &palette));
}

/// The current choice (Windows Standard and W95FA if nothing was installed).
pub fn appearance(ctx: &egui::Context) -> Appearance {
    ctx.data(|d| d.get_temp(appearance_id()))
        .unwrap_or_default()
}

/// Colors to paint with: the current scheme, or the one of [`with_palette`].
pub fn palette(ctx: &egui::Context) -> Palette {
    ctx.data(|d| d.get_temp::<Palette>(override_id()))
        .unwrap_or_else(|| appearance(ctx).scheme.palette())
}

/// Draw `add` with another palette (the Appearance preview).
pub fn with_palette<R>(
    ui: &mut egui::Ui,
    palette: Palette,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    let ctx = ui.ctx().clone();
    let before = ctx.data(|d| d.get_temp::<Palette>(override_id()));
    ctx.data_mut(|d| d.insert_temp(override_id(), palette));
    let r = ui
        .scope(|ui| {
            let mut style = (**ui.style()).clone();
            apply_style(&mut style, &palette);
            ui.set_style(style);
            add(ui)
        })
        .inner;
    ctx.data_mut(|d| {
        if let Some(p) = before {
            d.insert_temp(override_id(), p);
        } else {
            d.remove::<Palette>(override_id());
        }
    });
    r
}

fn set_fonts(ctx: &egui::Context, first: Font) {
    let mut fonts = FontDefinitions::default();
    fonts
        .font_data
        .insert(FONT_NAME.to_owned(), Arc::new(FontData::from_static(W95FA)));
    fonts.font_data.insert(
        ATKINSON_NAME.to_owned(),
        Arc::new(FontData::from_static(ATKINSON)),
    );
    // egui's emoji fonts (not its text font) as fallback: they hold the replacement
    // character, so `has_glyph` on these families tells whether the font itself has it.
    let emoji: Vec<String> = fonts
        .families
        .get(&FontFamily::Proportional)
        .into_iter()
        .flatten()
        .filter(|n| n.contains("moji"))
        .cloned()
        .collect();
    for f in Font::ALL {
        let mut list = vec![f.name().to_owned()];
        list.extend(emoji.iter().cloned());
        fonts.families.insert(f.family(), list);
    }
    // The chosen font first for UI text; egui's default fonts stay as fallback.
    // Monospace keeps egui's fixed-width font: both UI fonts are proportional.
    let prop = fonts.families.entry(FontFamily::Proportional).or_default();
    prop.insert(0, first.name().to_owned());
    ctx.set_fonts(fonts);
}

fn apply_style(style: &mut egui::Style, p: &Palette) {
    style.text_styles = [
        (TextStyle::Small, font(FONT_SIZE)),
        (TextStyle::Body, font(FONT_SIZE)),
        (TextStyle::Button, font(FONT_SIZE)),
        (
            TextStyle::Monospace,
            FontId::new(FONT_SIZE, FontFamily::Monospace),
        ),
        (TextStyle::Heading, font(FONT_SIZE + 3.0)),
    ]
    .into();
    style.spacing.item_spacing = Vec2::new(6.0, 4.0);
    style.spacing.button_padding = Vec2::new(6.0, 2.0);
    style.spacing.menu_margin = Margin::same(2);
    style.spacing.window_margin = Margin::same(3);
    style.animation_time = 0.0;

    let v = &mut style.visuals;
    v.dark_mode = p.dark;
    v.panel_fill = p.face;
    v.window_fill = p.face;
    v.faint_bg_color = p.face;
    v.extreme_bg_color = p.window;
    v.text_edit_bg_color = Some(p.window);
    v.code_bg_color = p.code_bg;
    v.weak_text_color = Some(p.gray_text);
    v.window_corner_radius = CornerRadius::ZERO;
    v.menu_corner_radius = CornerRadius::ZERO;
    v.window_shadow = Shadow::NONE;
    v.popup_shadow = Shadow::NONE;
    v.window_stroke = Stroke::new(1.0, p.shadow);
    v.selection.bg_fill = p.selection;
    v.selection.stroke = Stroke::new(1.0, p.selection_text);
    v.text_cursor.stroke = Stroke::new(2.0, p.window_text);
    v.hyperlink_color = p.link;
    v.error_fg_color = p.error;
    v.warn_fg_color = p.warning;
    for w in [
        &mut v.widgets.noninteractive,
        &mut v.widgets.inactive,
        &mut v.widgets.open,
    ] {
        w.bg_fill = p.face;
        w.weak_bg_fill = p.face;
        w.bg_stroke = Stroke::NONE;
        w.fg_stroke = Stroke::new(1.0, p.text);
        w.corner_radius = CornerRadius::ZERO;
        w.expansion = 0.0;
    }
    // Win95 menus and hovered items: selection color with its text.
    for w in [&mut v.widgets.hovered, &mut v.widgets.active] {
        w.bg_fill = p.selection;
        w.weak_bg_fill = p.selection;
        w.bg_stroke = Stroke::NONE;
        w.fg_stroke = Stroke::new(1.0, p.selection_text);
        w.corner_radius = CornerRadius::ZERO;
        w.expansion = 0.0;
    }
    v.widgets.open.weak_bg_fill = p.selection;
    v.widgets.open.fg_stroke = Stroke::new(1.0, p.selection_text);
}
