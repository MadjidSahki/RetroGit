use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;

fn click_at(h: &Harness<'_, String>, pos: egui::Pos2) {
    h.hover_at(pos);
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        });
    }
}

fn picking_right_of_the_text(selectable: bool, scroll: bool) {
    let mut h = Harness::new_ui_state(
        move |ui, picked: &mut String| {
            win95::combo_box(ui, "letters", "A", 180.0, |ui| {
                let mut items = |ui: &mut egui::Ui| {
                    for name in ["A", "B"] {
                        let r = if selectable {
                            ui.selectable_label(false, name)
                        } else {
                            ui.button(name)
                        };
                        if r.clicked() {
                            *picked = name.to_string();
                        }
                    }
                };
                if scroll {
                    egui::ScrollArea::vertical()
                        .max_height(300.0)
                        .show(ui, items);
                } else {
                    items(ui);
                }
            });
        },
        String::new(),
    );
    h.run();
    let combo = h.get_by_role(egui::accesskit::Role::ComboBox).rect();
    h.get_by_role(egui::accesskit::Role::ComboBox).click();
    h.run();
    let item = h.get_by_label("B").rect();
    // Far right of the one-letter text, still inside the 180 px wide popup.
    let x = combo.left() + 180.0 - 20.0;
    click_at(&h, egui::pos2(x, item.center().y));
    h.run();
    assert_eq!(h.state(), "B");
}

#[test]
fn clicking_right_of_a_selectable_item_text_picks_it() {
    picking_right_of_the_text(true, false);
}

#[test]
fn clicking_right_of_a_button_item_text_picks_it() {
    picking_right_of_the_text(false, false);
}

#[test]
fn clicking_right_of_an_item_text_in_a_scroll_area_picks_it() {
    picking_right_of_the_text(true, true);
}

#[test]
fn a_long_list_scrolls_with_the_wheel() {
    let mut h = Harness::builder()
        .with_size(egui::vec2(400.0, 400.0))
        .build_ui_state(
            |ui, picked: &mut String| {
                win95::combo_box(ui, "long", "item0", 180.0, |ui| {
                    for i in 0..60 {
                        let name = format!("item{i}");
                        if ui.button(&name).clicked() {
                            *picked = name;
                        }
                    }
                });
            },
            String::new(),
        );
    h.run();
    h.get_by_role(egui::accesskit::Role::ComboBox).click();
    h.run();
    let first = h.get_by_label("item1").rect();
    h.hover_at(first.center());
    h.event(egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: egui::vec2(0.0, -100_000.0),
        phase: egui::TouchPhase::Move,
        modifiers: egui::Modifiers::NONE,
    });
    h.run_steps(30);
    // The last branch can be reached: scrolled into the window, then picked.
    let last = h.get_by_label("item59").rect();
    assert!(last.bottom() <= 400.0, "item59 at {last:?}");
    click_at(&h, last.center());
    h.run();
    assert_eq!(h.state(), "item59");
}
