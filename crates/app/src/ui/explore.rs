//! Explore tab: the files of a version, the open file (content, blame, history), and the
//! results of a search in files.

use egui::{Align2, Color32, Rect, RichText, ScrollArea, Sense, Stroke, pos2, vec2};
use gitcore::{EntryKind, FileContent, LogSearch};
use win95::{Bevel, Button95, Dialog, bevel_frame, checkbox, combo_box, text_field};

use super::Ctx;
use crate::highlight::Target;
use crate::protocol::{Command, ExploreRequest};
use crate::state::{FileView, SearchForm, Tab, age_ranks};
use crate::strings as s;

const ROW: f32 = 18.0;
const CODE_ROW: f32 = super::diff_view::ROW_HEIGHT;
const GUTTER: f32 = 220.0;

/// Ask the explore service for what is shown, and the colors of the code shown.
fn requests(cx: &mut Ctx<'_>) {
    let Some(repo) = cx.state.current.as_ref().map(|c| c.path.clone()) else {
        return;
    };
    for req in cx.state.explore.needs() {
        cx.worker.explore(&repo, req);
    }
    let e = &mut cx.state.explore;
    let pending = crate::highlight::Colors::Pending;
    match e.view {
        FileView::Content => {
            if let Some(code) = &e.code
                && e.colors == crate::highlight::Colors::NotRequested
            {
                cx.highlighter.request(Target::Explore, code.clone());
                e.colors = pending;
            }
        }
        FileView::Blame => {
            if let Some(code) = &e.blame_code
                && e.blame_colors == crate::highlight::Colors::NotRequested
            {
                cx.highlighter.request(Target::ExploreBlame, code.clone());
                e.blame_colors = pending;
            }
        }
        FileView::History => {
            if let Some(d) = &e.history_diff
                && e.history_colors == crate::highlight::Colors::NotRequested
            {
                cx.highlighter.request(Target::ExploreHistory, d.clone());
                e.history_colors = pending;
            }
        }
    }
}

pub fn show(ui: &mut egui::Ui, cx: &mut Ctx<'_>) {
    requests(cx);
    toolbar(ui, cx);
    ui.add_space(2.0);
    egui::Panel::left("explore_tree")
        .frame(egui::Frame::NONE)
        .resizable(true)
        .default_size(280.0)
        .show(ui, |ui| {
            if cx.state.explore.search.is_some() {
                results(ui, cx);
            } else {
                tree(ui, cx);
            }
        });
    egui::CentralPanel::default()
        .frame(egui::Frame::NONE.inner_margin(egui::Margin {
            left: 4,
            ..Default::default()
        }))
        .show(ui, |ui| file(ui, cx));
}

fn toolbar(ui: &mut egui::Ui, cx: &mut Ctx<'_>) {
    let mut pick: Option<String> = None;
    ui.horizontal(|ui| {
        let e = &mut cx.state.explore;
        ui.label(s::REF);
        let shown = if e.rev.len() == 40 {
            e.rev.chars().take(7).collect()
        } else {
            e.rev.clone()
        };
        combo_box(ui, "explore_ref", &shown, 200.0, |ui| {
            ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
                for r in &e.refs {
                    if ui.selectable_label(r.name == e.rev, &r.name).clicked() {
                        pick = Some(r.name.clone());
                    }
                }
            });
        });
        if let Some(c) = &e.commit {
            let short: String = c.chars().take(7).collect();
            ui.label(RichText::new(short).monospace());
        }
        ui.add_space(12.0);
        ui.label(s::FIND_FILE);
        text_field(ui, &mut e.filter, 160.0, false);
        if ui
            .add(Button95::new(s::SEARCH_IN_FILES).min_size(vec2(120.0, 22.0)))
            .clicked()
        {
            let last = e.search.as_ref().map(|sv| SearchForm {
                text: sv.text.clone(),
                match_case: sv.match_case,
                paths: sv.paths.clone(),
            });
            e.search_form = Some(last.unwrap_or_default());
        }
    });
    if let Some(rev) = pick {
        cx.state.explore.set_rev(&rev);
    }
}

