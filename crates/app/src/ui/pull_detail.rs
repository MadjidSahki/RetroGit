//! Detail of the selected pull request: header, labels, actions and sub-tabs.

use egui::{Color32, RichText, ScrollArea};
use gitcore::LineKind;
use github::{CheckStatus, DiffSide, PrDetail, PrState, ReviewState, TimelineItem};
use win95::FlatRows;
use win95::{Bevel, Button95, bevel_frame, label_chip, markdown_view, text_area};

use super::Ctx;
use super::diff_view::ROW_HEIGHT;
use crate::protocol::{Command, Slug};
use crate::state::{
    PullDialog, PullTab, default_merge_method, merge_defaults, merge_disabled_reason,
    review_events_allowed,
};
use crate::strings as s;

pub fn state_text(pal: &win95::Palette, d: &PrDetail) -> (&'static str, Color32) {
    match d.summary.state {
        PrState::Merged => (s::STATE_MERGED, pal.merged),
        PrState::Closed => (s::STATE_CLOSED, pal.error),
        PrState::Open if d.summary.draft => (s::DRAFT, pal.gray_text),
        PrState::Open => (s::STATE_OPEN, pal.success),
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

/// "Loading..." or, if loading the selected pull request failed, why.
fn loading(ui: &mut egui::Ui, p: &crate::state::PullsView) {
    match &p.load_error {
        Some((n, err)) if p.selected == Some(*n) => {
            ui.label(RichText::new(err.as_str()).color(win95::theme::palette(ui.ctx()).error))
        }
        _ => ui.label(s::LOADING_PULL),
    };
}

/// The whole comment, shown on hover when only its first line fits in the row.
pub fn comment_hover(body: &str) -> Option<&str> {
    body.trim_end().contains('\n').then_some(body)
}

pub fn show(ui: &mut egui::Ui, cx: &mut Ctx<'_>, slug: &Slug) {
    bevel_frame(
        ui,
        Bevel::Field,
        win95::theme::palette(ui.ctx()).window,
        4,
        |ui| {
            ui.set_min_size(ui.available_size());
            let p = &cx.state.pulls;
            if p.selected.is_none() {
                ui.label(s::SELECT_A_PULL);
                return;
            }
            let Some(d) = p.detail.clone() else {
                loading(ui, p);
                return;
            };
            // Checks running: reload now and then, so Merge follows them.
            if let Some(at) = cx.state.pulls.loaded_at {
                let now = std::time::Instant::now();
                if crate::state::needs_auto_refresh(&d, at, now)
                    && let Some(slug) = cx.state.github_slug()
                {
                    cx.state.pulls.loaded_at = Some(now);
                    cx.worker.send(Command::RefreshPull {
                        slug,
                        number: d.summary.number,
                    });
                } else if d.summary.checks == github::ChecksState::Pending {
                    ui.ctx().request_repaint_after(crate::state::CHECKS_REFRESH);
                }
            }
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
        },
    );
}

fn header(ui: &mut egui::Ui, cx: &mut Ctx<'_>, d: &PrDetail) {
    let navy = win95::theme::palette(ui.ctx()).link;
    let busy = cx.state.pulls.busy;
    ui.horizontal_top(|ui| {
        // The title wraps in the room left by the Edit button (a label in a horizontal
        // layout never wraps: a long title used to push Edit off screen).
        let reserve = if d.viewer_can_update { 70.0 } else { 0.0 };
        let width = (ui.available_width() - reserve).max(80.0);
        ui.allocate_ui_with_layout(
            egui::vec2(width, 0.0),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.set_max_width(width);
                ui.add(
                    egui::Label::new(
                        RichText::new(format!("#{} {}", d.summary.number, d.summary.title))
                            .size(win95::theme::FONT_SIZE + 3.0)
                            .color(navy),
                    )
                    .wrap(),
                );
            },
        );
        if d.viewer_can_update && ui.add(Button95::new(s::EDIT_PULL).enabled(!busy)).clicked() {
            cx.state.pulls.dialog = Some(PullDialog::EditPull {
                title: d.summary.title.clone(),
                body: d.body.clone(),
            });
        }
    });
    let (state, color) = state_text(&win95::theme::palette(ui.ctx()), d);
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(state).color(color));
        let what = s::WANTS_TO_MERGE
            .replace("{head}", &d.summary.head)
            .replace("{base}", &d.summary.base);
        ui.label(format!("{} {what}", d.summary.author));
        ui.label(
            RichText::new(format!("({} @{})", s::AS_ACCOUNT, d.viewer))
                .color(win95::theme::palette(ui.ctx()).gray_text),
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
            let names: Vec<String> = d.summary.labels.iter().map(|l| l.name.clone()).collect();
            cx.state.pulls.dialog = Some(PullDialog::Labels {
                old: names.clone(),
                checked: names,
            });
        }
    });
    people_row(ui, cx, d);
    ui.horizontal(|ui| {
        let busy = cx.state.pulls.busy;
        let size = egui::vec2(80.0, 22.0);
        let open = d.summary.state == PrState::Open;
        if ui
            .add(
                Button95::new(s::CHECKOUT)
                    .min_size(size)
                    .enabled(open && !busy && cx.state.sync.running.is_none()),
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
            ui.label(RichText::new(why).color(win95::theme::palette(ui.ctx()).gray_text));
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
        if open && d.viewer_can_update {
            let (label, draft) = if d.summary.draft {
                (s::READY_FOR_REVIEW, false)
            } else {
                (s::CONVERT_TO_DRAFT, true)
            };
            if ui
                .add(
                    Button95::new(label)
                        .min_size(egui::vec2(120.0, 22.0))
                        .enabled(!busy),
                )
                .clicked()
                && let Some(slug) = cx.state.github_slug()
            {
                cx.state.pulls.busy = true;
                cx.worker.send(Command::SetDraft {
                    slug,
                    number: d.summary.number,
                    pull_id: d.id.clone(),
                    draft,
                });
            }
        }
        if ui
            .add(Button95::new(s::OPEN_ON_GITHUB).min_size(egui::vec2(110.0, 22.0)))
            .clicked()
        {
            ui.ctx().open_url(egui::OpenUrl::new_tab(&d.summary.url));
        }
    });
    if !cx.state.pulls.selected_pending().is_empty() {
        // Shown until the review is submitted.
        ui.label(
            RichText::new(format!(
                "{} {}",
                cx.state.pulls.selected_pending().len(),
                s::PENDING_COMMENTS
            ))
            .color(win95::theme::palette(ui.ctx()).warning),
        );
    }
}

