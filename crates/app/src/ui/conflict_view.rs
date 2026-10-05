//! Conflict editor: Mine | Result (editable) | Theirs, with a choice per block.

use egui::{Color32, Panel, RichText, ScrollArea, TextEdit};
#[cfg(test)]
use gitcore::parse_conflicts;
use gitcore::{Choice, ConflictKind, Operation, Pane, Pick, Segment, block_lines};
use win95::FlatRows;
use win95::{Bevel, Button95, Dialog, bevel_frame};

use super::Ctx;
use crate::protocol::Command;
use crate::state::{ConflictConfirm, ConflictEditor};
use crate::strings as s;

const ROW: f32 = 17.0;

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

/// How each side is called: during a rebase Git's "ours" is the upstream and "theirs"
/// the user's commit; after a stash, "theirs" is the user's stashed work.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SideNames {
    /// For buttons: "Use mine", "Use upstream"...
    pub mine: &'static str,
    pub theirs: &'static str,
    /// In sentences: "your version", "the upstream"...
    pub mine_long: &'static str,
    pub theirs_long: &'static str,
}

pub fn side_names(op: Option<Operation>, segments: &[Segment]) -> SideNames {
    let stash = segments.iter().any(|seg| {
        matches!(seg, Segment::Conflict { theirs_label, .. } if theirs_label == "Stashed changes")
    });
    match op {
        Some(Operation::Rebase) => SideNames {
            mine: s::SIDE_UPSTREAM,
            theirs: s::SIDE_MY_COMMIT,
            mine_long: s::SIDE_UPSTREAM_LONG,
            theirs_long: s::SIDE_MY_COMMIT_LONG,
        },
        _ if stash => SideNames {
            mine: s::SIDE_CURRENT,
            theirs: s::SIDE_MY_STASH,
            mine_long: s::SIDE_CURRENT_LONG,
            theirs_long: s::SIDE_MY_STASH_LONG,
        },
        _ => SideNames {
            mine: s::SIDE_MINE,
            theirs: s::SIDE_THEIRS,
            mine_long: s::SIDE_MINE_LONG,
            theirs_long: s::SIDE_THEIRS_LONG,
        },
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
pub fn line_backgrounds(
    pal: &win95::Palette,
    blocks: &[(usize, usize)],
    current: usize,
    lines: usize,
) -> Vec<Color32> {
    let mut out = vec![Color32::TRANSPARENT; lines];
    for (i, (start, count)) in blocks.iter().enumerate() {
        let color = if i == current {
            pal.conflict_current
        } else {
            pal.conflict_block
        };
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
    bevel_frame(
        ui,
        Bevel::Field,
        win95::theme::palette(ui.ctx()).window,
        4,
        |ui| {
            ui.set_min_size(ui.available_size());
            if let Some(err) = &cx.state.changes.conflict_error {
                let pal = win95::theme::palette(ui.ctx());
                ui.label(RichText::new(err.as_str()).color(pal.error));
                return;
            }
            // Taken out while drawing (no copy of large files every frame), then put back.
            let Some(mut ed) = cx.state.changes.conflict.take() else {
                ui.label(s::LOADING_CONFLICT);
                return;
            };
            match ed.file.kind {
                ConflictKind::Content | ConflictKind::AddedByBoth => content(ui, cx, &mut ed),
                _ => whole_file_only(ui, &mut ed),
            }
            if cx.state.changes.conflict.is_none() {
                cx.state.changes.conflict = Some(ed);
            }
        },
    );
}

/// Binary files, and files deleted on one side: keep one version, or the deletion (each
/// asked first, in [`confirm_dialog`]).
fn whole_file_only(ui: &mut egui::Ui, ed: &mut ConflictEditor) {
    let path = ed.file.path.clone();
    let names = side_names(ed.file.operation, &ed.segments);
    ui.label(RichText::new(&path).color(win95::theme::palette(ui.ctx()).link));
    let keep = ConflictConfirm::WholeFile;
    let (question, choices): (String, Vec<(String, ConflictConfirm)>) = match ed.file.kind {
        ConflictKind::DeletedByUs => (
            s::CONFLICT_DELETED_IN
                .replace("{deleted}", names.mine_long)
                .replace("{changed}", names.theirs_long),
            vec![
                (s::KEEP_FILE.to_string(), keep(Pick::Theirs)),
                (s::DELETE_FILE.to_string(), ConflictConfirm::DeleteFile),
            ],
        ),
        ConflictKind::DeletedByThem => (
            s::CONFLICT_DELETED_IN
                .replace("{deleted}", names.theirs_long)
                .replace("{changed}", names.mine_long),
            vec![
                (s::KEEP_FILE.to_string(), keep(Pick::Ours)),
                (s::DELETE_FILE.to_string(), ConflictConfirm::DeleteFile),
            ],
        ),
        _ => (
            s::CONFLICT_BINARY.to_string(),
            vec![
                (s::USE_SIDE.replace("{side}", names.mine), keep(Pick::Ours)),
                (
                    s::USE_SIDE.replace("{side}", names.theirs),
                    keep(Pick::Theirs),
                ),
            ],
        ),
    };
    ui.label(question);
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        for (label, confirm) in choices {
            if ui
                .add(
                    Button95::new(label)
                        .min_size(egui::vec2(140.0, 23.0))
                        .enabled(!ed.resolving),
                )
                .clicked()
            {
                ed.confirm = Some(confirm);
            }
        }
    });
}

fn content(ui: &mut egui::Ui, cx: &mut Ctx<'_>, ed: &mut ConflictEditor) {
    let left = ed.conflicts_left();
    let (mine_title, theirs_title) = pane_titles(ed.file.operation, &ed.segments);
    let names = side_names(ed.file.operation, &ed.segments);
    // Toolbar.
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(&ed.file.path).color(win95::theme::palette(ui.ctx()).link));
        ui.label(conflicts_left_text(left));
        let b = |t: &str| Button95::new(t.to_string()).min_size(egui::vec2(70.0, 20.0));
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
            (s::USE_SIDE.replace("{side}", names.mine), Choice::Mine),
            (s::USE_SIDE.replace("{side}", names.theirs), Choice::Theirs),
            (s::USE_BOTH.to_string(), Choice::Both),
        ] {
            if ui.add(b(&label).enabled(left > 0)).clicked() {
                ed.choose(choice);
            }
        }
        ui.separator();
        for (side, pick) in [(names.mine, Pick::Ours), (names.theirs, Pick::Theirs)] {
            if ui
                .add(b(&s::WHOLE_FILE_SIDE.replace("{side}", side)).enabled(!ed.resolving))
                .clicked()
            {
                ed.confirm = Some(ConflictConfirm::WholeFile(pick));
            }
        }
        if let Some(repo) = cx.state.current.as_ref().map(|c| c.path.clone())
            && let Some(ide) = crate::ide::ide_for(&cx.state.config, &cx.state.ides, &repo)
            && ui.add(b(s::OPEN_IN_IDE_SHORT)).clicked()
            && let Err(e) = crate::ide::open(ide, &repo, Some(std::path::Path::new(&ed.file.path)))
        {
            let mut err =
                crate::protocol::AppError::new(crate::protocol::Severity::Error, s::ERR_OPEN_IDE);
            err.detail = Some(format!("{}: {e}", ide.name));
            cx.state.messages.push_back(err);
        }
    });
    if ed.on_disk.is_some() {
        egui::Frame::NONE
            .fill(win95::theme::palette(ui.ctx()).note_bg)
            .inner_margin(egui::Margin::same(3))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(s::CHANGED_ON_DISK)
                            .color(win95::theme::palette(ui.ctx()).text),
                    );
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
                    .add(
                        Button95::new(s::MARK_RESOLVED)
                            .min_size(egui::vec2(120.0, 23.0))
                            .enabled(!ed.resolving),
                    )
                    .clicked()
                {
                    if gitcore::has_marker_lines(&ed.result) {
                        ed.confirm = Some(ConflictConfirm::ResolveWithMarkers);
                    } else {
                        let cmd = Command::ResolveConflict {
                            path: ed.file.path.clone(),
                            content: ed.content(),
                        };
                        cx.worker.send(ed.resolve(cmd));
                    }
                }
            });
        });
    for (target, diff) in ed.colors_to_request() {
        cx.highlighter.request(target, diff);
    }
    let width = ui.available_width();
    let mine_block = ed.current_side_block(Pane::Mine);
    let theirs_block = ed.current_side_block(Pane::Theirs);
    let path = ed.file.path.clone();
    Panel::left("conflict_mine")
        .frame(egui::Frame::NONE)
        .resizable(true)
        .default_size(width / 3.0)
        .min_size(80.0)
        .show(ui, |ui| {
            side_pane(
                ui,
                ("mine", &path),
                &mine_title,
                ed.file.mine.as_deref().unwrap_or_default(),
                mine_block,
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
                ("theirs", &path),
                &theirs_title,
                ed.file.theirs.as_deref().unwrap_or_default(),
                theirs_block,
                &ed.theirs_colors,
            )
        });
    egui::CentralPanel::default()
        .frame(egui::Frame::NONE.inner_margin(egui::Margin::symmetric(4, 0)))
        .show(ui, |ui| result_pane(ui, ed));
}

