//! Unified diff with per-line check boxes and per-hunk buttons.

use egui::{Color32, RichText, ScrollArea};
use gitcore::{Change, FileDiff, LineKind, Selection, Side};
use win95::{Bevel, Button95, bevel_frame, checkbox};

use super::Ctx;
use crate::protocol::Command;
use crate::state::LARGE_DIFF_LINES;
use crate::strings as s;

const ADDED_BG: Color32 = Color32::from_rgb(0xE6, 0xFF, 0xE6);
const REMOVED_BG: Color32 = Color32::from_rgb(0xFF, 0xE6, 0xE6);
const HUNK_BG: Color32 = Color32::from_rgb(0xE0, 0xE0, 0xF0);
const ROW_HEIGHT: f32 = 17.0;

/// One displayed row: a hunk header or a line of a hunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row {
    Hunk(usize),
    Line(usize, usize),
}

pub fn rows(diff: &FileDiff) -> Vec<Row> {
    let mut out = Vec::with_capacity(diff.line_count() + diff.hunks.len());
    for (h, hunk) in diff.hunks.iter().enumerate() {
        out.push(Row::Hunk(h));
        out.extend((0..hunk.lines.len()).map(|l| Row::Line(h, l)));
    }
    out
}

enum Action {
    Lines,
    Hunk(usize),
    File,
    ShowLarge,
}

