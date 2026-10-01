//! Conflict editor: Mine | Result (editable) | Theirs, with a choice per block.

use egui::{Color32, Panel, RichText, ScrollArea, TextEdit};
use gitcore::{Choice, ConflictKind, Operation, Pane, Pick, Segment, block_lines, parse_conflicts};
use win95::{Bevel, Button95, Dialog, bevel_frame};

use super::Ctx;
use crate::protocol::Command;
use crate::state::{ConflictConfirm, ConflictEditor};
use crate::strings as s;

const ROW: f32 = 17.0;
const BLOCK_BG: Color32 = Color32::from_rgb(0xFF, 0xF6, 0xC8);
const CURRENT_BG: Color32 = Color32::from_rgb(0xFF, 0xD8, 0x90);
const BASE_FG: Color32 = Color32::from_rgb(0x80, 0x80, 0x80);

/// Titles of the mine and theirs panes: Git swaps "ours" and "theirs" during a rebase.
pub fn pane_titles(op: Option<Operation>, segments: &[Segment]) -> (String, String) {
    let (ml, tl) = segments
        .iter()
        .find_map(|seg| match seg {
            Segment::Conflict {
                mine_label,
                theirs_label,
                ..
            } => Some((mine_label.clone(), theirs_label.clone())),
            Segment::Common(_) => None,
        })
        .unwrap_or_default();
    let with = |name: &str, label: &str| {
        if label.is_empty() {
            name.to_string()
        } else {
            format!("{name} ({label})")
        }
    };
    match op {
        Some(Operation::Rebase) => (with(s::PANE_UPSTREAM, &ml), with(s::PANE_YOUR_COMMIT, &tl)),
        _ => (with(s::PANE_MINE, &ml), with(s::PANE_THEIRS, &tl)),
    }
}

/// "1 conflict left", "3 conflicts left".
pub fn conflicts_left_text(n: usize) -> String {
    if n == 1 {
        format!("1 {}", s::CONFLICT_LEFT)
    } else {
        format!("{n} {}", s::CONFLICTS_LEFT)
    }
}

/// Background of each line of `text`: conflict blocks pale, the current block stronger.
pub fn line_backgrounds(blocks: &[(usize, usize)], current: usize, lines: usize) -> Vec<Color32> {
    let mut out = vec![Color32::TRANSPARENT; lines];
    for (i, (start, count)) in blocks.iter().enumerate() {
        let color = if i == current { CURRENT_BG } else { BLOCK_BG };
        let end = (start + count).min(lines);
        if *start < end {
            out[*start..end].fill(color);
        }
    }
    out
}

pub fn show(ui: &mut egui::Ui, cx: &mut Ctx<'_>) {
    let c = &mut cx.state.changes;
    if c.load_conflict
        && let Some(path) = c.conflict_path.clone()
    {
        c.load_conflict = false;
        cx.worker.send(Command::LoadConflict(path));
    }
    confirm_dialog(ui.ctx(), cx);
    bevel_frame(ui, Bevel::Field, win95::theme::WHITE, 4, |ui| {
        ui.set_min_size(ui.available_size());
        // Taken out while drawing (no copy of large files every frame), then put back.
        let Some(mut ed) = cx.state.changes.conflict.take() else {
            ui.label(s::LOADING_CONFLICT);
            return;
        };
        match ed.file.kind {
            ConflictKind::Content | ConflictKind::AddedByBoth => content(ui, cx, &mut ed),
            _ => whole_file_only(ui, cx, &ed),
        }
        if cx.state.changes.conflict.is_none() {
            cx.state.changes.conflict = Some(ed);
        }
    });
}

/// Binary files, and files deleted on one side: keep one version, or the deletion.
fn whole_file_only(ui: &mut egui::Ui, cx: &mut Ctx<'_>, ed: &ConflictEditor) {
    let path = ed.file.path.clone();
    ui.label(RichText::new(&path).color(win95::theme::NAVY));
    let (question, choices): (&str, Vec<(&str, Command)>) = match ed.file.kind {
        ConflictKind::DeletedByUs => (
            s::CONFLICT_DELETED_BY_US,
            vec![
                (
                    s::KEEP_FILE,
                    Command::ResolveConflictWith {
                        path: path.clone(),
                        pick: Pick::Theirs,
                    },
                ),
                (s::DELETE_FILE, Command::ResolveDelete(path.clone())),
            ],
        ),
        ConflictKind::DeletedByThem => (
            s::CONFLICT_DELETED_BY_THEM,
            vec![
                (
                    s::KEEP_FILE,
                    Command::ResolveConflictWith {
                        path: path.clone(),
                        pick: Pick::Ours,
                    },
                ),
                (s::DELETE_FILE, Command::ResolveDelete(path.clone())),
            ],
        ),
        _ => (
            s::CONFLICT_BINARY,
            vec![
                (
                    s::KEEP_MINE,
                    Command::ResolveConflictWith {
                        path: path.clone(),
                        pick: Pick::Ours,
                    },
                ),
                (
                    s::TAKE_THEIRS,
                    Command::ResolveConflictWith {
                        path: path.clone(),
                        pick: Pick::Theirs,
                    },
                ),
            ],
        ),
    };
    ui.label(question);
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        for (label, cmd) in choices {
            if ui
                .add(Button95::new(label).min_size(egui::vec2(140.0, 23.0)))
                .clicked()
            {
                cx.worker.send(cmd);
            }
        }
    });
}

