//! History tab: commit graph list on top, commit detail below.

use egui::{Align2, Color32, Pos2, Rect, RichText, ScrollArea, Sense, Stroke, pos2, vec2};
use gitcore::{GraphRow, LineKind, RefKind, SignatureStatus};
use win95::{Bevel, bevel_frame, splitter};

use super::Ctx;
use crate::format::format_epoch;
use crate::protocol::Command;
use crate::state::HistoryAction;
use crate::strings as s;

pub const ROW_HEIGHT: f32 = 20.0;
pub const LANE_WIDTH: f32 = 14.0;
/// Load the next page when this close to the end of the list.
const PREFETCH_ROWS: usize = 50;

/// Color of graph lane `i` in `palette`.
pub fn lane_color(palette: &win95::Palette, i: usize) -> Color32 {
    palette.lanes[i % palette.lanes.len()]
}

/// Center x of lane `col` in a graph area starting at `left`.
pub fn lane_x(left: f32, col: usize) -> f32 {
    left + LANE_WIDTH / 2.0 + col as f32 * LANE_WIDTH
}

/// "5m ago", "3h ago", "2d ago", or the date for older commits.
pub fn relative_time(now: i64, t: i64) -> String {
    let d = (now - t).max(0);
    match d {
        0..60 => "just now".into(),
        60..3_600 => format!("{}m ago", d / 60),
        3_600..86_400 => format!("{}h ago", d / 3_600),
        86_400..2_592_000 => format!("{}d ago", d / 86_400),
        _ => format_epoch(t)[..10].to_string(),
    }
}

fn paint_graph(painter: &egui::Painter, row: &GraphRow, left: f32, rect: Rect) {
    let pal = win95::theme::palette(painter.ctx());
    let (top, mid, bottom) = (rect.top(), rect.center().y, rect.bottom());
    let line = |a: Pos2, b: Pos2, c: usize| {
        painter.line_segment([a, b], Stroke::new(2.0, lane_color(&pal, c)))
    };
    for e in &row.up {
        line(
            pos2(lane_x(left, e.from), top),
            pos2(lane_x(left, e.to), mid),
            e.color,
        );
    }
    for e in &row.down {
        line(
            pos2(lane_x(left, e.from), mid),
            pos2(lane_x(left, e.to), bottom),
            e.color,
        );
    }
    let c = pos2(lane_x(left, row.column), mid);
    painter.circle_filled(c, 4.5, lane_color(&pal, row.color));
    painter.circle_stroke(c, 4.5, Stroke::new(1.0, pal.window));
}

pub fn show(ui: &mut egui::Ui, cx: &mut Ctx<'_>) {
    let mut split = cx.state.history.split;
    splitter(
        ui,
        "history_split",
        &mut split,
        cx,
        |ui, cx| list(ui, cx),
        |ui, cx| detail(ui, cx),
    );
    cx.state.history.split = split;
}