/// "@carol", or "@dan (approved)" once reviewed.
pub fn reviewer_text(r: &github::Reviewer) -> String {
    match r.state {
        None => format!("@{}", r.login),
        Some(state) => format!("@{} ({})", r.login, review_verb(state)),
    }
}

fn people_row(ui: &mut egui::Ui, cx: &mut Ctx<'_>, d: &PrDetail) {
    let busy = cx.state.pulls.busy;
    let mut open: Option<crate::state::PeopleKind> = None;
    ui.horizontal_wrapped(|ui| {
        ui.label(s::REVIEWERS);
        if d.reviewers.is_empty() {
            ui.label(RichText::new(s::NOBODY).color(win95::theme::palette(ui.ctx()).gray_text));
        }
        for r in &d.reviewers {
            ui.label(reviewer_text(r));
        }
        if d.viewer_can_update
            && ui
                .add(Button95::new(s::EDIT_REVIEWERS).enabled(!busy))
                .clicked()
        {
            open = Some(crate::state::PeopleKind::Reviewers);
        }
        ui.separator();
        ui.label(s::ASSIGNEES);
        if d.assignees.is_empty() {
            ui.label(RichText::new(s::NOBODY).color(win95::theme::palette(ui.ctx()).gray_text));
        }
        for a in &d.assignees {
            ui.label(format!("@{a}"));
        }
        if d.viewer_can_update
            && ui
                .add(Button95::new(s::EDIT_ASSIGNEES).enabled(!busy))
                .clicked()
        {
            open = Some(crate::state::PeopleKind::Assignees);
        }
    });
    if let Some(kind) = open {
        let checked = match kind {
            crate::state::PeopleKind::Reviewers => d
                .reviewers
                .iter()
                .filter(|r| r.state.is_none())
                .map(|r| r.login.clone())
                .collect(),
            crate::state::PeopleKind::Assignees => d.assignees.clone(),
        };
        cx.state.pulls.dialog = Some(PullDialog::People {
            kind,
            checked,
            filter: String::new(),
        });
        if let Some(slug) = cx.state.github_slug() {
            cx.state.pulls.assignable_query = Some(String::new());
            cx.worker.send(Command::LoadAssignable {
                slug,
                query: String::new(),
            });
        }
    }
}

