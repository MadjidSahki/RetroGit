//! Win95 palette, font and egui style.

use std::sync::Arc;

use egui::{
    Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Margin, Shadow, Stroke,
    TextStyle, Vec2,
};

pub const SILVER: Color32 = Color32::from_rgb(0xC0, 0xC0, 0xC0);
pub const LIGHT: Color32 = Color32::from_rgb(0xDF, 0xDF, 0xDF);
pub const WHITE: Color32 = Color32::WHITE;
pub const GRAY: Color32 = Color32::from_rgb(0x80, 0x80, 0x80);
pub const BLACK: Color32 = Color32::BLACK;
pub const NAVY: Color32 = Color32::from_rgb(0x00, 0x00, 0x80);
pub const TITLE_END: Color32 = Color32::from_rgb(0x10, 0x84, 0xD0);
pub const INACTIVE_TITLE: Color32 = Color32::from_rgb(0x80, 0x80, 0x80);
pub const INACTIVE_TITLE_END: Color32 = Color32::from_rgb(0xB5, 0xB5, 0xB5);

/// Base font size in points. W95FA is a pixel font: keep this a whole number.
pub const FONT_SIZE: f32 = 13.0;
pub const FONT_NAME: &str = "W95FA";

static W95FA: &[u8] = include_bytes!("../assets/W95FA.otf");

pub fn font(size: f32) -> FontId {
    FontId::new(size, FontFamily::Proportional)
}

/// Install the Win95 font and style on `ctx`. Call once at startup.
pub fn install(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    fonts
        .font_data
        .insert(FONT_NAME.to_owned(), Arc::new(FontData::from_static(W95FA)));
    for family in [FontFamily::Proportional, FontFamily::Monospace] {
        // W95FA first, egui's default fonts stay as fallback for missing glyphs.
        fonts
            .families
            .entry(family)
            .or_default()
            .insert(0, FONT_NAME.to_owned());
    }
    ctx.set_fonts(fonts);
    ctx.set_theme(egui::Theme::Light);
    ctx.all_styles_mut(apply_style);
}

fn apply_style(style: &mut egui::Style) {
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
    v.dark_mode = false;
    v.panel_fill = SILVER;
    v.window_fill = SILVER;
    v.faint_bg_color = SILVER;
    v.extreme_bg_color = WHITE;
    v.text_edit_bg_color = Some(WHITE);
    v.window_corner_radius = CornerRadius::ZERO;
    v.menu_corner_radius = CornerRadius::ZERO;
    v.window_shadow = Shadow::NONE;
    v.popup_shadow = Shadow::NONE;
    v.window_stroke = Stroke::new(1.0, GRAY);
    v.selection.bg_fill = NAVY;
    v.selection.stroke = Stroke::new(1.0, WHITE);
    v.hyperlink_color = NAVY;
    for w in [
        &mut v.widgets.noninteractive,
        &mut v.widgets.inactive,
        &mut v.widgets.open,
    ] {
        w.bg_fill = SILVER;
        w.weak_bg_fill = SILVER;
        w.bg_stroke = Stroke::NONE;
        w.fg_stroke = Stroke::new(1.0, BLACK);
        w.corner_radius = CornerRadius::ZERO;
        w.expansion = 0.0;
    }
    // Win95 menus and hovered items: navy highlight with white text.
    for w in [&mut v.widgets.hovered, &mut v.widgets.active] {
        w.bg_fill = NAVY;
        w.weak_bg_fill = NAVY;
        w.bg_stroke = Stroke::NONE;
        w.fg_stroke = Stroke::new(1.0, WHITE);
        w.corner_radius = CornerRadius::ZERO;
        w.expansion = 0.0;
    }
    v.widgets.open.weak_bg_fill = NAVY;
    v.widgets.open.fg_stroke = Stroke::new(1.0, WHITE);
}
