//! Detail of the selected pull request: header, labels, actions and sub-tabs.

use egui::{Color32, RichText, ScrollArea};
use gitcore::LineKind;
use github::{CheckStatus, DiffSide, PrDetail, PrState, ReviewState, TimelineItem};
use win95::{Bevel, Button95, bevel_frame, label_chip, markdown_view, text_area};

use super::Ctx;
use super::diff_view::ROW_HEIGHT;
use super::pulls::{AMBER, GREEN, RED};
use crate::protocol::{Command, Slug};
use crate::state::{
    PullDialog, PullTab, default_merge_method, merge_defaults, merge_disabled_reason,
    review_events_allowed,
};
use crate::strings as s;

const COMMENT_BG: Color32 = Color32::from_rgb(0xFF, 0xFF, 0xE8);
const PENDING_BG: Color32 = Color32::from_rgb(0xFF, 0xF0, 0xC0);

pub fn state_text(d: &PrDetail) -> (&'static str, Color32) {
    match d.summary.state {
        PrState::Merged => (s::STATE_MERGED, Color32::from_rgb(0x60, 0x20, 0x90)),
        PrState::Closed => (s::STATE_CLOSED, RED),
        PrState::Open if d.summary.draft => (s::DRAFT, win95::theme::GRAY),
        PrState::Open => (s::STATE_OPEN, GREEN),
    }
}

pub fn review_verb(state: ReviewState) -> &'static str {
    match state {
        ReviewState::Approved => s::REVIEW_VERB_APPROVED,
        ReviewState::ChangesRequested => s::REVIEW_VERB_CHANGES,
        ReviewState::Commented => s::REVIEW_VERB_COMMENTED,
        ReviewState::Dismissed => s::REVIEW_VERB_DISMISSED,
    }
}

/// `Some(true)`: the viewer may resolve the thread, `Some(false)`: unresolve it.
pub fn resolve_action(t: &github::ReviewThread) -> Option<bool> {
    match (t.resolved, t.can_resolve, t.can_unresolve) {
        (false, true, _) => Some(true),
        (true, _, true) => Some(false),
        _ => None,
    }
}

fn resolve_label(resolve: bool) -> &'static str {
    if resolve { s::RESOLVE } else { s::UNRESOLVE }
}

/// Ask the worker to (un)resolve thread `id` of the shown pull request.
fn send_resolve(cx: &mut Ctx<'_>, number: u64, id: String, resolve: bool) {
    let Some(slug) = cx.state.github_slug() else {
        return;
    };
    cx.state.pulls.busy = true;
    cx.worker.send(Command::ResolveThread {
        slug,
        number,
        thread_id: id,
        resolve,
    });
}

/// "2m 05s" from two ISO timestamps.
pub fn duration_text(start: Option<&str>, end: Option<&str>) -> String {
    let (Some(a), Some(b)) = (
        start.and_then(crate::format::parse_iso8601),
        end.and_then(crate::format::parse_iso8601),
    ) else {
        return String::new();
    };
    let d = (b - a).max(0);
    format!("{}m {:02}s", d / 60, d % 60)
}

pub fn show(ui: &mut egui::Ui, cx: &mut Ctx<'_>, slug: &Slug) {
    bevel_frame(ui, Bevel::Field, win95::theme::WHITE, 4, |ui| {
        ui.set_min_size(ui.available_size());
        let p = &cx.state.pulls;
        if p.selected.is_none() {
            ui.label(s::SELECT_A_PULL);
            return;
        }
        let Some(d) = p.detail.clone() else {
            ui.label(s::LOADING_PULL);
            return;
        };
        header(ui, cx, &d);
        ui.separator();
        let files = cx.state.pulls.files.as_ref().map(|f| f.len()).unwrap_or(0);
        let labels = [
            s::PULL_CONVERSATION.to_string(),
            format!("{} ({})", s::PULL_COMMITS, d.commit_count),
            format!("{} ({files})", s::PULL_FILES),
            s::PULL_CHECKS.to_string(),
        ];
        let tabs = [
            PullTab::Conversation,
            PullTab::Commits,
            PullTab::Files,
            PullTab::Checks,
        ];
        let mut i = tabs
            .iter()
            .position(|t| *t == cx.state.pulls.sub_tab)
            .unwrap_or(0);
        let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
        win95::tabs(ui, &mut i, &refs);
        cx.state.pulls.sub_tab = tabs[i];
        match cx.state.pulls.sub_tab {
            PullTab::Conversation => conversation(ui, cx, slug, &d),
            PullTab::Commits => commits(ui, cx, &d),
            PullTab::Files => files_tab(ui, cx, &d),
            PullTab::Checks => checks(ui, &d),
        }
    });
}