/// Small Win95 pictograms: closed or open folder, page.
fn paint_icon(p: &egui::Painter, rect: Rect, kind: EntryKind, open: bool) {
    let r = Rect::from_center_size(rect.center(), vec2(14.0, 11.0));
    let edge = Stroke::new(1.0, Color32::BLACK);
    match kind {
        EntryKind::Dir => {
            let yellow = Color32::from_rgb(0xFF, 0xE0, 0x60);
            let tab = Rect::from_min_size(r.min, vec2(6.0, 3.0));
            p.rect_filled(tab, 0.0, yellow);
            p.rect_stroke(tab, 0.0, edge, egui::StrokeKind::Inside);
            let body = Rect::from_min_max(pos2(r.min.x, r.min.y + 2.0), r.max);
            p.rect_filled(body, 0.0, yellow);
            p.rect_stroke(body, 0.0, edge, egui::StrokeKind::Inside);
            if open {
                let flap = Rect::from_min_max(pos2(r.min.x + 2.0, r.min.y + 5.0), r.max);
                p.rect_filled(flap, 0.0, Color32::from_rgb(0xFF, 0xF0, 0xA0));
                p.rect_stroke(flap, 0.0, edge, egui::StrokeKind::Inside);
            }
        }
        _ => {
            let page = Rect::from_center_size(rect.center(), vec2(10.0, 13.0));
            p.rect_filled(page, 0.0, Color32::WHITE);
            p.rect_stroke(page, 0.0, edge, egui::StrokeKind::Inside);
            for i in 0..3 {
                let y = page.top() + 4.0 + i as f32 * 3.0;
                p.line_segment(
                    [pos2(page.left() + 2.0, y), pos2(page.right() - 2.0, y)],
                    Stroke::new(1.0, Color32::GRAY),
                );
            }
        }
    }
}

fn tree(ui: &mut egui::Ui, cx: &mut Ctx<'_>) {
    let pal = win95::theme::palette(ui.ctx());
    let mut toggle: Option<String> = None;
    let mut open: Option<String> = None;
    bevel_frame(ui, Bevel::Field, pal.window, 2, |ui| {
        ui.set_min_size(ui.available_size());
        if let (None, Some(err)) = (&cx.state.explore.commit, &cx.state.explore.load_error) {
            ui.label(RichText::new(err).color(pal.error));
            return;
        }
        if cx.state.explore.commit.as_deref() == Some("") {
            ui.label(s::NO_COMMITS_YET);
            return;
        }
        if cx.state.explore.commit.is_none() {
            ui.label(RichText::new(s::LOADING_FILES).color(pal.gray_text));
            return;
        }
        let rows = cx.state.explore.rows().to_vec();
        let e = &cx.state.explore;
        let font = win95::theme::font(win95::theme::FONT_SIZE);
        ScrollArea::both()
            .id_salt("explore_tree_scroll")
            .auto_shrink([false, false])
            .show_rows(ui, ROW, rows.len(), |ui, range| {
                ui.spacing_mut().item_spacing.y = 0.0;
                for row in &rows[range] {
                    let indent = 4.0 + row.depth as f32 * 16.0;
                    let width =
                        (indent + 22.0 + row.name.len() as f32 * 8.0).max(ui.available_width());
                    let (rect, resp) = ui.allocate_exact_size(vec2(width, ROW), Sense::click());
                    let name = row.name.clone();
                    resp.widget_info(|| {
                        egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &name)
                    });
                    let selected = e.file.as_deref() == Some(row.path.as_str());
                    let p = ui.painter();
                    let dim = matches!(row.kind, EntryKind::Submodule | EntryKind::Symlink);
                    let color = if selected {
                        p.rect_filled(rect, 0.0, pal.selection);
                        pal.selection_text
                    } else if dim {
                        pal.gray_text
                    } else {
                        pal.window_text
                    };
                    let icon = Rect::from_min_size(
                        pos2(rect.left() + indent, rect.top()),
                        vec2(18.0, ROW),
                    );
                    paint_icon(p, icon, row.kind, row.open);
                    p.text(
                        pos2(icon.right() + 4.0, rect.center().y),
                        Align2::LEFT_CENTER,
                        &row.name,
                        font.clone(),
                        color,
                    );
                    if resp.clicked() {
                        match row.kind {
                            EntryKind::Dir => toggle = Some(row.path.clone()),
                            EntryKind::File => open = Some(row.path.clone()),
                            _ => {}
                        }
                    }
                }
            });
    });
    if let Some(d) = toggle {
        cx.state.explore.toggle_dir(&d);
    }
    if let Some(f) = open {
        cx.state.explore.open_file(&f);
    }
}

