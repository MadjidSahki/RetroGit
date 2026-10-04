//! Every character the UI can show must exist in the installed fonts (no "tofu" squares).

#[test]
fn every_ui_character_has_a_glyph() {
    let ctx = egui::Context::default();
    win95::theme::install(&ctx);
    let mut out = ctx.run_ui(Default::default(), |ui| {
        ui.label("warm up");
    });
    out.textures_delta.clear();
    // Each interface font on its own (no fallback to egui's fonts).
    let fonts: Vec<egui::FontId> = win95::theme::Font::ALL
        .iter()
        .map(|f| egui::FontId::new(win95::theme::FONT_SIZE, f.family()))
        .collect();
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
                // The diff view uses the monospace font, everything else the interface fonts.
                let ids: Vec<&egui::FontId> = if file_is_diff {
                    vec![&mono]
                } else {
                    fonts.iter().collect()
                };
                let ok = ids.iter().all(|id| ctx.fonts_mut(|f| f.has_glyph(id, c)));
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
