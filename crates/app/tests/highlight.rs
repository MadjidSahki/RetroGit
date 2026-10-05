#![allow(clippy::unwrap_used)]
//! Background highlighting: the time budget is injected, so giving up is deterministic.

use std::time::Duration;

use gitcore::{DiffLine, FileDiff, Hunk, LineKind, Side};
use retrogit::highlight::{Service, Target};

fn added(t: &str) -> DiffLine {
    DiffLine {
        kind: LineKind::Added,
        old_no: None,
        new_no: None,
        text: t.into(),
        raw: t.as_bytes().to_vec(),
        no_newline_at_eof: false,
    }
}

fn rust_file() -> FileDiff {
    FileDiff {
        path: "a.rs".into(),
        side: Side::Unstaged,
        binary: false,
        hunks: vec![Hunk {
            header: "@@".into(),
            old_start: 1,
            old_lines: 0,
            new_start: 1,
            new_lines: 2,
            lines: vec![added("fn main() {\n"), added("}\n")],
        }],
    }
}

fn first_result(budget: Duration) -> retrogit::highlight::Highlighted {
    let (tx, rx) = std::sync::mpsc::channel();
    let service = Service::start_with_budget(budget, move |r| {
        let _ = tx.send(r);
    });
    service.request(Target::Changes, rust_file());
    rx.recv_timeout(Duration::from_secs(30)).unwrap()
}

#[test]
fn a_diff_over_its_time_budget_stays_plain_text() {
    let r = first_result(Duration::ZERO);
    assert_eq!(r.diff, rust_file());
    assert!(r.colors.is_none(), "gives up and stays plain");
}

#[test]
fn the_same_diff_within_its_budget_is_colored() {
    let r = first_result(Duration::from_secs(3600));
    assert!(r.colors.is_some());
}

#[test]
fn tab_goes_past_diff_rows_to_the_next_control() {
    let mut h = egui_kittest::Harness::new_ui_state(
        |ui, button: &mut Option<egui::Id>| {
            for t in ["fn a() {}", "fn b() {}"] {
                let job = egui::text::LayoutJob::simple_singleline(
                    t.into(),
                    egui::FontId::monospace(12.0),
                    egui::Color32::BLACK,
                );
                retrogit::highlight::diff_row(ui, job, 16.0, egui::Color32::WHITE);
            }
            *button = Some(ui.button("Next").id);
        },
        None,
    );
    h.run();
    h.key_press(egui::Key::Tab);
    h.run();
    let button = *h.state();
    assert_eq!(h.ctx.memory(|m| m.focused()), button);
}