fn results(ui: &mut egui::Ui, cx: &mut Ctx<'_>) {
    let pal = win95::theme::palette(ui.ctx());
    let mut open: Option<(String, usize)> = None;
    let mut back = false;
    let mut cancel = false;
    ui.horizontal(|ui| {
        back = ui.add(Button95::new(s::BACK_TO_FILES)).clicked();
        if cx
            .state
            .explore
            .search
            .as_ref()
            .is_some_and(|sv| sv.running)
        {
            ui.label(s::SEARCHING);
            cancel = ui.add(Button95::new(s::CANCEL_SEARCH)).clicked();
        }
    });
    bevel_frame(ui, Bevel::Field, pal.window, 2, |ui| {
        ui.set_min_size(ui.available_size());
        let Some(sv) = &cx.state.explore.search else {
            return;
        };
        let Some(result) = &sv.result else { return };
        if result.matches.is_empty() {
            ui.label(s::NO_MATCHES);
        }
        if result.truncated {
            ui.label(RichText::new(s::MATCHES_TRUNCATED).color(pal.warning));
        }
        let mono = egui::FontId::monospace(win95::theme::FONT_SIZE);
        ScrollArea::both()
            .id_salt("explore_results")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                let mut last: Option<&str> = None;
                for m in &result.matches {
                    if last != Some(m.path.as_str()) {
                        ui.label(RichText::new(&m.path).color(pal.link));
                        last = Some(m.path.as_str());
                    }
                    let job = match_job(&pal, &mono, m.line, &m.text, &sv.text, sv.match_case);
                    let r = ui.add(egui::Label::new(job).sense(Sense::click()).truncate());
                    if r.clicked() {
                        open = Some((m.path.clone(), m.line));
                    }
                }
            });
    });
    let repo = cx.state.current.as_ref().map(|c| c.path.clone());
    if cancel {
        if let Some(sv) = cx.state.explore.search.as_mut() {
            sv.running = false;
        }
        if let Some(repo) = &repo {
            cx.worker.explore(repo, ExploreRequest::CancelSearch);
        }
    }
    if back {
        if cx
            .state
            .explore
            .search
            .as_ref()
            .is_some_and(|sv| sv.running)
            && let Some(repo) = &repo
        {
            cx.worker.explore(repo, ExploreRequest::CancelSearch);
        }
        cx.state.explore.search = None;
    }
    if let Some((path, line)) = open {
        cx.state.explore.open_match(&path, line);
    }
}

