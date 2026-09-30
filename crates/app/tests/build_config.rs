//! Guards against feature flags that silently disable user-facing behaviour.

const WORKSPACE_MANIFEST: &str = include_str!("../../../Cargo.toml");

#[test]
fn eframe_can_open_links_in_the_browser() {
    // Without eframe's "links" feature, `ctx.open_url` and every hyperlink only log
    // "Cannot open url" (Open browser button, SSO links).
    let eframe = WORKSPACE_MANIFEST
        .lines()
        .find(|l| l.trim_start().starts_with("eframe"))
        .unwrap_or_default();
    assert!(eframe.contains("\"links\""), "eframe features: {eframe}");
}

#[test]
fn ui_never_uses_egui_strong_text() {
    // In the Win95 theme, egui's "strong" color is the white used for hovered menu items:
    // `.strong()` text is white on silver, unreadable. Use an explicit color instead.
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ui");
    for entry in std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{e}")) {
        let path = entry.unwrap_or_else(|e| panic!("{e}")).path();
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        assert!(
            !text.contains(".strong()"),
            "{} uses .strong()",
            path.display()
        );
    }
}
