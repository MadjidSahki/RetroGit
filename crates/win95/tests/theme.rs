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

#[test]
fn hovered_widgets_use_white_text_on_navy() {
    let h = Harness::new_ui(|ui| {
        ui.label("x");
    });
    win95::theme::install(&h.ctx);
    let v = h.ctx.global_style().visuals.clone();
    // A global override would force black text on the navy hover background.
    assert_eq!(v.override_text_color, None);
    assert_eq!(v.widgets.hovered.text_color(), win95::theme::WHITE);
    assert_eq!(v.widgets.hovered.weak_bg_fill, win95::theme::NAVY);
    assert_eq!(v.text_color(), win95::theme::BLACK);
}