/// "  12  text" with the searched text highlighted.
fn match_job(
    pal: &win95::Palette,
    font: &egui::FontId,
    line: usize,
    text: &str,
    needle: &str,
    match_case: bool,
) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    let fmt = |color: Color32, bg: Color32| egui::TextFormat {
        font_id: font.clone(),
        color,
        background: bg,
        ..Default::default()
    };
    job.append(
        &format!("{line:>5}  "),
        0.0,
        fmt(pal.gray_text, Color32::TRANSPARENT),
    );
    let text = crate::highlight::expand_tabs(text);
    let hay = if match_case {
        text.clone()
    } else {
        text.to_lowercase()
    };
    let pin = if match_case {
        needle.to_string()
    } else {
        needle.to_lowercase()
    };
    // Lowercasing can change byte lengths: only highlight when it did not.
    let at = (hay.len() == text.len() && !pin.is_empty())
        .then(|| hay.find(&pin))
        .flatten();
    match at {
        Some(i) if text.is_char_boundary(i) && text.is_char_boundary(i + pin.len()) => {
            job.append(&text[..i], 0.0, fmt(pal.window_text, Color32::TRANSPARENT));
            job.append(
                &text[i..i + pin.len()],
                0.0,
                fmt(pal.selection_text, pal.selection),
            );
            job.append(
                &text[i + pin.len()..],
                0.0,
                fmt(pal.window_text, Color32::TRANSPARENT),
            );
        }
        _ => job.append(&text, 0.0, fmt(pal.window_text, Color32::TRANSPARENT)),
    }
    job
}

fn file(ui: &mut egui::Ui, cx: &mut Ctx<'_>) {
    let pal = win95::theme::palette(ui.ctx());
    let Some(path) = cx.state.explore.file.clone() else {
        bevel_frame(ui, Bevel::Field, pal.window, 4, |ui| {
            ui.set_min_size(ui.available_size());
            ui.label(s::EXPLORE_SELECT_FILE);
        });
        return;
    };
    ui.horizontal(|ui| {
        ui.label(RichText::new(&path).color(pal.link));
    });
    const VIEWS: [FileView; 3] = [FileView::Content, FileView::Blame, FileView::History];
    let mut i = VIEWS
        .iter()
        .position(|v| *v == cx.state.explore.view)
        .unwrap_or(0);
    win95::tabs(
        ui,
        &mut i,
        &[s::FILE_CONTENT, s::FILE_BLAME, s::FILE_HISTORY],
    );
    cx.state.explore.view = VIEWS[i];
    bevel_frame(ui, Bevel::Field, pal.window, 4, |ui| {
        ui.set_min_size(ui.available_size());
        match cx.state.explore.view {
            FileView::Content => content(ui, cx),
            FileView::Blame => blame(ui, cx),
            FileView::History => history(ui, cx),
        }
    });
}

fn content(ui: &mut egui::Ui, cx: &mut Ctx<'_>) {
    let pal = win95::theme::palette(ui.ctx());
    let e = &cx.state.explore;
    match &e.content {
        None => loading(ui, &pal, e.load_error.as_deref()),
        Some(FileContent::Binary) => {
            ui.label(s::BINARY_FILE);
        }
        Some(FileContent::TooLarge(n)) => {
            let mb = format!("{:.1}", *n as f64 / (1024.0 * 1024.0));
            ui.label(s::FILE_TOO_LARGE.replace("{size}", &mb));
        }
        Some(FileContent::Text(_)) => {
            let Some(code) = &e.code else { return };
            let lines = code
                .hunks
                .first()
                .map(|h| h.lines.as_slice())
                .unwrap_or(&[]);
            let mono = egui::FontId::monospace(win95::theme::FONT_SIZE);
            let mut area = ScrollArea::both()
                .id_salt(("explore_content", &e.rev, &code.path))
                .auto_shrink([false, false]);
            let key = egui::Id::new("explore_scrolled_to");
            if let Some(line) = e.goto_line
                && ui.ctx().data(|d| d.get_temp::<(String, usize)>(key))
                    != Some((code.path.clone(), line))
            {
                area = area.vertical_scroll_offset(((line as f32 - 5.0) * CODE_ROW).max(0.0));
                ui.ctx()
                    .data_mut(|d| d.insert_temp(key, (code.path.clone(), line)));
            }
            area.show_rows(ui, CODE_ROW, lines.len(), |ui, range| {
                ui.spacing_mut().item_spacing.y = 0.0;
                for i in range {
                    let text = lines[i].text.trim_end_matches(['\n', '\r']);
                    let job = crate::highlight::colored_line(
                        &pal,
                        &format!("{:>6}  ", i + 1),
                        text,
                        e.colors.line(0, i),
                        "",
                        mono.clone(),
                        Color32::TRANSPARENT,
                    );
                    let bg = if e.goto_line == Some(i + 1) {
                        pal.line_selected
                    } else {
                        pal.window
                    };
                    crate::highlight::diff_row(ui, job, CODE_ROW, bg);
                }
            });
        }
    }
}