/// Read-only file with the current block's lines highlighted and scrolled into view.
fn side_pane(
    ui: &mut egui::Ui,
    id: (&str, &str),
    title: &str,
    text: &str,
    block: Option<(usize, usize)>,
    colors: &crate::highlight::Colors,
) {
    ui.label(RichText::new(title).color(win95::theme::palette(ui.ctx()).link));
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let pal = win95::theme::palette(ui.ctx());
    let bg = line_backgrounds(&pal, block.as_slice(), 0, lines.len());
    let mono = egui::FontId::monospace(win95::theme::FONT_SIZE);
    let mut area = ScrollArea::both()
        .id_salt(("conflict_side", id))
        .auto_shrink([false, false]);
    let target = block.map(|(start, _)| start as f32 * ROW);
    let key = egui::Id::new(("conflict_scrolled", id));
    if ui.ctx().data(|d| d.get_temp::<Option<f32>>(key)) != Some(target) {
        if let Some(y) = target {
            area = area.vertical_scroll_offset((y - 3.0 * ROW).max(0.0));
        }
        ui.ctx().data_mut(|d| d.insert_temp(key, target));
    }
    area.show_rows_flat(ui, ROW, lines.len(), |ui, range| {
        ui.spacing_mut().item_spacing.y = 0.0;
        for i in range {
            let job = crate::highlight::colored_line(
                &win95::theme::palette(ui.ctx()),
                &format!("{:>5} ", i + 1),
                lines[i].trim_end_matches(['\n', '\r']),
                colors.line(0, i),
                "",
                mono.clone(),
                Color32::TRANSPARENT,
            );
            let bg = if bg[i] == Color32::TRANSPARENT {
                pal.window
            } else {
                bg[i]
            };
            crate::highlight::diff_row(ui, job, ROW, bg);
        }
    });
}

