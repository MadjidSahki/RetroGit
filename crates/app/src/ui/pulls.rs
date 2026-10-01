//! Pull Requests tab: toolbar, list of pull requests on the left, detail on the right.

use egui::{Align2, Color32, ScrollArea, Sense, pos2, vec2};
use github::{ChecksState, PrFilter, PrState, PrSummary, ReviewDecision};
use win95::{Bevel, Button95, bevel_frame, combo_box};

use super::Ctx;
use crate::protocol::{Command, Slug};
use crate::state::PullDialog;
use crate::strings as s;

const ROW_HEIGHT: f32 = 38.0;
pub const GREEN: Color32 = Color32::from_rgb(0x00, 0x80, 0x00);
pub const RED: Color32 = Color32::from_rgb(0xC0, 0x00, 0x00);
pub const AMBER: Color32 = Color32::from_rgb(0xC0, 0x80, 0x00);

pub fn filter_label(f: PrFilter) -> &'static str {
    match f {
        PrFilter::Open => s::FILTER_OPEN,
        PrFilter::Mine => s::FILTER_MINE,
        PrFilter::ReviewRequested => s::FILTER_REVIEW_REQUESTED,
        PrFilter::Closed => s::FILTER_CLOSED,
    }
}

/// Text and color of a checks state (`None` when there are no checks).
pub fn checks_text(c: ChecksState) -> Option<(&'static str, Color32)> {
    match c {
        ChecksState::Success => Some((s::CHECKS_PASSED, GREEN)),
        ChecksState::Failure => Some((s::CHECKS_FAILED, RED)),
        ChecksState::Pending => Some((s::CHECKS_RUNNING, AMBER)),
        ChecksState::None => None,
    }
}

pub fn review_text(r: ReviewDecision) -> Option<(&'static str, Color32)> {
    match r {
        ReviewDecision::Approved => Some((s::REVIEW_APPROVED, GREEN)),
        ReviewDecision::ChangesRequested => Some((s::REVIEW_CHANGES, RED)),
        ReviewDecision::ReviewRequired => Some((s::REVIEW_REQUIRED, AMBER)),
        ReviewDecision::None => None,
    }
}

/// "ada -> main · 2h ago · checks passed · approved" (second line of a list row).
pub fn row_details(p: &PrSummary, now: i64) -> Vec<(String, Color32)> {
    let mut parts = vec![(format!("{} -> {}", p.author, p.base), win95::theme::BLACK)];
    if let Some(t) = crate::format::parse_iso8601(&p.updated_at) {
        parts.push((super::history::relative_time(now, t), win95::theme::GRAY));
    }
    match p.state {
        PrState::Merged => {
            parts.push((s::STATE_MERGED.into(), Color32::from_rgb(0x60, 0x20, 0x90)))
        }
        PrState::Closed => parts.push((s::STATE_CLOSED.into(), RED)),
        PrState::Open if p.draft => parts.push((s::DRAFT.into(), win95::theme::GRAY)),
        PrState::Open => {}
    }
    if let Some((t, c)) = checks_text(p.checks) {
        parts.push((t.into(), c));
    }
    if let Some((t, c)) = review_text(p.review_decision) {
        parts.push((t.into(), c));
    }
    parts
}

pub fn show(ui: &mut egui::Ui, cx: &mut Ctx<'_>) {
    let Some(slug) = cx.state.github_slug() else {
        bevel_frame(ui, Bevel::Field, win95::theme::WHITE, 8, |ui| {
            ui.set_min_size(ui.available_size());
            ui.label(s::NOT_GITHUB);
        });
        return;
    };
    if cx.state.auth == crate::state::Auth::SignedOut {
        bevel_frame(ui, Bevel::Field, win95::theme::WHITE, 8, |ui| {
            ui.set_min_size(ui.available_size());
            ui.label(s::PULLS_SIGN_IN);
        });
        return;
    }
    let p = &mut cx.state.pulls;
    if p.stale && !p.loading {
        p.stale = false;
        p.loading = true;
        cx.worker.send(Command::LoadPulls {
            slug: slug.clone(),
            filter: p.filter,
        });
    }
    if p.load_selected {
        p.load_selected = false;
        if let Some(number) = p.selected {
            cx.worker.send(Command::LoadPull {
                slug: slug.clone(),
                number,
            });
        }
    }
    toolbar(ui, cx, &slug);
    ui.add_space(2.0);
    egui::Panel::left("pulls_list")
        .frame(egui::Frame::NONE)
        .resizable(true)
        .default_size(330.0)
        .min_size(200.0)
        .max_size((ui.available_width() - 260.0).max(200.0))
        .show(ui, |ui| list(ui, cx, &slug));
    egui::CentralPanel::default()
        .frame(egui::Frame::NONE.inner_margin(egui::Margin {
            left: 4,
            ..Default::default()
        }))
        .show(ui, |ui| super::pull_detail::show(ui, cx, &slug));
}

fn toolbar(ui: &mut egui::Ui, cx: &mut Ctx<'_>, slug: &Slug) {
    ui.horizontal(|ui| {
        let current = cx.state.pulls.filter;
        let mut picked = None;
        combo_box(ui, "pull_filter", filter_label(current), 150.0, |ui| {
            for f in [
                PrFilter::Open,
                PrFilter::Mine,
                PrFilter::ReviewRequested,
                PrFilter::Closed,
            ] {
                if ui.selectable_label(f == current, filter_label(f)).clicked() {
                    picked = Some(f);
                }
            }
        });
        if let Some(f) = picked.filter(|f| *f != current) {
            let p = &mut cx.state.pulls;
            p.filter = f;
            p.list.clear();
            p.loading = true;
            cx.worker.send(Command::LoadPulls {
                slug: slug.clone(),
                filter: f,
            });
        }
        let size = egui::vec2(70.0, 22.0);
        if ui.add(Button95::new(s::REFRESH).min_size(size)).clicked() {
            let p = &mut cx.state.pulls;
            p.loading = true;
            cx.worker.send(Command::LoadPulls {
                slug: slug.clone(),
                filter: p.filter,
            });
            if let Some(number) = p.selected {
                cx.worker.send(Command::LoadPull {
                    slug: slug.clone(),
                    number,
                });
            }
        }
        if ui
            .add(Button95::new(s::NEW_PULL).min_size(egui::vec2(120.0, 22.0)))
            .clicked()
        {
            open_create(cx, slug);
        }
    });
}