fn list(ui: &mut egui::Ui, cx: &mut Ctx<'_>) {
    super::explore::history_filter(ui, cx);
    bevel_frame(
        ui,
        Bevel::Field,
        win95::theme::palette(ui.ctx()).window,
        1,
        |ui| {
            ui.set_min_size(ui.available_size());
            let h = &cx.state.history;
            // A filter shows the commits it found, without the graph (it would not connect).
            let filtered = h.filter.results.is_some();
            let entries = h.shown();
            if entries.is_empty() && !filtered {
                ui.label(if h.loading {
                    s::LOADING_HISTORY
                } else {
                    s::NO_HISTORY
                });
                return;
            }
            let lanes = h
                .graph
                .iter()
                .map(GraphRow::width)
                .max()
                .unwrap_or(1)
                .min(12);
            let graph_w = if filtered {
                4.0
            } else {
                lanes as f32 * LANE_WIDTH + 6.0
            };
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            let font = win95::theme::font(win95::theme::FONT_SIZE);
            let mut clicked: Option<String> = None;
            let mut menu: Option<(HistoryAction, gitcore::LogEntry)> = None;
            let mut last_visible = 0;
            ScrollArea::vertical()
                .auto_shrink([false, false])
                .show_rows(ui, ROW_HEIGHT, entries.len(), |ui, range| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    for i in range {
                        last_visible = i;
                        let e = &entries[i];
                        let (rect, resp) = ui.allocate_exact_size(
                            vec2(ui.available_width(), ROW_HEIGHT),
                            Sense::click(),
                        );
                        let selected = h.selected.as_deref() == Some(e.id.as_str());
                        let p = ui.painter();
                        let pal = win95::theme::palette(ui.ctx());
                        if selected {
                            p.rect_filled(rect, 0.0, pal.selection);
                        }
                        if !filtered && let Some(row) = h.graph.get(i) {
                            paint_graph(&p.with_clip_rect(rect), row, rect.left(), rect);
                        }
                        let text_color = if selected {
                            pal.selection_text
                        } else {
                            pal.window_text
                        };
                        let mut x = rect.left() + graph_w;
                        let y = rect.center().y;
                        let mono = egui::FontId::monospace(win95::theme::FONT_SIZE);
                        let id_rect = p.text(
                            pos2(x, y),
                            Align2::LEFT_CENTER,
                            &e.short_id,
                            mono,
                            text_color,
                        );
                        x = id_rect.right() + 8.0;
                        for r in &e.refs {
                            let bg = match r.kind {
                                RefKind::Head => pal.ref_head,
                                RefKind::LocalBranch => pal.ref_branch,
                                RefKind::RemoteBranch => pal.ref_remote,
                                RefKind::Tag => pal.ref_tag,
                            };
                            let fg = pal.window_text;
                            let g = p.layout_no_wrap(r.name.clone(), font.clone(), fg);
                            let tag =
                                Rect::from_min_size(pos2(x, y - 8.0), vec2(g.size().x + 8.0, 16.0));
                            p.rect_filled(tag, 0.0, bg);
                            p.rect_stroke(
                                tag,
                                0.0,
                                Stroke::new(1.0, pal.shadow),
                                egui::StrokeKind::Inside,
                            );
                            p.galley(pos2(x + 4.0, y - g.size().y / 2.0), g, fg);
                            x = tag.right() + 4.0;
                        }
                        let right = format!("{}  {}", e.author, relative_time(now, e.time));
                        let right_rect = p.text(
                            rect.right_center() - vec2(6.0, 0.0),
                            Align2::RIGHT_CENTER,
                            &right,
                            font.clone(),
                            text_color,
                        );
                        p.with_clip_rect(Rect::from_min_max(
                            pos2(x, rect.top()),
                            pos2(right_rect.left() - 8.0, rect.bottom()),
                        ))
                        .text(
                            pos2(x + 2.0, y),
                            Align2::LEFT_CENTER,
                            &e.summary,
                            font.clone(),
                            text_color,
                        );
                        if resp.clicked() {
                            clicked = Some(e.id.clone());
                        }
                        resp.context_menu(|ui| {
                            for (action, label) in [
                                (HistoryAction::CherryPick, s::MENU_CHERRY_PICK),
                                (HistoryAction::Revert, s::MENU_REVERT),
                                (HistoryAction::Reset, s::MENU_RESET),
                                (HistoryAction::RebaseFrom, s::MENU_REBASE_FROM),
                                (HistoryAction::CreateTag, s::MENU_CREATE_TAG),
                                (HistoryAction::Browse, s::MENU_BROWSE),
                            ] {
                                if ui.button(label).clicked() {
                                    menu = Some((action, e.clone()));
                                    ui.close();
                                }
                            }
                        });
                    }
                });
            let need_more = !filtered
                && last_visible + PREFETCH_ROWS >= cx.state.history.entries.len()
                && cx.state.wants_more_history();
            if need_more {
                cx.state.history.loading = true;
                cx.worker.send(Command::LoadLog {
                    skip: cx.state.history.entries.len(),
                });
            }
            if let Some((action, e)) = menu {
                if let Some(cmd) = cx.state.history_action(action, &e) {
                    cx.worker.send(cmd);
                }
                if action == HistoryAction::CreateTag {
                    // The dialog flags names already taken.
                    cx.worker.send(Command::LoadTags);
                }
            }
            if let Some(id) = clicked {
                cx.state.select_commit(&id);
                cx.worker.send(Command::LoadCommit(id));
            }
        },
    );
}