/// "Loading..." or, if it failed, why.
fn loading(ui: &mut egui::Ui, pal: &win95::Palette, error: Option<&str>) {
    match error {
        Some(err) => ui.label(RichText::new(err).color(pal.error)),
        None => ui.label(RichText::new(s::LOADING_FILE).color(pal.gray_text)),
    };
}

/// `a` mixed with `b`: `t` = 1 gives `a`.
fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let m = |x: u8, y: u8| (f32::from(x) * t + f32::from(y) * (1.0 - t)).round() as u8;
    Color32::from_rgb(m(a.r(), b.r()), m(a.g(), b.g()), m(a.b(), b.b()))
}

fn blame(ui: &mut egui::Ui, cx: &mut Ctx<'_>) {
    let pal = win95::theme::palette(ui.ctx());
    let mut back = false;
    let mut parent: Option<gitcore::BlameBlock> = None;
    let mut show_commit: Option<String> = None;
    {
        let e = &cx.state.explore;
        if let Some((rev, path)) = e.blame_target() {
            ui.horizontal(|ui| {
                if e.can_go_back() {
                    back = ui.add(Button95::new(s::BLAME_BACK)).clicked();
                }
                let short: String = rev.chars().take(12).collect();
                ui.label(
                    RichText::new(
                        s::BLAME_OF
                            .replace("{path}", &path)
                            .replace("{rev}", &short),
                    )
                    .color(pal.gray_text),
                );
            });
        }
        if matches!(
            e.content,
            Some(FileContent::Binary | FileContent::TooLarge(_))
        ) {
            ui.label(s::NO_BLAME_BINARY);
            return;
        }
        let Some(blocks) = &e.blame else {
            loading(ui, &pal, e.load_error.as_deref());
            return;
        };
        let ages = age_ranks(blocks);
        // (block, line within the block) for each displayed line.
        let rows: Vec<(usize, usize)> = blocks
            .iter()
            .enumerate()
            .flat_map(|(b, block)| (0..block.lines.len()).map(move |l| (b, l)))
            .collect();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let mono = egui::FontId::monospace(win95::theme::FONT_SIZE);
        let font = win95::theme::font(win95::theme::FONT_SIZE);
        ScrollArea::both()
            .id_salt(("explore_blame", e.blame_target()))
            .auto_shrink([false, false])
            .show_rows(ui, CODE_ROW, rows.len(), |ui, range| {
                ui.spacing_mut().item_spacing.y = 0.0;
                for i in range {
                    let (b, l) = rows[i];
                    let block = &blocks[b];
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        let (rect, resp) =
                            ui.allocate_exact_size(vec2(GUTTER, CODE_ROW), Sense::click());
                        let bg = mix(pal.ref_tag, pal.window, ages[b]);
                        ui.painter().rect_filled(rect, 0.0, bg);
                        if l == 0 {
                            let short: String = block.commit.chars().take(7).collect();
                            let when = super::history::relative_time(now, block.time);
                            let text = format!("{short} {} {when}", block.author);
                            resp.widget_info(|| {
                                egui::WidgetInfo::labeled(egui::WidgetType::Label, true, &text)
                            });
                            ui.painter().with_clip_rect(rect.shrink(2.0)).text(
                                rect.left_center() + vec2(4.0, 0.0),
                                Align2::LEFT_CENTER,
                                &text,
                                font.clone(),
                                pal.window_text,
                            );
                        }
                        let resp = resp.on_hover_text(&block.summary);
                        if resp.clicked() {
                            show_commit = Some(block.commit.clone());
                        }
                        resp.context_menu(|ui| {
                            if ui
                                .add_enabled(
                                    block.previous.is_some(),
                                    egui::Button::new(s::BLAME_PARENT),
                                )
                                .clicked()
                            {
                                parent = Some(block.clone());
                                ui.close();
                            }
                            if ui.button(s::OPEN_IN_HISTORY).clicked() {
                                show_commit = Some(block.commit.clone());
                                ui.close();
                            }
                        });
                        let line_no = block.start + l;
                        let job = crate::highlight::colored_line(
                            &pal,
                            &format!(" {line_no:>6}  "),
                            &block.lines[l],
                            e.blame_colors.line(0, i),
                            "",
                            mono.clone(),
                            Color32::TRANSPARENT,
                        );
                        crate::highlight::diff_row(ui, job, CODE_ROW, pal.window);
                    });
                }
            });
    }
    if back {
        cx.state.explore.blame_back();
    }
    if let Some(b) = parent {
        cx.state.explore.blame_parent(&b);
    }
    if let Some(id) = show_commit {
        open_in_history(cx, &id);
    }
}