/// Open the "New pull request" dialog for the current branch.
pub fn open_create(cx: &mut Ctx<'_>, slug: &Slug) {
    use crate::protocol::{AppError, Severity};
    let Some(branch) = super::sync_toolbar::current_branch(&cx.state.branches).cloned() else {
        cx.state
            .messages
            .push_back(AppError::new(Severity::Info, s::NEEDS_BRANCH));
        return;
    };
    let last = cx
        .state
        .current
        .as_ref()
        .and_then(|c| c.last_commit.as_ref())
        .map(|c| c.summary.clone());
    let base = cx
        .state
        .pulls
        .meta
        .as_ref()
        .map(|m| m.default_branch.clone())
        .unwrap_or_default();
    cx.state.pulls.dialog = Some(PullDialog::Create {
        title: crate::state::prefill_title(last.as_deref(), &branch.name),
        body: String::new(),
        base,
        draft: false,
        labels: Vec::new(),
        publish: branch.upstream.is_none(),
    });
    cx.worker.send(Command::LoadRepoMeta(slug.clone()));
}

fn list(ui: &mut egui::Ui, cx: &mut Ctx<'_>, slug: &Slug) {
    bevel_frame(ui, Bevel::Field, win95::theme::WHITE, 1, |ui| {
        ui.set_min_size(ui.available_size());
        let p = &cx.state.pulls;
        if p.list.is_empty() {
            ui.label(if p.loading {
                s::LOADING_PULLS
            } else {
                s::NO_PULLS
            });
            return;
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let font = win95::theme::font(win95::theme::FONT_SIZE);
        let mut clicked = None;
        ScrollArea::vertical()
            .auto_shrink([false, false])
            .show_rows(ui, ROW_HEIGHT, p.list.len(), |ui, range| {
                ui.spacing_mut().item_spacing.y = 0.0;
                for pr in &p.list[range] {
                    let (rect, resp) = ui.allocate_exact_size(
                        vec2(ui.available_width(), ROW_HEIGHT),
                        Sense::click(),
                    );
                    let selected = p.selected == Some(pr.number);
                    let painter = ui.painter().with_clip_rect(rect);
                    if selected {
                        painter.rect_filled(rect, 0.0, win95::theme::NAVY);
                    }
                    let fg = |c: Color32| if selected { win95::theme::WHITE } else { c };
                    let top = rect.top() + 10.0;
                    let bottom = rect.top() + 28.0;
                    let title = format!("#{} {}", pr.number, pr.title);
                    painter.text(
                        pos2(rect.left() + 4.0, top),
                        Align2::LEFT_CENTER,
                        title,
                        font.clone(),
                        fg(win95::theme::BLACK),
                    );
                    let mut x = rect.left() + 4.0;
                    for (text, color) in row_details(pr, now) {
                        let r = painter.text(
                            pos2(x, bottom),
                            Align2::LEFT_CENTER,
                            &text,
                            font.clone(),
                            fg(color),
                        );
                        x = r.right() + 10.0;
                    }
                    for label in &pr.labels {
                        let chip =
                            win95::paint_chip(&painter, pos2(x, bottom), &label.name, label.color);
                        x = chip.right() + 4.0;
                    }
                    painter.line_segment(
                        [
                            pos2(rect.left(), rect.bottom() - 0.5),
                            pos2(rect.right(), rect.bottom() - 0.5),
                        ],
                        egui::Stroke::new(1.0, win95::theme::LIGHT),
                    );
                    if resp.clicked() {
                        clicked = Some(pr.number);
                    }
                }
            });
        if let Some(number) = clicked {
            cx.state.pulls.select(number);
            cx.worker.send(Command::LoadPull {
                slug: slug.clone(),
                number,
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pr() -> PrSummary {
        PrSummary {
            number: 1,
            title: "t".into(),
            url: String::new(),
            author: "ada".into(),
            head: "x".into(),
            base: "main".into(),
            draft: true,
            state: PrState::Open,
            labels: vec![],
            checks: ChecksState::Failure,
            review_decision: ReviewDecision::Approved,
            updated_at: "2026-10-01T10:00:00Z".into(),
        }
    }

    #[test]
    fn row_details_say_who_where_when_and_how() {
        let now = crate::format::parse_iso8601("2026-10-01T12:00:00Z").unwrap_or(0);
        let texts: Vec<String> = row_details(&pr(), now)
            .into_iter()
            .map(|(t, _)| t)
            .collect();
        assert_eq!(
            texts,
            [
                "ada -> main",
                "2h ago",
                s::DRAFT,
                s::CHECKS_FAILED,
                s::REVIEW_APPROVED
            ]
        );
        let mut merged = pr();
        merged.state = PrState::Merged;
        merged.checks = ChecksState::None;
        merged.review_decision = ReviewDecision::None;
        merged.updated_at = String::new();
        let texts: Vec<String> = row_details(&merged, now)
            .into_iter()
            .map(|(t, _)| t)
            .collect();
        assert_eq!(texts, ["ada -> main", s::STATE_MERGED]);
    }
}