fn header(ui: &mut egui::Ui, cx: &mut Ctx<'_>, d: &PrDetail) {
    let navy = win95::theme::NAVY;
    ui.label(
        RichText::new(format!("#{} {}", d.summary.number, d.summary.title))
            .size(win95::theme::FONT_SIZE + 3.0)
            .color(navy),
    );
    let (state, color) = state_text(d);
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(state).color(color));
        let what = s::WANTS_TO_MERGE
            .replace("{head}", &d.summary.head)
            .replace("{base}", &d.summary.base);
        ui.label(format!("{} {what}", d.summary.author));
        ui.label(
            RichText::new(format!("({} @{})", s::AS_ACCOUNT, d.viewer)).color(win95::theme::GRAY),
        );
    });
    ui.horizontal_wrapped(|ui| {
        ui.label(s::LABELS);
        for l in &d.summary.labels {
            label_chip(ui, &l.name, l.color);
        }
        if d.viewer_can_write
            && ui
                .add(Button95::new(s::EDIT_LABELS).enabled(!cx.state.pulls.busy))
                .clicked()
        {
            cx.state.pulls.dialog = Some(PullDialog::Labels {
                checked: d.summary.labels.iter().map(|l| l.name.clone()).collect(),
            });
        }
    });
    ui.horizontal(|ui| {
        let busy = cx.state.pulls.busy;
        let size = egui::vec2(80.0, 22.0);
        let open = d.summary.state == PrState::Open;
        if ui
            .add(
                Button95::new(s::CHECKOUT)
                    .min_size(size)
                    .enabled(open && !busy),
            )
            .clicked()
        {
            let head = (!d.cross_repository).then(|| d.summary.head.clone());
            cx.worker.send(Command::CheckoutPull {
                number: d.summary.number,
                head,
            });
        }
        let events = review_events_allowed(d);
        if ui
            .add(
                Button95::new(s::REVIEW)
                    .min_size(size)
                    .enabled(!events.is_empty() && !busy),
            )
            .clicked()
        {
            cx.state.pulls.dialog = Some(PullDialog::Review {
                event: events[0],
                body: String::new(),
            });
        }
        let why = merge_disabled_reason(d);
        let merge = ui.add(
            Button95::new(s::MERGE_PULL)
                .min_size(size)
                .enabled(why.is_none() && !busy),
        );
        if let Some(why) = why {
            // With several accounts, another one may be allowed: say how to switch.
            let why = if why == s::WHY_NO_PERMISSION && cx.state.accounts.len() > 1 {
                format!("{why} {}", s::SWITCH_ACCOUNT_HINT)
            } else {
                why.to_string()
            };
            merge.on_disabled_hover_text(&why);
            ui.label(RichText::new(why).color(win95::theme::GRAY));
        } else if merge.clicked()
            && let Some(method) = default_merge_method(d)
        {
            let (title, message) = merge_defaults(d, method);
            cx.state.pulls.dialog = Some(PullDialog::Merge {
                method,
                title,
                message,
                delete_branch: !d.cross_repository,
            });
        }
        if ui
            .add(Button95::new(s::OPEN_ON_GITHUB).min_size(egui::vec2(110.0, 22.0)))
            .clicked()
        {
            ui.ctx().open_url(egui::OpenUrl::new_tab(&d.summary.url));
        }
    });
    if !cx.state.pulls.pending.is_empty() {
        // Shown until the review is submitted.
        ui.label(
            RichText::new(format!(
                "{} {}",
                cx.state.pulls.pending.len(),
                s::PENDING_COMMENTS
            ))
            .color(AMBER),
        );
    }
}