fn content(ui: &mut egui::Ui, cx: &mut Ctx<'_>, ed: &mut ConflictEditor) {
    let segments = parse_conflicts(&ed.result);
    let left = ed.conflicts_left();
    let (mine_title, theirs_title) = pane_titles(ed.file.operation, &segments);
    // Toolbar.
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(&ed.file.path).color(win95::theme::NAVY));
        ui.label(conflicts_left_text(left));
        let b = |t| Button95::new(t).min_size(egui::vec2(70.0, 20.0));
        if ui
            .add(b(s::PREV_CONFLICT).enabled(ed.current > 0))
            .clicked()
        {
            ed.previous();
        }
        if ui
            .add(b(s::NEXT_CONFLICT).enabled(ed.current + 1 < left))
            .clicked()
        {
            ed.next();
        }
        ui.separator();
        for (label, choice) in [
            (s::USE_MINE, Choice::Mine),
            (s::USE_THEIRS, Choice::Theirs),
            (s::USE_BOTH, Choice::Both),
        ] {
            if ui.add(b(label).enabled(left > 0)).clicked() {
                ed.choose(choice);
            }
        }
        ui.separator();
        ui.label(s::WHOLE_FILE);
        if ui.add(b(s::WHOLE_MINE)).clicked() {
            ed.confirm = Some(ConflictConfirm::WholeFile(Pick::Ours));
        }
        if ui.add(b(s::WHOLE_THEIRS)).clicked() {
            ed.confirm = Some(ConflictConfirm::WholeFile(Pick::Theirs));
        }
        if let Some(repo) = cx.state.current.as_ref().map(|c| c.path.clone())
            && let Some(ide) = crate::ide::ide_for(&cx.state.config, &cx.state.ides, &repo)
            && ui.add(b(s::OPEN_IN_IDE_SHORT)).clicked()
            && let Err(e) = crate::ide::open(ide, &repo)
        {
            log::warn!("cannot open the IDE: {e}");
        }
    });
    if ed.on_disk.is_some() {
        egui::Frame::NONE
            .fill(Color32::from_rgb(0xFF, 0xFF, 0xC0))
            .inner_margin(egui::Margin::same(3))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(s::CHANGED_ON_DISK).color(win95::theme::BLACK));
                    if ui.add(Button95::new(s::RELOAD)).clicked() {
                        ed.reload();
                    }
                    if ui.add(Button95::new(s::KEEP_MY_EDITS)).clicked() {
                        ed.on_disk = None;
                    }
                });
            });
    }
    Panel::bottom("conflict_actions")
        .frame(egui::Frame::NONE.inner_margin(egui::Margin::same(2)))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                if ui
                    .add(Button95::new(s::MARK_RESOLVED).min_size(egui::vec2(120.0, 23.0)))
                    .clicked()
                {
                    if gitcore::has_marker_lines(&ed.result) {
                        ed.confirm = Some(ConflictConfirm::ResolveWithMarkers);
                    } else {
                        cx.worker.send(Command::ResolveConflict {
                            path: ed.file.path.clone(),
                            content: ed.result.clone(),
                        });
                    }
                }
            });
        });
    request_colors(cx, ed);
    let width = ui.available_width();
    let mine_text = ed.file.mine.clone().unwrap_or_default();
    let theirs_text = ed.file.theirs.clone().unwrap_or_default();
    let current = ed.current;
    Panel::left("conflict_mine")
        .frame(egui::Frame::NONE)
        .resizable(true)
        .default_size(width / 3.0)
        .min_size(80.0)
        .show(ui, |ui| {
            side_pane(
                ui,
                "mine",
                &mine_title,
                &mine_text,
                &block_lines(&segments, Pane::Mine),
                current,
                &ed.mine_colors,
            )
        });
    Panel::right("conflict_theirs")
        .frame(egui::Frame::NONE)
        .resizable(true)
        .default_size(width / 3.0)
        .min_size(80.0)
        .show(ui, |ui| {
            side_pane(
                ui,
                "theirs",
                &theirs_title,
                &theirs_text,
                &block_lines(&segments, Pane::Theirs),
                current,
                &ed.theirs_colors,
            )
        });
    egui::CentralPanel::default()
        .frame(egui::Frame::NONE.inner_margin(egui::Margin::symmetric(4, 0)))
        .show(ui, |ui| result_pane(ui, ed, &segments));
}

