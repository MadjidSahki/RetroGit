use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;

#[test]
fn installing_the_theme_renders_accented_text() {
    let mut h = Harness::new_ui(|ui| {
        ui.label("Clone a repository — éàç");
    });
    win95::theme::install(&h.ctx);
    h.run();
    assert!(h.query_by_label("Clone a repository — éàç").is_some());
    let style = h.ctx.global_style();
    assert_eq!(style.visuals.panel_fill, win95::theme::SILVER);
}
