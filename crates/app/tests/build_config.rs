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