fn boxed(ui: &mut egui::Ui, title: &str, body: &str) {
    egui::Frame::NONE
        .fill(COMMENT_BG)
        .stroke(egui::Stroke::new(1.0, win95::theme::LIGHT))
        .inner_margin(egui::Margin::same(6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(title).color(win95::theme::NAVY));
            if !body.trim().is_empty() {
                markdown_view(ui, body);
            }
        });
    ui.add_space(4.0);
}

fn conversation(ui: &mut egui::Ui, cx: &mut Ctx<'_>, slug: &Slug, d: &PrDetail) {
    let mut send = false;
    let mut toggle: Option<(String, bool)> = None;
    let busy = cx.state.pulls.busy;
    ScrollArea::vertical()
        .id_salt(("pull_conversation", d.summary.number))
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if d.body.trim().is_empty() {
                ui.label(RichText::new(s::NO_DESCRIPTION).color(win95::theme::GRAY));
            } else {
                markdown_view(ui, &d.body);
            }
            ui.separator();
            for item in &d.timeline {
                let at = item.at().get(..16).unwrap_or("").replace('T', " ");
                match item {
                    TimelineItem::Comment { author, body, .. } => {
                        boxed(ui, &format!("{author} {}  {at}", s::COMMENTED), body)
                    }
                    TimelineItem::Review {
                        author,
                        state,
                        body,
                        ..
                    } => boxed(ui, &format!("{author} {}  {at}", review_verb(*state)), body),
                }
            }
            // Every line thread, also shown under its line in Files when it can be placed.
            for t in &d.threads {
                let line = t
                    .line
                    .or(t.original_line)
                    .map(|l| format!(":{l}"))
                    .unwrap_or_default();
                let tag = if t.outdated {
                    format!(" ({})", s::OUTDATED)
                } else if t.resolved {
                    format!(" ({})", s::RESOLVED)
                } else {
                    String::new()
                };
                let title = format!("{}{line}{tag}", t.path);
                let body = t
                    .comments
                    .iter()
                    .map(|c| format!("{}: {}", c.author, c.body))
                    .collect::<Vec<_>>()
                    .join("\n\n");
                boxed(ui, &title, &body);
                if let Some(resolve) = resolve_action(t)
                    && ui
                        .add(Button95::new(resolve_label(resolve)).enabled(!busy))
                        .clicked()
                {
                    toggle = Some((t.id.clone(), resolve));
                }
            }
            ui.add_space(6.0);
            let width = ui.available_width() - 12.0;
            text_area(ui, &mut cx.state.pulls.comment, width, 3);
            let ready = !cx.state.pulls.comment.trim().is_empty() && !cx.state.pulls.busy;
            if ui.add(Button95::new(s::COMMENT).enabled(ready)).clicked() {
                send = true;
            }
        });
    if let Some((id, resolve)) = toggle {
        send_resolve(cx, d.summary.number, id, resolve);
    }
    if send && let Some(body) = cx.state.pulls.send_comment() {
        cx.worker.send(Command::AddPullComment {
            slug: slug.clone(),
            number: d.summary.number,
            body,
        });
    }
}

fn commits(ui: &mut egui::Ui, cx: &mut Ctx<'_>, d: &PrDetail) {
    let mut open: Option<String> = None;
    ScrollArea::vertical()
        .id_salt(("pull_commits", d.summary.number))
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for c in &d.commits {
                let date = c.date.get(..10).unwrap_or("");
                let text = format!("{}  {}  {}  {date}", c.short_oid, c.headline, c.author);
                if ui
                    .selectable_label(false, RichText::new(text).color(win95::theme::BLACK))
                    .clicked()
                {
                    open = Some(c.oid.clone());
                }
            }
        });
    if let Some(id) = open {
        // Same view as the History tab (the commit must have been fetched).
        cx.state.tab = crate::state::Tab::History;
        cx.state.select_commit(&id);
        cx.worker.send(Command::LoadCommit(id));
    }
}

