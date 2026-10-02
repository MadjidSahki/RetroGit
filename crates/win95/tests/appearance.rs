//! Color scheme and font stored in the egui context; every widget paints with them.

use egui::{Color32, Shape};
use win95::theme::{self, Appearance, Font};
use win95::{Bevel, Button95, ProgressBar95, Scheme};

fn colors(shape: &Shape, out: &mut Vec<Color32>) {
    match shape {
        Shape::Vec(v) => v.iter().for_each(|s| colors(s, out)),
        Shape::Rect(r) => out.extend([r.fill, r.stroke.color]),
        Shape::Circle(c) => out.extend([c.fill, c.stroke.color]),
        Shape::LineSegment { stroke, .. } => out.push(stroke.color),
        Shape::Path(p) => {
            out.push(p.fill);
            if let egui::epaint::ColorMode::Solid(c) = p.stroke.color {
                out.push(c);
            }
        }
        Shape::Mesh(m) => out.extend(m.vertices.iter().map(|v| v.color)),
        Shape::Text(t) => {
            out.push(t.fallback_color);
            out.extend(t.galley.job.sections.iter().map(|s| s.format.color));
        }
        _ => {}
    }
}

fn all_widgets(ui: &mut egui::Ui) {
    let mut on = true;
    let mut sel = 0;
    let mut text = String::from("text");
    win95::TitleBar::new("Title").show(ui);
    ui.add(Button95::new("Button"));
    ui.add(Button95::new("Disabled").enabled(false));
    win95::checkbox(ui, &mut on, "Check");
    win95::tabs(ui, &mut sel, &["One", "Two"]);
    win95::text_field(ui, &mut text, 100.0, false);
    win95::combo_box(ui, "c", "Choice", 100.0, |_| {});
    ui.add(ProgressBar95::new(Some(0.5)));
    win95::status_bar(ui, &[("Ready", None)]);
    win95::bevel_frame(ui, Bevel::Field, theme::palette(ui.ctx()).window, 2, |ui| {
        win95::markdown_view(ui, "# Head\n\nSome **bold** `code`\n\n> quote");
    });
}

#[test]
fn dark_widgets_use_no_standard_color() {
    let ctx = egui::Context::default();
    theme::install(&ctx);
    theme::apply(
        &ctx,
        Appearance {
            scheme: Scheme::Dark,
            font: Font::W95fa,
        },
    );
    let mut shapes = Vec::new();
    for _ in 0..2 {
        let mut out = ctx.run_ui(egui::RawInput::default(), all_widgets);
        out.textures_delta.clear();
        shapes = out.shapes;
    }
    let mut seen = Vec::new();
    for s in &shapes {
        colors(&s.shape, &mut seen);
    }
    let standard_only = [0xC0C0C0, 0xDFDFDF, 0x000080, 0x1084D0, 0x808080, 0xF0F0F0];
    for hex in standard_only {
        let c = Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8);
        assert!(
            !seen.contains(&c),
            "Standard color #{hex:06X} painted in Dark"
        );
    }
    assert!(seen.contains(&Scheme::Dark.palette().face));
}

#[test]
fn apply_changes_the_palette_and_the_preview_restores_it() {
    let ctx = egui::Context::default();
    theme::install(&ctx);
    assert_eq!(theme::appearance(&ctx), Appearance::default());
    assert_eq!(theme::palette(&ctx), Scheme::Standard.palette());
    let dark = Appearance {
        scheme: Scheme::Dark,
        font: Font::Atkinson,
    };
    theme::apply(&ctx, dark);
    assert_eq!(theme::appearance(&ctx), dark);
    assert_eq!(
        ctx.global_style().visuals.panel_fill,
        Scheme::Dark.palette().face
    );
    let out = ctx.run_ui(egui::RawInput::default(), |ui| {
        let slate = Scheme::Slate.palette();
        theme::with_palette(ui, slate, |ui| {
            assert_eq!(theme::palette(ui.ctx()), slate);
            assert_eq!(ui.visuals().panel_fill, slate.face);
        });
        assert_eq!(theme::palette(ui.ctx()), Scheme::Dark.palette());
    });
    let mut out = out;
    out.textures_delta.clear();
}

#[test]
fn both_fonts_are_installed_and_named() {
    let ctx = egui::Context::default();
    theme::install(&ctx);
    let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
        ui.label("warm up");
    });
    out.textures_delta.clear();
    for f in Font::ALL {
        assert_eq!(Font::from_name(f.name()), Some(f));
        let id = egui::FontId::new(13.0, f.family());
        let missing: String = "Clone a repository éàç"
            .chars()
            .filter(|c| !ctx.fonts_mut(|fonts| fonts.has_glyph(&id, *c)))
            .collect();
        assert!(missing.is_empty(), "{} lacks {missing:?}", f.name());
    }
    assert_eq!(Font::from_name("Comic Sans"), None);
}