fn signature_text(pal: &win95::Palette, s: Option<&SignatureStatus>) -> (String, Color32) {
    match s {
        None => (s::SIG_CHECKING.into(), pal.gray_text),
        Some(SignatureStatus::Good { signer }) => {
            (format!("{} ({signer})", s::SIG_GOOD), pal.success)
        }
        Some(SignatureStatus::Bad) => (s::SIG_BAD.into(), pal.error),
        Some(SignatureStatus::Unknown) => (s::SIG_UNKNOWN.into(), pal.warning),
        Some(SignatureStatus::Unsigned) => (s::SIG_UNSIGNED.into(), pal.gray_text),
    }
}

fn detail(ui: &mut egui::Ui, cx: &mut Ctx<'_>) {
    let mut open_file: Option<String> = None;
    {
        let h = &mut cx.state.history;
        if let Some(d) = &h.detail_diff
            && h.detail_colors == crate::highlight::Colors::NotRequested
        {
            cx.highlighter
                .request(crate::highlight::Target::History, d.clone());
            h.detail_colors = crate::highlight::Colors::Pending;
        }
    }
    bevel_frame(
        ui,
        Bevel::Field,
        win95::theme::palette(ui.ctx()).window,
        4,
        |ui| {
            ui.set_min_size(ui.available_size());
            let h = &cx.state.history;
            let Some(d) = &h.detail else {
                ui.label(s::SELECT_A_COMMIT);
                return;
            };
            let (sig, sig_color) =
                signature_text(&win95::theme::palette(ui.ctx()), h.signature.as_ref());
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new(&d.short_id)
                        .font(egui::FontId::monospace(win95::theme::FONT_SIZE)),
                );
                ui.label(format!(
                    "- {} <{}> - {}",
                    d.author,
                    d.email,
                    format_epoch(d.time)
                ));
                ui.label(RichText::new(sig).color(sig_color));
            });
            ui.separator();
            // Left: message and changed files (own scroll). Right: diff of the selected file
            // (own scroll, both directions, only visible rows are laid out).
            egui::Panel::left("commit_files")
                .frame(egui::Frame::NONE)
                .resizable(true)
                .default_size(260.0)
                .min_size(140.0)
                .max_size((ui.available_width() - 200.0).max(140.0))
                .show(ui, |ui| {
                    ScrollArea::vertical()
                        .id_salt("commit_files_scroll")
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.add(
                                egui::Label::new(
                                    RichText::new(&d.message)
                                        .color(win95::theme::palette(ui.ctx()).text),
                                )
                                .wrap(),
                            );
                            ui.add_space(6.0);
                            ui.label(s::FILES);
                            for f in &d.files {
                                let label = super::changes::describe(&f.path, &f.change);
                                let selected = h.detail_file.as_deref() == Some(f.path.as_str());
                                if ui.selectable_label(selected, label).clicked() {
                                    open_file = Some(f.path.clone());
                                }
                            }
                        });
                });
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE.inner_margin(egui::Margin {
                    left: 6,
                    ..Default::default()
                }))
                .show(ui, |ui| commit_file_diff(ui, h));
        },
    );
    if let Some(path) = open_file {
        let h = &mut cx.state.history;
        if let Some(id) = h.selected.clone() {
            h.detail_file = Some(path.clone());
            h.detail_diff = None;
            cx.worker.send(Command::LoadCommitFileDiff { id, path });
        }
    }
}

