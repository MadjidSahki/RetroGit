use egui::{Response, TextEdit, Ui};

use crate::bevel::Bevel;
use crate::panel::bevel_frame;
use crate::theme;

/// Multi-line white sunken text box, `rows` lines high.
pub fn text_area(ui: &mut Ui, text: &mut String, width: f32, rows: usize) -> Response {
    bevel_frame(ui, Bevel::Field, theme::palette(ui.ctx()).window, 1, |ui| {
        ui.add(
            TextEdit::multiline(text)
                .frame(egui::Frame::NONE)
                .desired_width(width)
                .desired_rows(rows),
        )
    })
    .inner
}

#[cfg(test)]
mod tests {
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    #[test]
    fn typing_multiple_lines() {
        let mut h = Harness::new_ui_state(
            |ui, s: &mut String| {
                super::text_area(ui, s, 300.0, 4);
            },
            String::new(),
        );
        h.get_by_role(egui::accesskit::Role::MultilineTextInput)
            .focus();
        h.run();
        h.get_by_role(egui::accesskit::Role::MultilineTextInput)
            .type_text("Fix bug");
        h.run();
        h.key_press(egui::Key::Enter);
        h.run();
        h.get_by_role(egui::accesskit::Role::MultilineTextInput)
            .type_text("Details");
        h.run();
        assert_eq!(h.state(), "Fix bug\nDetails");
    }
}