/// The editable result, conflict blocks highlighted (the ancestor's lines in gray).
fn result_pane(ui: &mut egui::Ui, ed: &mut ConflictEditor) {
    ui.label(RichText::new(s::PANE_RESULT).color(win95::theme::palette(ui.ctx()).link));
    let blocks = block_lines(&ed.segments, Pane::Result);
    let current = ed.current;
    let base_lines = base_line_set(&ed.segments);
    let colors = std::mem::replace(&mut ed.result_colors_shown, crate::highlight::Colors::Plain);
    let path = ed.file.path.clone();
    let mut text = std::mem::take(&mut ed.result);
    let mut layouter = |ui: &egui::Ui, buf: &dyn egui::TextBuffer, wrap: f32| {
        let pal = win95::theme::palette(ui.ctx());
        let s = buf.as_str();
        let lines: Vec<&str> = s.split_inclusive('\n').collect();
        let bg = line_backgrounds(&pal, &blocks, current, lines.len().max(1));
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
            let spans = colors.line(0, i).filter(|sp| {
                let mut rest = body;
                sp.iter().all(|x| match rest.strip_prefix(x.text.as_str()) {
                    Some(r) => {
                        rest = r;
                        true
                    }
                    None => false,
                }) && rest.is_empty()
            });
            match spans {
                // Colors computed for this very line: drawn while new ones are computed.
                Some(spans) if !base_lines.contains(&i) => {
                    for span in spans {
                        job.append(&span.text, 0.0, fmt(span.color, bg[i]));
                    }
                    job.append(&line[body.len()..], 0.0, fmt(pal.window_text, bg[i]));
                }
                _ => {
                    let color = if base_lines.contains(&i) {
                        pal.gray_text
                    } else {
                        pal.window_text
                    };
                    job.append(line, 0.0, fmt(color, bg[i]));
                }
            }
        }
        job.wrap.max_width = wrap;
        ui.fonts_mut(|f| f.layout_job(job))
    };
    let mut area = ScrollArea::both()
        .id_salt(("conflict_result_scroll", &path))
        .auto_shrink([false, false]);
    // Opening, Next / Previous, a choice: scroll to the current block once (edits that
    // only move the block leave the view where the user put it).
    if std::mem::take(&mut ed.scroll_result)
        && let Some((start, _)) = blocks.get(current)
    {
        let row = ui.fonts_mut(|f| f.row_height(&egui::FontId::monospace(win95::theme::FONT_SIZE)));
        area = area.vertical_scroll_offset(((*start as f32 - 3.0) * row).max(0.0));
    }
    let output = area
        .show(ui, |ui| {
            TextEdit::multiline(&mut text)
                // One undo history per file: undo never brings another file's text.
                .id_salt(("conflict_result", &path))
                .code_editor()
                .desired_width(f32::INFINITY)
                .desired_rows(20)
                .layouter(&mut layouter)
                .show(ui)
        })
        .inner;
    ed.result_colors_shown = colors;
    if output.response.changed() {
        ed.typed(text);
    } else {
        ed.result = text;
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

/// Questions of the conflict editor (drawn by the main window, whatever the tab).
pub fn confirm_dialog(egui_ctx: &egui::Context, cx: &mut Ctx<'_>) {
    let Some(confirm) = cx
        .state
        .changes
        .conflict
        .as_ref()
        .and_then(|e| e.confirm.clone())
    else {
        return;
    };
    let names = cx
        .state
        .changes
        .conflict
        .as_ref()
        .map(|e| side_names(e.file.operation, &e.segments))
        .unwrap_or_else(|| side_names(None, &[]));
    let question = match &confirm {
        ConflictConfirm::WholeFile(Pick::Ours) => s::CONFIRM_WHOLE_SIDE
            .replace("{kept}", names.mine_long)
            .replace("{dropped}", names.theirs_long),
        ConflictConfirm::WholeFile(Pick::Theirs) => s::CONFIRM_WHOLE_SIDE
            .replace("{kept}", names.theirs_long)
            .replace("{dropped}", names.mine_long),
        ConflictConfirm::ResolveWithMarkers => s::CONFIRM_MARKERS_LEFT.to_string(),
        ConflictConfirm::Discard(_) | ConflictConfirm::OpenRepo(_) => {
            s::CONFIRM_DISCARD_EDITS.to_string()
        }
        ConflictConfirm::Abort => s::CONFIRM_ABORT_EDITS.to_string(),
        ConflictConfirm::DeleteFile => {
            let (path, changed) = cx
                .state
                .changes
                .conflict
                .as_ref()
                .map(|e| {
                    let changed = match e.file.kind {
                        ConflictKind::DeletedByUs => names.theirs_long,
                        _ => names.mine_long,
                    };
                    (e.file.path.clone(), changed)
                })
                .unwrap_or_default();
            s::CONFIRM_DELETE_FILE
                .replace("{path}", &path)
                .replace("{changed}", changed)
        }
    };
    let (mut yes, mut no) = (false, false);
    let r = Dialog::new("conflict_confirm", s::CONFLICT_TITLE)
        .width(400.0)
        .show(egui_ctx, |ui| {
            ui.add(egui::Label::new(question.as_str()).wrap());
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
            let cmd = Command::ResolveConflictWith { path, pick };
            cx.worker.send(ed.resolve(cmd));
        }
        ConflictConfirm::ResolveWithMarkers => {
            let cmd = Command::ResolveConflict {
                path,
                content: ed.content(),
            };
            cx.worker.send(ed.resolve(cmd));
        }
        ConflictConfirm::Discard(_) => {
            ed.confirm = Some(confirm);
            if let Some(cmd) = c.discard_conflict_edits() {
                cx.worker.send(cmd);
            }
        }
        ConflictConfirm::DeleteFile => {
            cx.worker.send(ed.resolve(Command::ResolveDelete(path)));
        }
        ConflictConfirm::Abort => {
            c.conflict = None;
            c.conflict_path = None;
            cx.worker.send(Command::AbortOperation);
        }
        ConflictConfirm::OpenRepo(path) => {
            // The editor closes only when the switch goes (pending line comments ask first;
            // Cancel there keeps the edits).
            if let Some(cmd) = cx.state.request_repo_switch(Command::OpenRepo(path)) {
                let c = &mut cx.state.changes;
                c.conflict = None;
                c.conflict_path = None;
                cx.worker.send(cmd);
            }
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
        let p = win95::palette::STANDARD;
        let bg = line_backgrounds(&p, &[(1, 2), (4, 1)], 1, 6);
        let (block, cur) = (p.conflict_block, p.conflict_current);
        assert_eq!(
            bg,
            [
                Color32::TRANSPARENT,
                block,
                block,
                Color32::TRANSPARENT,
                cur,
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
