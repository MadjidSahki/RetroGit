//! Every character the UI can show must exist in the installed fonts (no "tofu" squares).

#[test]
fn every_ui_character_has_a_glyph() {
    let ctx = egui::Context::default();
    win95::theme::install(&ctx);
    let mut out = ctx.run_ui(Default::default(), |ui| {
        ui.label("warm up");
    });
    out.textures_delta.clear();
    let font = win95::theme::font(win95::theme::FONT_SIZE);
    let mono = egui::FontId::monospace(win95::theme::FONT_SIZE);

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
                // The diff view uses the monospace font, everything else the W95FA font.
                let font_id = if file_is_diff { &mono } else { &font };
                let ok = ctx.fonts_mut(|fonts| fonts.has_glyph(font_id, c));
                if !ok {
                    missing.push(format!(
                        "{}:{} {c:?} (U+{:04X})",
                        f.file_name()
                            .map(|n| n.to_string_lossy())
                            .unwrap_or_default(),
                        n + 1,
                        c as u32
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