pub fn show(ui: &mut egui::Ui, cx: &mut Ctx<'_>) {
    let mut action: Option<Action> = None;
    let c = &mut cx.state.changes;
    bevel_frame(ui, Bevel::Field, win95::theme::WHITE, 4, |ui| {
        ui.set_min_size(ui.available_size());
        let Some((path, side)) = c.shown.clone() else {
            let empty = c.files.is_empty();
            ui.label(if empty {
                s::WORKING_TREE_CLEAN
            } else {
                s::SELECT_A_FILE
            });
            return;
        };
        let (verb_lines, verb_hunk, verb_file, side_label) = match side {
            Side::Unstaged => (
                s::STAGE_LINES,
                s::STAGE_HUNK,
                s::STAGE_FILE,
                s::SIDE_UNSTAGED,
            ),
            Side::Staged => (
                s::UNSTAGE_LINES,
                s::UNSTAGE_HUNK,
                s::UNSTAGE_FILE,
                s::SIDE_STAGED,
            ),
        };
        let conflicted = c
            .files
            .iter()
            .any(|f| f.path == path && f.unstaged == Some(Change::Conflicted));
        // A staged rename can only be unstaged as a whole (both paths together).
        let whole_only = side == Side::Staged
            && c.files
                .iter()
                .any(|f| f.path == path && matches!(f.staged, Some(Change::Renamed { .. })));
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("{path} {side_label}")).strong());
            let has_lines = !c.selected_lines.is_empty();
            if whole_only {
                if ui.add(Button95::new(verb_file)).clicked() {
                    action = Some(Action::File);
                }
            } else if !conflicted
                && ui
                    .add(
                        Button95::new(verb_lines)
                            .min_size(egui::vec2(140.0, 20.0))
                            .enabled(has_lines),
                    )
                    .clicked()
            {
                action = Some(Action::Lines);
            }
        });
        ui.separator();
        if conflicted {
            ui.label(s::RESOLVE_CONFLICTS);
            return;
        }
        let Some(diff) = c.diff.as_ref() else { return };
        if diff.binary && !whole_only {
            ui.label(s::BINARY_FILE);
            if ui.add(Button95::new(verb_file)).clicked() {
                action = Some(Action::File);
            }
            return;
        }
        if diff.hunks.is_empty() {
            ui.label(s::NO_DIFF);
            return;
        }
        if diff.line_count() > LARGE_DIFF_LINES && !c.show_large {
            ui.label(s::DIFF_TOO_LARGE);
            if ui.add(Button95::new(s::SHOW_ANYWAY)).clicked() {
                action = Some(Action::ShowLarge);
            }
            return;
        }
        let all_rows = rows(diff);
        let mono = egui::FontId::monospace(win95::theme::FONT_SIZE);
        ScrollArea::both().auto_shrink([false, false]).show_rows(
            ui,
            ROW_HEIGHT,
            all_rows.len(),
            |ui, range| {
                ui.spacing_mut().item_spacing.y = 0.0;
                for row in &all_rows[range] {
                    match *row {
                        Row::Hunk(h) => {
                            egui::Frame::NONE.fill(HUNK_BG).show(ui, |ui| {
                                ui.set_min_width(ui.available_width());
                                ui.horizontal(|ui| {
                                    ui.set_height(ROW_HEIGHT);
                                    ui.label(
                                        RichText::new(&diff.hunks[h].header)
                                            .font(mono.clone())
                                            .color(win95::theme::NAVY),
                                    );
                                    if !whole_only
                                        && ui
                                            .add(
                                                Button95::new(verb_hunk)
                                                    .min_size(egui::vec2(90.0, 16.0)),
                                            )
                                            .clicked()
                                    {
                                        action = Some(Action::Hunk(h));
                                    }
                                });
                            });
                        }
                        Row::Line(h, l) => {
                            let line = &diff.hunks[h].lines[l];
                            let (bg, sign) = match line.kind {
                                LineKind::Added => (ADDED_BG, "+"),
                                LineKind::Removed => (REMOVED_BG, "-"),
                                LineKind::Context => (win95::theme::WHITE, " "),
                            };
                            egui::Frame::NONE.fill(bg).show(ui, |ui| {
                                ui.set_min_width(ui.available_width());
                                ui.horizontal(|ui| {
                                    ui.set_height(ROW_HEIGHT);
                                    if line.kind == LineKind::Context || whole_only {
                                        ui.add_space(13.0);
                                    } else {
                                        let mut on = c.selected_lines.contains(&(h, l));
                                        if checkbox(ui, &mut on, "").changed() {
                                            if on {
                                                c.selected_lines.insert((h, l));
                                            } else {
                                                c.selected_lines.remove(&(h, l));
                                            }
                                        }
                                    }
                                    let num = |n: Option<u32>| {
                                        n.map(|v| format!("{v:>5}"))
                                            .unwrap_or_else(|| "     ".into())
                                    };
                                    let text = line.text.trim_end_matches(['\n', '\r']);
                                    let eof = if line.no_newline_at_eof {
                                        "  ⏎̸"
                                    } else {
                                        ""
                                    };
                                    ui.label(
                                        RichText::new(format!(
                                            "{} {} {sign} {text}{eof}",
                                            num(line.old_no),
                                            num(line.new_no)
                                        ))
                                        .font(mono.clone())
                                        .color(win95::theme::BLACK),
                                    );
                                });
                            });
                        }
                    }
                }
            },
        );
    });

    let Some((path, side)) = cx.state.changes.shown.clone() else {
        return;
    };
    let shown = cx.state.changes.diff.clone();
    let selection = match action {
        None => return,
        Some(Action::ShowLarge) => {
            cx.state.changes.show_large = true;
            return;
        }
        Some(Action::Lines) => {
            Selection::Lines(cx.state.changes.selected_lines.iter().copied().collect())
        }
        Some(Action::Hunk(h)) => Selection::Hunks(vec![h]),
        Some(Action::File) => Selection::All,
    };
    // Whole-file actions touch both paths of a rename.
    let paths = match (
        &selection,
        cx.state.changes.files.iter().find(|f| f.path == path),
    ) {
        (Selection::All, Some(file)) => super::changes::paths_of(file, side),
        _ => vec![path],
    };
    for path in paths {
        let (selection, shown) = (selection.clone(), shown.clone());
        let cmd = match side {
            Side::Unstaged => Command::Stage {
                path,
                selection,
                shown,
            },
            Side::Staged => Command::Unstage {
                path,
                selection,
                shown,
            },
        };
        cx.worker.send(cmd);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gitcore::{DiffLine, Hunk};

    fn line(kind: LineKind) -> DiffLine {
        DiffLine {
            kind,
            old_no: None,
            new_no: None,
            text: "x\n".into(),
            raw: b"x\n".to_vec(),
            no_newline_at_eof: false,
        }
    }

    #[test]
    fn rows_interleave_hunk_headers_and_lines() {
        let hunk = |n| Hunk {
            header: "@@".into(),
            old_start: 1,
            old_lines: 1,
            new_start: 1,
            new_lines: 1,
            lines: (0..n).map(|_| line(LineKind::Context)).collect(),
        };
        let d = FileDiff {
            path: "f".into(),
            side: Side::Unstaged,
            binary: false,
            hunks: vec![hunk(2), hunk(1)],
        };
        assert_eq!(
            rows(&d),
            vec![
                Row::Hunk(0),
                Row::Line(0, 0),
                Row::Line(0, 1),
                Row::Hunk(1),
                Row::Line(1, 0)
            ]
        );
    }
}