fn checks(ui: &mut egui::Ui, d: &PrDetail) {
    if d.check_runs.is_empty() {
        ui.label(s::NO_CHECKS);
        return;
    }
    ScrollArea::vertical()
        .id_salt(("pull_checks", d.summary.number))
        .auto_shrink([false, false])
        .show(ui, |ui| {
            egui::Grid::new("checks_grid")
                .num_columns(4)
                .show(ui, |ui| {
                    for c in &d.check_runs {
                        let (text, color) = match c.status {
                            CheckStatus::Success => (s::CHECK_PASSED, GREEN),
                            CheckStatus::Failure => (s::CHECK_FAILED, RED),
                            CheckStatus::Pending => (s::CHECK_RUNNING, AMBER),
                            CheckStatus::Neutral => (s::CHECK_SKIPPED, win95::theme::GRAY),
                        };
                        ui.label(RichText::new(text).color(color));
                        ui.label(&c.name);
                        ui.label(duration_text(
                            c.started_at.as_deref(),
                            c.completed_at.as_deref(),
                        ));
                        match &c.url {
                            Some(url) => ui.hyperlink_to(s::CHECK_DETAILS, url),
                            None => ui.label(""),
                        };
                        ui.end_row();
                    }
                });
        });
}

/// Rows of the Files tab diff: hunks, lines, and under a line its comments.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileRow {
    Hunk(usize),
    Line(usize, usize),
    /// Thread index, comment index.
    Comment(usize, usize),
    /// Index in the pending review.
    Pending(usize),
}

/// Pure: lay out `diff` with the comments of `threads` and `pending` under their lines.
pub fn file_rows(
    diff: &gitcore::FileDiff,
    threads: &[github::ReviewThread],
    pending: &[github::LineComment],
) -> Vec<FileRow> {
    let mut out = Vec::new();
    for (h, hunk) in diff.hunks.iter().enumerate() {
        out.push(FileRow::Hunk(h));
        for (l, line) in hunk.lines.iter().enumerate() {
            out.push(FileRow::Line(h, l));
            for t in crate::pr_diff::threads_at(threads, &diff.path, line) {
                out.extend((0..threads[t].comments.len()).map(|c| FileRow::Comment(t, c)));
            }
            if let Some(target) = crate::pr_diff::line_target(line) {
                out.extend(
                    pending
                        .iter()
                        .enumerate()
                        .filter(|(_, c)| c.path == diff.path && (c.line, c.side) == target)
                        .map(|(i, _)| FileRow::Pending(i)),
                );
            }
        }
    }
    out
}

fn files_tab(ui: &mut egui::Ui, cx: &mut Ctx<'_>, d: &PrDetail) {
    let mut open: Option<String> = None;
    {
        let p = &mut cx.state.pulls;
        if let Some(diff) = &p.file_diff
            && p.file_colors == crate::highlight::Colors::NotRequested
        {
            cx.highlighter
                .request(crate::highlight::Target::Pull, diff.clone());
            p.file_colors = crate::highlight::Colors::Pending;
        }
    }
    let Some(files) = cx.state.pulls.files.clone() else {
        ui.label(s::LOADING_PULL);
        return;
    };
    egui::Panel::left("pull_files")
        .frame(egui::Frame::NONE)
        .resizable(true)
        .default_size(220.0)
        .min_size(120.0)
        .max_size((ui.available_width() - 200.0).max(120.0))
        .show(ui, |ui| {
            ScrollArea::vertical()
                .id_salt(("pull_files_list", d.summary.number))
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for f in files.iter() {
                        let selected = cx.state.pulls.file.as_deref() == Some(f.path.as_str());
                        let letter = match f.status.as_str() {
                            "added" => "A",
                            "removed" => "D",
                            "renamed" => "R",
                            _ => "M",
                        };
                        let text =
                            format!("{letter} {}  +{} -{}", f.path, f.additions, f.deletions);
                        if ui.selectable_label(selected, text).clicked() {
                            open = Some(f.path.clone());
                        }
                    }
                });
        });
    egui::CentralPanel::default()
        .frame(egui::Frame::NONE.inner_margin(egui::Margin {
            left: 6,
            ..Default::default()
        }))
        .show(ui, |ui| file_diff(ui, cx, d));
    if let Some(path) = open {
        cx.state.pulls.open_file(&path);
    }
}

