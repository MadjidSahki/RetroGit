use egui::{Response, TextEdit, Ui};

use crate::bevel::Bevel;
use crate::panel::bevel_frame;
use crate::theme;

/// Single-line white sunken text box.
pub fn text_field(ui: &mut Ui, text: &mut String, width: f32, password: bool) -> Response {
    bevel_frame(ui, Bevel::Field, theme::palette(ui.ctx()).window, 1, |ui| {
        ui.add(
            TextEdit::singleline(text)
                .frame(egui::Frame::NONE)
                .password(password)
                .desired_width(width),
        )
    })
    .inner
}

#[cfg(test)]
mod tests {
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    #[test]
    fn typing_updates_the_string() {
        let mut h = Harness::new_ui_state(
            |ui, s: &mut String| {
                super::text_field(ui, s, 200.0, false);
            },
            String::new(),
        );
        h.get_by_role(egui::accesskit::Role::TextInput).focus();
        h.run();
        h.get_by_role(egui::accesskit::Role::TextInput)
            .type_text("octocat");
        h.run();
        assert_eq!(h.state(), "octocat");
    }
}