/// Read-only file with the current block's lines highlighted and scrolled into view.
fn side_pane(
    ui: &mut egui::Ui,
    id: &str,
    title: &str,
    text: &str,
    blocks: &[(usize, usize)],
    current: usize,
    colors: &crate::highlight::Colors,
) {
    ui.label(RichText::new(title).color(win95::theme::NAVY));
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let bg = line_backgrounds(blocks, current, lines.len());
    let mono = egui::FontId::monospace(win95::theme::FONT_SIZE);
    let mut area = ScrollArea::both()
        .id_salt(("conflict_side", id))
        .auto_shrink([false, false]);
    let target = blocks.get(current).map(|(start, _)| *start as f32 * ROW);
    let key = egui::Id::new(("conflict_scrolled", id));
    if ui.ctx().data(|d| d.get_temp::<Option<f32>>(key)) != Some(target) {
        if let Some(y) = target {
            area = area.vertical_scroll_offset((y - 3.0 * ROW).max(0.0));
        }
        ui.ctx().data_mut(|d| d.insert_temp(key, target));
    }
    area.show_rows(ui, ROW, lines.len(), |ui, range| {
        ui.spacing_mut().item_spacing.y = 0.0;
        for i in range {
            let job = crate::highlight::colored_line(
                &format!("{:>5} ", i + 1),
                lines[i].trim_end_matches(['\n', '\r']),
                colors.line(0, i),
                "",
                mono.clone(),
                Color32::TRANSPARENT,
            );
            let bg = if bg[i] == Color32::TRANSPARENT {
                win95::theme::WHITE
            } else {
                bg[i]
            };
            crate::highlight::diff_row(ui, job, ROW, bg);
        }
    });
}

/// The editable result, conflict blocks highlighted (the ancestor's lines in gray).
fn result_pane(ui: &mut egui::Ui, ed: &mut ConflictEditor, segments: &[Segment]) {
    ui.label(RichText::new(s::PANE_RESULT).color(win95::theme::NAVY));
    let blocks = block_lines(segments, Pane::Result);
    let current = ed.current;
    let base_lines = base_line_set(segments);
    let colors = ed.result_colors_shown.clone();
    let mut text = ed.result.clone();
    let mut layouter = |ui: &egui::Ui, buf: &dyn egui::TextBuffer, wrap: f32| {
        let s = buf.as_str();
        let lines: Vec<&str> = s.split_inclusive('\n').collect();
        let bg = line_backgrounds(&blocks, current, lines.len().max(1));
        let mut job = egui::text::LayoutJob::default();
        let font = egui::FontId::monospace(win95::theme::FONT_SIZE);
        let fmt = |color: Color32, background: Color32| egui::TextFormat {
            font_id: font.clone(),
            color,
            background,
            ..Default::default()
        };
        for (i, line) in lines.iter().enumerate() {
            let body = line.trim_end_matches(['\n', '\r']);
            let spans = colors
                .line(0, i)
                .filter(|sp| sp.iter().map(|x| x.text.as_str()).collect::<String>() == body);
            match spans {
                // Colors computed for this very line: drawn while new ones are computed.
                Some(spans) if !base_lines.contains(&i) => {
                    for span in spans {
                        job.append(&span.text, 0.0, fmt(span.color, bg[i]));
                    }
                    job.append(&line[body.len()..], 0.0, fmt(win95::theme::BLACK, bg[i]));
                }
                _ => {
                    let color = if base_lines.contains(&i) {
                        BASE_FG
                    } else {
                        win95::theme::BLACK
                    };
                    job.append(line, 0.0, fmt(color, bg[i]));
                }
            }
        }
        job.wrap.max_width = wrap;
        ui.fonts_mut(|f| f.layout_job(job))
    };
    ScrollArea::both()
        .id_salt("conflict_result")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.add(
                TextEdit::multiline(&mut text)
                    .code_editor()
                    .desired_width(f32::INFINITY)
                    .desired_rows(20)
                    .layouter(&mut layouter),
            )
        });
    ed.edit(text);
}

/// Ask the background highlighter for the panes whose colors are missing.
fn request_colors(cx: &Ctx<'_>, ed: &mut ConflictEditor) {
    use crate::highlight::{Colors, Target};
    let path = ed.file.path.clone();
    let panes = [
        (
            Target::ConflictMine,
            ed.file.mine.clone(),
            &mut ed.mine_colors,
        ),
        (
            Target::ConflictTheirs,
            ed.file.theirs.clone(),
            &mut ed.theirs_colors,
        ),
        (
            Target::ConflictResult,
            Some(ed.result.clone()),
            &mut ed.result_colors,
        ),
    ];
    for (target, text, slot) in panes {
        if *slot == Colors::NotRequested
            && let Some(text) = text
        {
            cx.highlighter
                .request(target, crate::state::text_as_diff(&path, &text));
            *slot = Colors::Pending;
        }
    }
}

