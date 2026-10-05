//! Every character the UI can show must exist in the installed fonts (no "tofu" squares).

use std::sync::Arc;

use egui::{FontData, FontDefinitions, FontFamily, FontId};
use win95::theme::{FONT_SIZE, Font, font_bytes};

/// A context whose only fonts are W95FA and Atkinson, each alone in its family: no glyph
/// can come from a fallback (emoji) font.
fn strict_ctx() -> egui::Context {
    let ctx = egui::Context::default();
    let mut defs = FontDefinitions::empty();
    for f in Font::ALL {
        defs.font_data.insert(
            f.name().to_owned(),
            Arc::new(FontData::from_static(font_bytes(f))),
        );
        defs.families.insert(f.family(), vec![f.name().to_owned()]);
    }
    for family in [FontFamily::Proportional, FontFamily::Monospace] {
        defs.families
            .insert(family, vec![Font::W95fa.name().to_owned()]);
    }
    ctx.set_fonts(defs);
    let mut out = ctx.run_ui(Default::default(), |ui| {
        ui.label("warm up");
    });
    out.textures_delta.clear();
    ctx
}

/// Whether `font` itself draws `c`. egui's `has_glyph` cannot answer for a family of one
/// font (that font is also the replacement face, so the answer is always "no"); a missing
/// character is drawn as the replacement glyph, so compare the two atlas images.
fn has(ctx: &egui::Context, font: Font, c: char) -> bool {
    let id = FontId::new(FONT_SIZE, font.family());
    let uv = |c: char| {
        let g =
            ctx.fonts_mut(|f| f.layout_no_wrap(c.to_string(), id.clone(), egui::Color32::BLACK));
        g.rows
            .first()
            .and_then(|r| r.glyphs.first())
            .map(|g| g.uv_rect)
    };
    uv(c) != uv(char::REPLACEMENT_CHARACTER)
}

#[test]
fn the_strict_check_reports_a_character_the_fonts_lack() {
    let ctx = strict_ctx();
    for f in Font::ALL {
        assert!(has(&ctx, f, 'A'), "{}", f.name());
        assert!(has(&ctx, f, 'é'), "{}", f.name());
        // Both in egui's emoji fonts, in neither interface font.
        assert!(!has(&ctx, f, '😀'), "{}", f.name());
        assert!(!has(&ctx, f, '✓'), "{}", f.name());
    }
}

#[test]
fn every_ui_character_has_a_glyph() {
    let strict = strict_ctx();
    // Monospace (diff view): egui's own font, as installed by the app.
    let ctx = egui::Context::default();
    win95::theme::install(&ctx);
    let mut out = ctx.run_ui(Default::default(), |ui| {
        ui.label("warm up");
    });
    out.textures_delta.clear();
    let mono = egui::FontId::monospace(FONT_SIZE);

    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = vec![src.join("strings.rs"), src.join("protocol.rs")];
    for entry in std::fs::read_dir(src.join("ui")).unwrap_or_else(|e| panic!("{e}")) {
        files.push(entry.unwrap_or_else(|e| panic!("{e}")).path());
    }
    let mut missing = Vec::new();
    for f in files {
        let text = std::fs::read_to_string(&f).unwrap_or_default();
        let file_is_diff = f.ends_with("diff_view.rs");
        for (n, line) in text.lines().enumerate() {
            if line.trim_start().starts_with("//") {
                continue;
            }
            for c in line.chars().filter(|c| !c.is_ascii()) {
                // The diff view uses the monospace font, everything else the interface fonts.
                let lacking: Vec<&str> = if file_is_diff {
                    (!ctx.fonts_mut(|f| f.has_glyph(&mono, c)))
                        .then_some("monospace")
                        .into_iter()
                        .collect()
                } else {
                    Font::ALL
                        .into_iter()
                        .filter(|font| !has(&strict, *font, c))
                        .map(Font::name)
                        .collect()
                };
                if !lacking.is_empty() {
                    missing.push(format!(
                        "{}:{} {c:?} (U+{:04X}) not in {}",
                        f.file_name()
                            .map(|n| n.to_string_lossy())
                            .unwrap_or_default(),
                        n + 1,
                        c as u32,
                        lacking.join(", ")
                    ));
                }
            }
        }
    }
    assert!(
        missing.is_empty(),
        "characters without a glyph:\n{}",
        missing.join("\n")
    );
}