fn open_in_history(cx: &mut Ctx<'_>, id: &str) {
    cx.state.tab = Tab::History;
    cx.state.select_commit(id);
    cx.worker.send(Command::LoadCommit(id.to_string()));
}

fn history(ui: &mut egui::Ui, cx: &mut Ctx<'_>) {
    let pal = win95::theme::palette(ui.ctx());
    let mut select: Option<String> = None;
    let mut open: Option<String> = None;
    {
        let e = &cx.state.explore;
        let Some(commits) = &e.history else {
            loading(ui, &pal, e.load_error.as_deref());
            return;
        };
        if commits.is_empty() {
            ui.label(s::NO_FILE_HISTORY);
            return;
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        ScrollArea::vertical()
            .id_salt("explore_history")
            .max_height((ui.available_height() * 0.4).max(80.0))
            .auto_shrink([false, true])
            .show(ui, |ui| {
                for c in commits {
                    let renamed = c
                        .renamed_from
                        .as_ref()
                        .map(|p| format!("  {}", s::RENAMED_FROM.replace("{path}", p)))
                        .unwrap_or_default();
                    let text = format!(
                        "{}  {}  {}  {}{renamed}",
                        c.entry.short_id,
                        c.entry.summary,
                        c.entry.author,
                        super::history::relative_time(now, c.entry.time)
                    );
                    let on = e.history_selected.as_deref() == Some(c.entry.id.as_str());
                    let r = ui.selectable_label(on, text);
                    if r.double_clicked() {
                        open = Some(c.entry.id.clone());
                    } else if r.clicked() {
                        select = Some(c.entry.id.clone());
                    }
                }
            });
        ui.separator();
        match &e.history_diff {
            Some(d) => {
                super::history::diff_rows(ui, d, &e.history_colors, ("explore_diff", &d.path))
            }
            None if e.history_selected.is_some() => match &e.load_error {
                Some(err) => {
                    ui.label(RichText::new(err).color(pal.error));
                }
                None => {
                    ui.label(RichText::new(s::LOADING_DIFF).color(pal.gray_text));
                }
            },
            None => {}
        }
    }
    if let Some(id) = open {
        open_in_history(cx, &id);
    } else if let Some(id) = select {
        cx.state.explore.select_history(&id);
    }
}

/// The Search in files window.
pub fn search_dialog(egui_ctx: &egui::Context, cx: &mut Ctx<'_>) {
    let Some(mut form) = cx.state.explore.search_form.clone() else {
        return;
    };
    let (mut go, mut cancel) = (false, false);
    let r = Dialog::new("explore_search", s::SEARCH_TITLE)
        .width(420.0)
        .show(egui_ctx, |ui| {
            ui.label(s::SEARCH_TEXT);
            let field = text_field(ui, &mut form.text, 400.0, false);
            if field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                go = true;
            }
            checkbox(ui, &mut form.match_case, s::MATCH_CASE);
            ui.label(s::SEARCH_PATHS);
            text_field(ui, &mut form.paths, 400.0, false);
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let ready = !form.text.is_empty();
                go |= ui
                    .add(
                        Button95::new(s::SEARCH)
                            .min_size(vec2(90.0, 23.0))
                            .enabled(ready),
                    )
                    .clicked();
                cancel = ui
                    .add(Button95::new(s::CANCEL).min_size(vec2(90.0, 23.0)))
                    .clicked();
            });
        });
    if cancel || r.close_requested {
        cx.state.explore.search_form = None;
        return;
    }
    if go && !form.text.is_empty() {
        cx.state.explore.search_form = None;
        let req = cx
            .state
            .explore
            .start_search(&form.text, form.match_case, form.paths.trim());
        if let Some(repo) = cx.state.current.as_ref().map(|c| c.path.clone()) {
            cx.worker.explore(&repo, req);
        }
        return;
    }
    cx.state.explore.search_form = Some(form);
}