fn file_diff(ui: &mut egui::Ui, cx: &mut Ctx<'_>, d: &PrDetail) {
    let p = &cx.state.pulls;
    let Some(diff) = &p.file_diff else {
        ui.label(s::SELECT_A_FILE);
        return;
    };
    if diff.binary {
        ui.label(s::NO_PATCH);
        return;
    }
    if diff.hunks.is_empty() {
        ui.label(s::NO_DIFF);
        return;
    }
    let can_comment = d.summary.state == PrState::Open;
    let mono = egui::FontId::monospace(win95::theme::FONT_SIZE);
    let rows = file_rows(diff, &d.threads, &p.pending);
    let mut comment_on: Option<(String, u32, DiffSide, String)> = None;
    let mut reply_to: Option<u64> = None;
    let mut toggle: Option<(String, bool)> = None;
    let busy = p.busy;
    let mut drop_pending: Option<usize> = None;
    ScrollArea::both()
        .id_salt(("pull_file_diff", d.summary.number, &diff.path))
        .auto_shrink([false, false])
        .show_rows(ui, ROW_HEIGHT, rows.len(), |ui, range| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for row in &rows[range] {
                match *row {
                    FileRow::Hunk(hi) => {
                        let mut job = crate::highlight::colored_line(
                            "",
                            &diff.hunks[hi].header,
                            None,
                            "",
                            mono.clone(),
                            Color32::TRANSPARENT,
                        );
                        for section in &mut job.sections {
                            section.format.color = win95::theme::NAVY;
                        }
                        crate::highlight::diff_row(
                            ui,
                            job,
                            ROW_HEIGHT,
                            Color32::from_rgb(0xE0, 0xE0, 0xF0),
                        );
                    }
                    FileRow::Line(hi, li) => {
                        let l = &diff.hunks[hi].lines[li];
                        let (sign, bg) = match l.kind {
                            LineKind::Added => ("+", Color32::from_rgb(0xE6, 0xFF, 0xE6)),
                            LineKind::Removed => ("-", Color32::from_rgb(0xFF, 0xE6, 0xE6)),
                            LineKind::Context => (" ", win95::theme::WHITE),
                        };
                        let num = |n: Option<u32>| {
                            n.map(|v| format!("{v:>5}"))
                                .unwrap_or_else(|| "     ".into())
                        };
                        let prefix = format!("{} {} {sign} ", num(l.old_no), num(l.new_no));
                        let text = l.text.trim_end_matches(['\n', '\r']);
                        let job = crate::highlight::colored_line(
                            &prefix,
                            text,
                            p.file_colors.line(hi, li),
                            "",
                            mono.clone(),
                            Color32::TRANSPARENT,
                        );
                        let resp = crate::highlight::diff_row(ui, job, ROW_HEIGHT, bg);
                        if can_comment && let Some((line, side)) = crate::pr_diff::line_target(l) {
                            resp.context_menu(|ui| {
                                if ui.button(s::ADD_COMMENT).clicked() {
                                    comment_on =
                                        Some((diff.path.clone(), line, side, text.to_string()));
                                }
                            });
                        }
                    }
                    FileRow::Comment(t, c) => {
                        let thread = &d.threads[t];
                        let comment = &thread.comments[c];
                        let first = comment.body.lines().next().unwrap_or("");
                        let mut suffix = String::new();
                        if thread.resolved {
                            suffix = format!("  ({})", s::RESOLVED);
                        }
                        let job = crate::highlight::colored_line(
                            &format!("              > {}: ", comment.author),
                            first,
                            None,
                            &suffix,
                            mono.clone(),
                            Color32::TRANSPARENT,
                        );
                        let resp = crate::highlight::diff_row(ui, job, ROW_HEIGHT, COMMENT_BG);
                        if can_comment {
                            resp.context_menu(|ui| {
                                if ui.button(s::REPLY).clicked() {
                                    reply_to = Some(comment.id);
                                }
                                if let Some(resolve) = resolve_action(thread)
                                    && ui
                                        .add_enabled(
                                            !busy,
                                            egui::Button::new(resolve_label(resolve)),
                                        )
                                        .clicked()
                                {
                                    toggle = Some((thread.id.clone(), resolve));
                                }
                            });
                        }
                    }
                    FileRow::Pending(i) => {
                        let c = &p.pending[i];
                        let first = c.body.lines().next().unwrap_or("");
                        let job = crate::highlight::colored_line(
                            &format!("              > {}: ", s::PENDING_TAG),
                            first,
                            None,
                            "",
                            mono.clone(),
                            Color32::TRANSPARENT,
                        );
                        let resp = crate::highlight::diff_row(ui, job, ROW_HEIGHT, PENDING_BG);
                        resp.context_menu(|ui| {
                            if ui.button(s::DISCARD_PENDING).clicked() {
                                drop_pending = Some(i);
                            }
                        });
                    }
                }
            }
        });
    let p = &mut cx.state.pulls;
    if let Some((path, line, side, quote)) = comment_on {
        p.dialog = Some(PullDialog::LineComment {
            path,
            line,
            side,
            quote,
            body: String::new(),
        });
    }
    if let Some((id, resolve)) = toggle {
        send_resolve(cx, d.summary.number, id, resolve);
    }
    let p = &mut cx.state.pulls;
    if let Some(comment_id) = reply_to {
        p.dialog = Some(PullDialog::Reply {
            comment_id,
            body: String::new(),
        });
    }
    if let Some(i) = drop_pending {
        p.pending.remove(i);
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use github::{LineComment, ReviewThread, ThreadComment};

    #[test]
    fn comments_are_listed_under_their_line() {
        let diff = crate::pr_diff::parse_patch("a.rs", Some("@@ -1,2 +1,2 @@\n a\n-b\n+B"));
        let thread = ReviewThread {
            id: "PRRT_1".into(),
            can_resolve: true,
            can_unresolve: false,
            path: "a.rs".into(),
            line: Some(2),
            original_line: Some(2),
            side: DiffSide::Right,
            outdated: false,
            resolved: false,
            comments: vec![
                ThreadComment {
                    id: 1,
                    author: "bob".into(),
                    body: "Why?".into(),
                    at: String::new(),
                },
                ThreadComment {
                    id: 2,
                    author: "ada".into(),
                    body: "Because".into(),
                    at: String::new(),
                },
            ],
        };
        let pending = vec![LineComment {
            path: "a.rs".into(),
            line: 2,
            side: DiffSide::Left,
            body: "keep b".into(),
        }];
        assert_eq!(
            file_rows(&diff, &[thread], &pending),
            [
                FileRow::Hunk(0),
                FileRow::Line(0, 0),
                FileRow::Line(0, 1),
                FileRow::Pending(0),
                FileRow::Line(0, 2),
                FileRow::Comment(0, 0),
                FileRow::Comment(0, 1),
            ]
        );
    }

    #[test]
    fn threads_offer_resolve_or_unresolve_when_allowed() {
        let t = |resolved, can_resolve, can_unresolve| ReviewThread {
            id: "PRRT_1".into(),
            can_resolve,
            can_unresolve,
            path: "a".into(),
            line: Some(1),
            original_line: Some(1),
            side: DiffSide::Right,
            outdated: false,
            resolved,
            comments: vec![],
        };
        assert_eq!(resolve_action(&t(false, true, false)), Some(true));
        assert_eq!(resolve_action(&t(true, false, true)), Some(false));
        assert_eq!(
            resolve_action(&t(false, false, false)),
            None,
            "no permission"
        );
        assert_eq!(resolve_action(&t(true, true, false)), None);
    }

    #[test]
    fn check_durations() {
        assert_eq!(
            duration_text(Some("2026-10-01T08:34:06Z"), Some("2026-10-01T08:36:08Z")),
            "2m 02s"
        );
        assert_eq!(duration_text(None, Some("2026-10-01T08:36:08Z")), "");
    }
}