fn boxed(ui: &mut egui::Ui, title: &str, body: &str) {
    egui::Frame::NONE
        .fill(win95::theme::palette(ui.ctx()).comment_bg)
        .stroke(egui::Stroke::new(
            1.0,
            win95::theme::palette(ui.ctx()).light,
        ))
        .inner_margin(egui::Margin::same(6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(title).color(win95::theme::palette(ui.ctx()).link));
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
                ui.label(
                    RichText::new(s::NO_DESCRIPTION)
                        .color(win95::theme::palette(ui.ctx()).gray_text),
                );
            } else {
                markdown_view(ui, &d.body);
            }
            ui.separator();
            truncated_notes(ui, d);
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
                let title = thread_title(t);
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

/// One line per conversation connection GitHub sent only part of, and a link to the rest.
fn truncated_notes(ui: &mut egui::Ui, d: &PrDetail) {
    let reviews = d
        .timeline
        .iter()
        .filter(|t| matches!(t, TimelineItem::Review { .. }))
        .count();
    let comments = d.timeline.len() - reviews;
    let notes: Vec<String> = [
        (comments, d.comments_total, s::TRUNCATED_COMMENTS),
        (reviews, d.reviews_total, s::TRUNCATED_REVIEWS),
        (d.threads.len(), d.threads_total, s::TRUNCATED_THREADS),
    ]
    .into_iter()
    // Pending reviews are read but not shown: only a cut connection counts.
    .filter(|(_, total, _)| *total > github::DETAIL_PAGE)
    .map(|(shown, total, what)| s::detail_truncated(shown, total, what))
    .collect();
    if notes.is_empty() {
        return;
    }
    let gray = win95::theme::palette(ui.ctx()).gray_text;
    for n in notes {
        ui.label(RichText::new(n).color(gray));
    }
    ui.hyperlink_to(s::OPEN_ON_GITHUB, &d.summary.url);
    ui.separator();
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
                    // No forced color: the hovered row uses the selection's text color.
                    .selectable_label(false, text)
                    .clicked()
                {
                    open = Some(c.oid.clone());
                }
            }
            if d.commit_count as usize > d.commits.len() {
                ui.label(
                    RichText::new(s::commits_truncated(d.commits.len(), d.commit_count))
                        .color(win95::theme::palette(ui.ctx()).gray_text),
                );
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
                            CheckStatus::Success => {
                                (s::CHECK_PASSED, win95::theme::palette(ui.ctx()).success)
                            }
                            CheckStatus::Failure => {
                                (s::CHECK_FAILED, win95::theme::palette(ui.ctx()).error)
                            }
                            CheckStatus::Pending => {
                                (s::CHECK_RUNNING, win95::theme::palette(ui.ctx()).warning)
                            }
                            CheckStatus::Neutral => {
                                (s::CHECK_SKIPPED, win95::theme::palette(ui.ctx()).gray_text)
                            }
                        };
                        ui.label(RichText::new(text).color(color));
                        ui.label(&c.name);
                        ui.label(duration_text(
                            c.started_at.as_deref(),
                            c.completed_at.as_deref(),
                        ));
                        match &c.url {
                            Some(url) if win95::markdown::safe_link(url) => {
                                ui.hyperlink_to(s::CHECK_DETAILS, url)
                            }
                            Some(url) => ui.label(s::CHECK_DETAILS).on_hover_text(url),
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
    /// A comment's suggestion: the current lines (`Old`), the proposed ones (`New`), then
    /// the Apply row. Thread index, comment index, line index.
    SuggestionOld(usize, usize, usize),
    SuggestionNew(usize, usize, usize),
    SuggestionApply(usize, usize),
}

/// Title of a thread in the Conversation tab: `path:N` or `path lines a-b`, ` (old)`
/// before the line on the old side, then outdated (no longer placed) or resolved.
pub fn thread_title(t: &github::ReviewThread) -> String {
    let side = if t.side == github::DiffSide::Left {
        format!(" ({})", s::OLD_SIDE)
    } else {
        String::new()
    };
    let line = match thread_range(t).filter(|(a, b)| a < b) {
        Some((a, b)) => format!(
            " {}",
            s::LINES_RANGE
                .replace("{a}", &a.to_string())
                .replace("{b}", &b.to_string())
        ),
        None => t
            .line
            .or(t.original_line)
            .map(|l| format!(":{l}"))
            .unwrap_or_default(),
    };
    let tag = if t.outdated && t.line.is_none() {
        format!(" ({})", s::OUTDATED)
    } else if t.resolved {
        format!(" ({})", s::RESOLVED)
    } else {
        String::new()
    };
    format!("{}{side}{line}{tag}", t.path)
}

/// Lines `start..=end` (first..last) a thread is about.
pub fn thread_range(t: &github::ReviewThread) -> Option<(u32, u32)> {
    let end = t.line?;
    Some((t.start_line.filter(|s| *s <= end).unwrap_or(end), end))
}

/// Text of the new-side lines `start..=end` of `diff`, if all are shown in it.
pub fn new_side_lines(diff: &gitcore::FileDiff, start: u32, end: u32) -> Option<Vec<String>> {
    let found: Vec<(u32, String)> = diff
        .hunks
        .iter()
        .flat_map(|h| &h.lines)
        .filter_map(|l| {
            let n = l.new_no?;
            (n >= start && n <= end).then(|| (n, l.text.trim_end_matches(['\n', '\r']).to_string()))
        })
        .collect();
    (found.len() as u32 == end - start + 1).then(|| found.into_iter().map(|(_, t)| t).collect())
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
                let thread = &threads[t];
                for (c, comment) in thread.comments.iter().enumerate() {
                    out.push(FileRow::Comment(t, c));
                    let Some(code) = github::suggestions(&comment.body).into_iter().next() else {
                        continue;
                    };
                    let old = thread_range(thread)
                        .and_then(|(a, b)| new_side_lines(diff, a, b))
                        .map_or(0, |l| l.len());
                    out.extend((0..old).map(|k| FileRow::SuggestionOld(t, c, k)));
                    out.extend((0..code.lines().count()).map(|k| FileRow::SuggestionNew(t, c, k)));
                    out.push(FileRow::SuggestionApply(t, c));
                }
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
        loading(ui, &cx.state.pulls);
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

/// A line comment to open: path, last line, side, quoted text, first line, prefilled body.
type CommentOn = (String, u32, DiffSide, String, Option<u32>, Option<String>);

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
    let rows = file_rows(diff, &d.threads, p.selected_pending());
    let mut comment_on: Option<CommentOn> = None;
    let mut select: Option<(usize, usize, bool)> = None;
    let selection = p.selection;
    let branch = cx
        .state
        .branches
        .iter()
        .find(|b| b.is_head && !b.remote)
        .map(|b| b.name.clone());
    let mut apply: Option<Command> = None;
    let mut reply_to: Option<u64> = None;
    let mut toggle: Option<(String, bool)> = None;
    let busy = p.busy;
    let mut drop_pending: Option<usize> = None;
    ScrollArea::both()
        .id_salt(("pull_file_diff", d.summary.number, &diff.path))
        .auto_shrink([false, false])
        .show_rows_flat(ui, ROW_HEIGHT, rows.len(), |ui, range| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for row in &rows[range] {
                match *row {
                    FileRow::Hunk(hi) => {
                        let mut job = crate::highlight::colored_line(
                            &win95::theme::palette(ui.ctx()),
                            "",
                            &diff.hunks[hi].header,
                            None,
                            "",
                            mono.clone(),
                            Color32::TRANSPARENT,
                        );
                        for section in &mut job.sections {
                            section.format.color = win95::theme::palette(ui.ctx()).link;
                        }
                        crate::highlight::diff_row(
                            ui,
                            job,
                            ROW_HEIGHT,
                            win95::theme::palette(ui.ctx()).hunk,
                        );
                    }
                    FileRow::Line(hi, li) => {
                        let l = &diff.hunks[hi].lines[li];
                        let (sign, bg) = match l.kind {
                            LineKind::Added => ("+", win95::theme::palette(ui.ctx()).added),
                            LineKind::Removed => ("-", win95::theme::palette(ui.ctx()).removed),
                            LineKind::Context => (" ", win95::theme::palette(ui.ctx()).window),
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
                            p.file_colors.line(hi, li),
                            "",
                            mono.clone(),
                            Color32::TRANSPARENT,
                        );
                        let in_selection = selection
                            .is_some_and(|sel| sel.hunk == hi && (sel.from..=sel.to).contains(&li));
                        let bg = if in_selection {
                            win95::theme::palette(ui.ctx()).line_selected
                        } else {
                            bg
                        };
                        let resp = crate::highlight::diff_row(ui, job, ROW_HEIGHT, bg);
                        if resp.clicked() || resp.secondary_clicked() && !in_selection {
                            let shift = ui.input(|i| i.modifiers.shift) && resp.clicked();
                            select = Some((hi, li, shift));
                        }
                        if can_comment {
                            // The selection when right-clicking inside it, else this line.
                            let sel = if in_selection {
                                selection
                            } else {
                                Some(crate::state::LineSelection {
                                    hunk: hi,
                                    from: li,
                                    to: li,
                                })
                            };
                            let target = sel.and_then(|s| crate::state::selection_target(diff, s));
                            if let Some(t) = target {
                                resp.context_menu(|ui| {
                                    let label = if t.start < t.end {
                                        s::COMMENT_ON_LINES
                                            .replace("{a}", &t.start.to_string())
                                            .replace("{b}", &t.end.to_string())
                                    } else {
                                        s::ADD_COMMENT.to_string()
                                    };
                                    let start = (t.start < t.end).then_some(t.start);
                                    if ui.button(label).clicked() {
                                        comment_on = Some((
                                            diff.path.clone(),
                                            t.end,
                                            t.side,
                                            t.lines.join("\n"),
                                            start,
                                            None,
                                        ));
                                    }
                                    if t.side == DiffSide::Right
                                        && ui.button(s::SUGGEST_CHANGE).clicked()
                                    {
                                        comment_on = Some((
                                            diff.path.clone(),
                                            t.end,
                                            t.side,
                                            t.lines.join("\n"),
                                            start,
                                            Some(crate::state::suggestion_prefill(&t)),
                                        ));
                                    }
                                });
                            }
                        }
                    }
                    FileRow::Comment(t, c) => {
                        let thread = &d.threads[t];
                        let comment = &thread.comments[c];
                        let first = comment.body.lines().next().unwrap_or("");
                        let mut suffix = String::new();
                        if c == 0
                            && let Some((a, b)) = thread_range(thread).filter(|(a, b)| a < b)
                        {
                            let range = s::LINES_RANGE
                                .replace("{a}", &a.to_string())
                                .replace("{b}", &b.to_string());
                            suffix = format!("  ({range})");
                        }
                        if thread.resolved {
                            suffix.push_str(&format!("  ({})", s::RESOLVED));
                        }
                        let job = crate::highlight::colored_line(
                            &win95::theme::palette(ui.ctx()),
                            &format!("              > {}: ", comment.author),
                            first,
                            None,
                            &suffix,
                            mono.clone(),
                            Color32::TRANSPARENT,
                        );
                        let resp = crate::highlight::diff_row(
                            ui,
                            job,
                            ROW_HEIGHT,
                            win95::theme::palette(ui.ctx()).comment_bg,
                        );
                        let resp = match comment_hover(&comment.body) {
                            Some(full) => resp.on_hover_text(full),
                            None => resp,
                        };
                        // Resolve as in Conversation, whatever the state; Reply: open only.
                        let resolve_offer = resolve_action(thread);
                        if can_comment || resolve_offer.is_some() {
                            resp.context_menu(|ui| {
                                if can_comment && ui.button(s::REPLY).clicked() {
                                    reply_to = Some(comment.id);
                                }
                                if let Some(resolve) = resolve_offer
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
                    FileRow::SuggestionOld(t, _, k) => {
                        let line = thread_range(&d.threads[t])
                            .and_then(|(a, b)| new_side_lines(diff, a, b))
                            .and_then(|l| l.get(k).cloned())
                            .unwrap_or_default();
                        let job = crate::highlight::colored_line(
                            &win95::theme::palette(ui.ctx()),
                            "              - ",
                            &line,
                            None,
                            "",
                            mono.clone(),
                            Color32::TRANSPARENT,
                        );
                        crate::highlight::diff_row(
                            ui,
                            job,
                            ROW_HEIGHT,
                            win95::theme::palette(ui.ctx()).removed,
                        );
                    }
                    FileRow::SuggestionNew(t, c, k) => {
                        let code = github::suggestions(&d.threads[t].comments[c].body)
                            .into_iter()
                            .next()
                            .unwrap_or_default();
                        let line = code.lines().nth(k).unwrap_or_default().to_string();
                        let job = crate::highlight::colored_line(
                            &win95::theme::palette(ui.ctx()),
                            "              + ",
                            &line,
                            None,
                            "",
                            mono.clone(),
                            Color32::TRANSPARENT,
                        );
                        crate::highlight::diff_row(
                            ui,
                            job,
                            ROW_HEIGHT,
                            win95::theme::palette(ui.ctx()).added,
                        );
                    }
                    FileRow::SuggestionApply(t, c) => {
                        let thread = &d.threads[t];
                        let comment = &thread.comments[c];
                        let range = thread_range(thread);
                        let current = range.and_then(|(a, b)| new_side_lines(diff, a, b));
                        let why = crate::state::apply_disabled_reason_for(
                            d.cross_repository,
                            &d.summary.head,
                            d.summary.number,
                            branch.as_deref(),
                            thread.outdated || current.is_none(),
                            thread.side,
                        );
                        ui.horizontal(|ui| {
                            ui.add_space(110.0);
                            let b = ui.add(
                                Button95::new(s::APPLY_SUGGESTION).enabled(why.is_none() && !busy),
                            );
                            if let Some(why) = why {
                                ui.label(
                                    RichText::new(why)
                                        .color(win95::theme::palette(ui.ctx()).gray_text),
                                );
                            } else if b.clicked()
                                && let (Some((a, b)), Some(expected)) = (range, current)
                            {
                                apply = Some(Command::ApplySuggestion {
                                    number: d.summary.number,
                                    head_branch: d.summary.head.clone(),
                                    head_sha: d.head_sha.clone(),
                                    path: diff.path.clone(),
                                    start: a,
                                    end: b,
                                    expected,
                                    replacement: github::suggestions(&comment.body)
                                        .into_iter()
                                        .next()
                                        .unwrap_or_default(),
                                    author: comment.author.clone(),
                                    author_id: comment.author_id,
                                });
                            }
                        });
                    }
                    FileRow::Pending(i) => {
                        let c = &p.selected_pending()[i];
                        let first = c.body.lines().next().unwrap_or("");
                        let job = crate::highlight::colored_line(
                            &win95::theme::palette(ui.ctx()),
                            &format!("              > {}: ", s::PENDING_TAG),
                            first,
                            None,
                            "",
                            mono.clone(),
                            Color32::TRANSPARENT,
                        );
                        let resp = crate::highlight::diff_row(
                            ui,
                            job,
                            ROW_HEIGHT,
                            win95::theme::palette(ui.ctx()).pending_bg,
                        );
                        let resp = match comment_hover(&c.body) {
                            Some(full) => resp.on_hover_text(full),
                            None => resp,
                        };
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
    if let Some((path, line, side, quote, start, prefill)) = comment_on {
        let body = prefill.unwrap_or_default();
        p.dialog = Some(PullDialog::LineComment {
            number: d.summary.number,
            head_sha: d.head_sha.clone(),
            path,
            line,
            side,
            start,
            quote,
            body,
        });
    }
    if let Some((hunk, line, shift)) = select {
        p.selection = Some(match p.selection {
            Some(sel) if shift => crate::state::extend_selection(sel, hunk, line),
            _ => crate::state::LineSelection {
                hunk,
                from: line,
                to: line,
            },
        });
    }
    if ui.input(|i| i.key_pressed(egui::Key::Escape)) && p.dialog.is_none() {
        p.selection = None;
    }
    if let Some(cmd) = apply {
        p.busy = true;
        cx.worker.send(cmd);
    }
    if let Some((id, resolve)) = toggle {
        send_resolve(cx, d.summary.number, id, resolve);
    }
    let p = &mut cx.state.pulls;
    if let Some(comment_id) = reply_to {
        p.dialog = Some(PullDialog::Reply {
            number: d.summary.number,
            comment_id,
            body: String::new(),
        });
    }
    if let Some(i) = drop_pending {
        p.drop_pending(i);
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
            start_line: None,
            start_side: None,
            outdated: false,
            resolved: false,
            comments: vec![
                ThreadComment {
                    id: 1,
                    author: "bob".into(),
                    author_id: None,
                    body: "Why?".into(),
                    at: String::new(),
                },
                ThreadComment {
                    id: 2,
                    author: "ada".into(),
                    author_id: None,
                    body: "Because".into(),
                    at: String::new(),
                },
            ],
        };
        let pending = vec![LineComment {
            path: "a.rs".into(),
            line: 2,
            side: DiffSide::Left,
            start: None,
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
            start_line: None,
            start_side: None,
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