/// Lines of `diff3` ancestor sections (shown in gray).
fn base_line_set(segments: &[Segment]) -> std::collections::HashSet<usize> {
    let mut out = std::collections::HashSet::new();
    let mut line = 0;
    for seg in segments {
        match seg {
            Segment::Common(t) => line += t.split_inclusive('\n').count(),
            Segment::Conflict {
                mine, base, raw, ..
            } => {
                if let Some(b) = base {
                    // <<<<<<< + mine + ||||||| then the base lines.
                    let start = line + 1 + mine.split_inclusive('\n').count() + 1;
                    out.extend(start..start + b.split_inclusive('\n').count());
                }
                line += raw.split_inclusive('\n').count();
            }
        }
    }
    out
}

fn confirm_dialog(egui_ctx: &egui::Context, cx: &mut Ctx<'_>) {
    let Some(confirm) = cx
        .state
        .changes
        .conflict
        .as_ref()
        .and_then(|e| e.confirm.clone())
    else {
        return;
    };
    let question = match &confirm {
        ConflictConfirm::WholeFile(Pick::Ours) => s::CONFIRM_WHOLE_MINE,
        ConflictConfirm::WholeFile(Pick::Theirs) => s::CONFIRM_WHOLE_THEIRS,
        ConflictConfirm::ResolveWithMarkers => s::CONFIRM_MARKERS_LEFT,
        ConflictConfirm::Discard(_) | ConflictConfirm::Abort => s::CONFIRM_DISCARD_EDITS,
    };
    let (mut yes, mut no) = (false, false);
    let r = Dialog::new("conflict_confirm", s::CONFLICT_TITLE)
        .width(400.0)
        .show(egui_ctx, |ui| {
            ui.add(egui::Label::new(question).wrap());
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                yes = ui
                    .add(Button95::new(s::OK).min_size(egui::vec2(90.0, 23.0)))
                    .clicked();
                no = ui
                    .add(Button95::new(s::CANCEL).min_size(egui::vec2(90.0, 23.0)))
                    .clicked();
            });
        });
    let c = &mut cx.state.changes;
    if no || r.close_requested {
        if let Some(ed) = c.conflict.as_mut() {
            ed.confirm = None;
        }
        return;
    }
    if !yes {
        return;
    }
    let Some(ed) = c.conflict.as_mut() else {
        return;
    };
    let path = ed.file.path.clone();
    ed.confirm = None;
    match confirm {
        ConflictConfirm::WholeFile(pick) => {
            cx.worker.send(Command::ResolveConflictWith { path, pick })
        }
        ConflictConfirm::ResolveWithMarkers => cx.worker.send(Command::ResolveConflict {
            path,
            content: ed.result.clone(),
        }),
        ConflictConfirm::Discard(_) => {
            ed.confirm = Some(confirm);
            if let Some(next) = c.discard_conflict_edits() {
                cx.worker.send(Command::LoadConflict(next));
            }
        }
        ConflictConfirm::Abort => {
            c.conflict = None;
            c.conflict_path = None;
            cx.worker.send(Command::AbortOperation);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_follow_the_operation() {
        let segs = parse_conflicts("<<<<<<< HEAD\na\n=======\nb\n>>>>>>> feat/x\n");
        assert_eq!(
            pane_titles(Some(Operation::Merge), &segs),
            ("Mine (HEAD)".to_string(), "Theirs (feat/x)".to_string())
        );
        assert_eq!(
            pane_titles(Some(Operation::Rebase), &segs),
            (
                "Upstream (HEAD)".to_string(),
                "Your commit (feat/x)".to_string()
            )
        );
        assert_eq!(
            pane_titles(None, &[]),
            ("Mine".to_string(), "Theirs".to_string())
        );
    }

    #[test]
    fn blocks_are_highlighted_and_the_current_one_stands_out() {
        let bg = line_backgrounds(&[(1, 2), (4, 1)], 1, 6);
        assert_eq!(
            bg,
            [
                Color32::TRANSPARENT,
                BLOCK_BG,
                BLOCK_BG,
                Color32::TRANSPARENT,
                CURRENT_BG,
                Color32::TRANSPARENT
            ]
        );
    }

    #[test]
    fn ancestor_lines_of_diff3_blocks() {
        let segs =
            parse_conflicts("x\n<<<<<<< a\nm\n||||||| base\nb1\nb2\n=======\nt\n>>>>>>> c\n");
        let mut lines: Vec<usize> = base_line_set(&segs).into_iter().collect();
        lines.sort();
        assert_eq!(lines, [4, 5]);
    }
}