/// Read-only diff of the file selected in the commit detail.
fn commit_file_diff(ui: &mut egui::Ui, h: &crate::state::HistoryView) {
    let Some(diff) = &h.detail_diff else {
        if h.detail_file.is_some() {
            ui.label(s::LOADING_DIFF);
        }
        return;
    };
    diff_rows(
        ui,
        diff,
        &h.detail_colors,
        ("commit_file_diff", &h.selected, &diff.path),
    );
}

/// A read-only colored diff (History commits, stashes).
pub fn diff_rows(
    ui: &mut egui::Ui,
    diff: &gitcore::FileDiff,
    colors: &crate::highlight::Colors,
    salt: impl std::hash::Hash + std::fmt::Debug,
) {
    if diff.binary {
        ui.label(s::BINARY_FILE);
        return;
    }
    if diff.hunks.is_empty() {
        ui.label(s::NO_DIFF);
        return;
    }
    let mono = egui::FontId::monospace(win95::theme::FONT_SIZE);
    let rows = super::diff_view::rows(diff);
    ScrollArea::both()
        .id_salt(salt)
        .auto_shrink([false, false])
        .show_rows(ui, super::diff_view::ROW_HEIGHT, rows.len(), |ui, range| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for row in &rows[range] {
                match *row {
                    super::diff_view::Row::Hunk(hi) => {
                        let header = crate::highlight::colored_line(
                            &win95::theme::palette(ui.ctx()),
                            "",
                            &diff.hunks[hi].header,
                            None,
                            "",
                            mono.clone(),
                            Color32::TRANSPARENT,
                        );
                        let pal = win95::theme::palette(ui.ctx());
                        let mut header = header;
                        for section in &mut header.sections {
                            section.format.color = pal.link;
                        }
                        crate::highlight::diff_row(
                            ui,
                            header,
                            super::diff_view::ROW_HEIGHT,
                            pal.hunk,
                        );
                    }
                    super::diff_view::Row::Line(hi, li) => {
                        let l = &diff.hunks[hi].lines[li];
                        let pal = win95::theme::palette(ui.ctx());
                        let (sign, bg) = match l.kind {
                            LineKind::Added => ("+", pal.added),
                            LineKind::Removed => ("-", pal.removed),
                            LineKind::Context => (" ", pal.window),
                        };
                        let num = |n: Option<u32>| {
                            n.map(|v| format!("{v:>5}"))
                                .unwrap_or_else(|| "     ".into())
                        };
                        let prefix = format!("{} {} {sign} ", num(l.old_no), num(l.new_no));
                        let text = l.text.trim_end_matches(['\n', '\r']);
                        let job = crate::highlight::colored_line(
                            &win95::theme::palette(ui.ctx()),
                            &prefix,
                            text,
                            colors.line(hi, li),
                            "",
                            mono.clone(),
                            Color32::TRANSPARENT,
                        );
                        crate::highlight::diff_row(ui, job, super::diff_view::ROW_HEIGHT, bg);
                    }
                }
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_times() {
        assert_eq!(relative_time(1000, 990), "just now");
        assert_eq!(relative_time(10_000, 10_000 - 300), "5m ago");
        assert_eq!(relative_time(100_000, 100_000 - 7_200), "2h ago");
        assert_eq!(relative_time(1_000_000, 1_000_000 - 172_800), "2d ago");
        assert_eq!(relative_time(1_700_000_000, 0), "1970-01-01");
    }

    #[test]
    fn lanes_are_evenly_spaced_and_colors_cycle() {
        assert_eq!(lane_x(0.0, 0), 7.0);
        assert_eq!(lane_x(0.0, 2), 35.0);
        let p = win95::palette::STANDARD;
        assert_eq!(lane_color(&p, 0), lane_color(&p, 8));
    }
}