/// Label of a History filter kind.
pub fn search_kind_label(k: LogSearch) -> &'static str {
    match k {
        LogSearch::Message => s::SEARCH_MESSAGE,
        LogSearch::Author => s::SEARCH_AUTHOR,
        LogSearch::Hash => s::SEARCH_HASH,
        LogSearch::ChangedText => s::SEARCH_CHANGED,
    }
}

/// The filter line above the History list.
pub fn history_filter(ui: &mut egui::Ui, cx: &mut Ctx<'_>) {
    let mut go = false;
    let mut clear = false;
    let mut cancel = false;
    ui.horizontal(|ui| {
        let f = &mut cx.state.history.filter;
        ui.label(s::SEARCH_IN);
        combo_box(
            ui,
            "history_search_kind",
            search_kind_label(f.kind),
            110.0,
            |ui| {
                for k in [
                    LogSearch::Message,
                    LogSearch::Author,
                    LogSearch::Hash,
                    LogSearch::ChangedText,
                ] {
                    if ui
                        .selectable_label(f.kind == k, search_kind_label(k))
                        .clicked()
                    {
                        f.kind = k;
                    }
                }
            },
        );
        let field = text_field(ui, &mut f.query, 220.0, false);
        if field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            go = true;
        }
        go |= ui
            .add(Button95::new(s::SEARCH).enabled(!f.query.trim().is_empty() && !f.running))
            .clicked();
        if f.running {
            ui.label(s::SEARCHING);
            cancel = ui.add(Button95::new(s::CANCEL_SEARCH)).clicked();
        }
        if let Some((found, more)) = &f.results {
            let n = if found.len() == 1 {
                s::COMMIT_FOUND.to_string()
            } else {
                s::COMMITS_FOUND.replace("{n}", &found.len().to_string())
            };
            ui.label(format!("{n}{}", if *more { s::FIRST_SHOWN } else { "" }));
        }
        if f.results.is_some() || f.running {
            clear = ui.add(Button95::new(s::CLEAR)).clicked();
        }
    });
    let repo = cx.state.current.as_ref().map(|c| c.path.clone());
    if (cancel || clear)
        && cx.state.history.filter.running
        && let Some(repo) = &repo
    {
        cx.worker.explore(repo, ExploreRequest::CancelLogSearch);
    }
    if clear || cancel {
        cx.state.history.clear_filter();
    }
    if go
        && let Some(req) = cx.state.history.start_filter()
        && let Some(repo) = &repo
    {
        cx.worker.explore(repo, req);
    }
}
