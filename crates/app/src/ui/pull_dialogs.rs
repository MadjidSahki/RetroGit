//! Dialogs of the Pull Requests tab: review, merge, new pull request, labels, comments.

use github::{LineComment, Merge, MergeMethod, NewPull, Review, ReviewEvent};
use win95::{Button95, Dialog, checkbox, combo_box, text_area, text_field};

use super::Ctx;
use crate::protocol::{Command, Slug};
use crate::state::{PeopleKind, PullDialog, merge_defaults, review_events_allowed};
use crate::strings as s;

pub fn event_label(e: ReviewEvent) -> &'static str {
    match e {
        ReviewEvent::Comment => s::REVIEW_COMMENT,
        ReviewEvent::Approve => s::REVIEW_APPROVE,
        ReviewEvent::RequestChanges => s::REVIEW_REQUEST_CHANGES,
    }
}

pub fn method_label(m: MergeMethod) -> &'static str {
    match m {
        MergeMethod::Merge => s::MERGE_COMMIT,
        MergeMethod::Squash => s::SQUASH_MERGE,
        MergeMethod::Rebase => s::REBASE_MERGE,
    }
}

/// GitHub needs some text for a "comment" or "request changes" review, unless line
/// comments carry it.
pub fn review_ready(event: ReviewEvent, body: &str, pending: usize) -> bool {
    match event {
        ReviewEvent::Approve => true,
        ReviewEvent::Comment => !body.trim().is_empty() || pending > 0,
        ReviewEvent::RequestChanges => !body.trim().is_empty(),
    }
}

/// Branches a pull request can target: local and remote names, without `origin/`, minus
/// the head branch, sorted and unique.
pub fn base_candidates(branches: &[gitcore::Branch], head: &str) -> Vec<String> {
    let mut out: Vec<String> = branches
        .iter()
        .map(|b| {
            if b.remote {
                b.name
                    .split_once('/')
                    .map_or(b.name.clone(), |(_, n)| n.to_string())
            } else {
                b.name.clone()
            }
        })
        .filter(|n| n != head && n != "HEAD")
        .collect();
    out.sort();
    out.dedup();
    out
}

enum Outcome {
    Keep(PullDialog),
    Close,
    /// Send, and keep the dialog until the worker answers (`PullActionDone` closes it;
    /// after an error it is still there, with what the user typed).
    Send(Command, PullDialog),
}

const BUTTON: egui::Vec2 = egui::vec2(110.0, 23.0);

pub fn show(egui_ctx: &egui::Context, cx: &mut Ctx<'_>) {
    let Some(slug) = cx.state.github_slug() else {
        cx.state.pulls.dialog = None;
        return;
    };
    let Some(dialog) = cx.state.pulls.dialog.take() else {
        return;
    };
    let outcome = match dialog {
        PullDialog::Review { event, body } => review(egui_ctx, cx, &slug, event, body),
        PullDialog::Merge {
            method,
            title,
            message,
            delete_branch,
        } => merge(egui_ctx, cx, &slug, method, title, message, delete_branch),
        PullDialog::Create {
            title,
            body,
            base,
            draft,
            labels,
            publish,
        } => create(
            egui_ctx,
            cx,
            &slug,
            (title, body, base, draft, labels, publish),
        ),
        PullDialog::Labels { checked } => labels(egui_ctx, cx, &slug, checked),
        PullDialog::EditPull { title, body } => edit_pull(egui_ctx, cx, &slug, title, body),
        PullDialog::People {
            kind,
            checked,
            filter,
        } => people(egui_ctx, cx, &slug, kind, checked, filter),
        PullDialog::LineComment {
            path,
            line,
            side,
            start,
            quote,
            body,
        } => {
            let mut body = body;
            let busy = cx.state.pulls.busy;
            let head = cx.state.pulls.detail.as_ref().map(|d| d.head_sha.clone());
            let number = cx.state.pulls.selected.unwrap_or_default();
            // Like github.com: post it now, or keep it for the review being written.
            let choices: &[&str] = if busy {
                &[]
            } else {
                &[s::ADD_SINGLE_COMMENT, s::ADD_TO_REVIEW]
            };
            let comment = |body: String| LineComment {
                path: path.clone(),
                line,
                side,
                start: start.filter(|s| *s < line).map(|s| (s, side)),
                body,
            };
            let lines = match start.filter(|s| *s < line) {
                Some(first) => format!("{first}-{line}"),
                None => line.to_string(),
            };
            let keep = |body: String| PullDialog::LineComment {
                path: path.clone(),
                line,
                side,
                start,
                quote: quote.clone(),
                body,
            };
            match text_dialog(
                egui_ctx,
                s::LINE_COMMENT_TITLE,
                &format!("{path}:{lines}\n{quote}"),
                &mut body,
                choices,
            ) {
                Some(Some(0)) if head.is_some() => Outcome::Send(
                    Command::AddLineComment {
                        slug: slug.clone(),
                        number,
                        commit_id: head.unwrap_or_default(),
                        comment: comment(body.clone()),
                    },
                    keep(body),
                ),
                Some(Some(_)) => {
                    cx.state.queue_line_comment(comment(body));
                    Outcome::Close
                }
                Some(None) => Outcome::Close,
                None => Outcome::Keep(keep(body)),
            }
        }
        PullDialog::Reply { comment_id, body } => {
            let mut body = body;
            let number = cx.state.pulls.selected.unwrap_or_default();
            let thread = cx
                .state
                .pulls
                .detail
                .as_ref()
                .and_then(|d| {
                    d.threads
                        .iter()
                        .find(|t| t.comments.iter().any(|c| c.id == comment_id))
                })
                .map(|t| {
                    t.comments
                        .iter()
                        .map(|c| format!("{}: {}", c.author, c.body))
                        .collect::<Vec<_>>()
                        .join("\n\n")
                })
                .unwrap_or_default();
            match text_dialog(egui_ctx, s::REPLY_TITLE, &thread, &mut body, &[s::SEND]) {
                Some(Some(_)) => {
                    cx.state.pulls.busy = true;
                    Outcome::Send(
                        Command::ReplyToThread {
                            slug: slug.clone(),
                            number,
                            comment_id,
                            body: body.clone(),
                        },
                        PullDialog::Reply { comment_id, body },
                    )
                }
                Some(None) => Outcome::Close,
                None => Outcome::Keep(PullDialog::Reply { comment_id, body }),
            }
        }
    };
    match outcome {
        Outcome::Keep(d) => cx.state.pulls.dialog = Some(d),
        Outcome::Close => {}
        Outcome::Send(cmd, keep) => {
            cx.state.pulls.busy = true;
            cx.worker.send(cmd);
            cx.state.pulls.dialog = Some(keep);
        }
    }
}

/// Context text, a text box, one button per choice (enabled with some text) and Cancel.
/// `Some(Some(i))` = choice `i`, `Some(None)` = cancelled, `None` = still open.
fn text_dialog(
    egui_ctx: &egui::Context,
    title: &str,
    context: &str,
    body: &mut String,
    choices: &[&str],
) -> Option<Option<usize>> {
    let mut result = None;
    let r = Dialog::new(("pull_text", title), title)
        .width(480.0)
        .show(egui_ctx, |ui| {
            egui::ScrollArea::vertical()
                .max_height(160.0)
                .show(ui, |ui| {
                    ui.add(egui::Label::new(egui::RichText::new(context).monospace()).wrap());
                });
            ui.add_space(4.0);
            text_area(ui, body, 460.0, 5);
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                for (i, choice) in choices.iter().enumerate() {
                    if ui
                        .add(
                            Button95::new(*choice)
                                .min_size(egui::vec2(140.0, 23.0))
                                .enabled(!body.trim().is_empty()),
                        )
                        .clicked()
                    {
                        result = Some(Some(i));
                    }
                }
                if ui.add(Button95::new(s::CANCEL).min_size(BUTTON)).clicked() {
                    result = Some(None);
                }
            });
        });
    if r.close_requested {
        return Some(None);
    }
    result
}

fn review(
    egui_ctx: &egui::Context,
    cx: &mut Ctx<'_>,
    slug: &Slug,
    mut event: ReviewEvent,
    mut body: String,
) -> Outcome {
    let Some(d) = cx.state.pulls.detail.clone() else {
        return Outcome::Close;
    };
    let allowed = review_events_allowed(&d);
    let pending = cx.state.pulls.pending.len();
    let busy = cx.state.pulls.busy;
    let mut submit = false;
    let mut cancel = false;
    let r = Dialog::new("pull_review", s::REVIEW_TITLE)
        .width(480.0)
        .show(egui_ctx, |ui| {
            text_area(ui, &mut body, 460.0, 6);
            ui.add_space(4.0);
            for e in [
                ReviewEvent::Comment,
                ReviewEvent::Approve,
                ReviewEvent::RequestChanges,
            ] {
                let enabled = allowed.contains(&e);
                ui.add_enabled_ui(enabled, |ui| {
                    // egui's radio circle is invisible in the Win95 theme when unselected.
                    if ui.selectable_label(event == e, event_label(e)).clicked() {
                        event = e;
                    }
                });
            }
            if d.viewer_is_author {
                ui.label(
                    egui::RichText::new(s::OWN_PULL_REVIEW)
                        .color(win95::theme::palette(ui.ctx()).gray_text),
                );
            }
            if pending > 0 {
                ui.label(format!("{pending} {}", s::PENDING_IN_REVIEW));
            }
            let ready = review_ready(event, &body, pending);
            if !ready {
                ui.label(
                    egui::RichText::new(s::REVIEW_NEEDS_TEXT)
                        .color(win95::theme::palette(ui.ctx()).gray_text),
                );
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui
                    .add(
                        Button95::new(s::REVIEW_SUBMIT)
                            .min_size(BUTTON)
                            .enabled(ready && !busy),
                    )
                    .clicked()
                {
                    submit = true;
                }
                if ui.add(Button95::new(s::CANCEL).min_size(BUTTON)).clicked() {
                    cancel = true;
                }
            });
        });
    if cancel || r.close_requested {
        return Outcome::Close;
    }
    let keep = PullDialog::Review {
        event,
        body: body.clone(),
    };
    if submit {
        // Pending comments are cleared only once GitHub accepted the review.
        let cmd = Command::SubmitReview {
            slug: slug.clone(),
            number: d.summary.number,
            review: Review {
                commit_id: d.head_sha.clone(),
                event,
                body,
                comments: cx.state.pulls.pending.clone(),
            },
        };
        return Outcome::Send(cmd, keep);
    }
    Outcome::Keep(keep)
}

#[allow(clippy::too_many_arguments)]
fn merge(
    egui_ctx: &egui::Context,
    cx: &mut Ctx<'_>,
    slug: &Slug,
    mut method: MergeMethod,
    mut title: String,
    mut message: String,
    mut delete_branch: bool,
) -> Outcome {
    let Some(d) = cx.state.pulls.detail.clone() else {
        return Outcome::Close;
    };
    let busy = cx.state.pulls.busy;
    let mut confirm = false;
    let mut cancel = false;
    let r = Dialog::new("pull_merge", s::MERGE_TITLE)
        .width(480.0)
        .show(egui_ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(s::MERGE_METHOD);
                let mut picked = None;
                combo_box(ui, "merge_method", method_label(method), 220.0, |ui| {
                    for m in &d.allowed_methods {
                        if ui
                            .selectable_label(*m == method, method_label(*m))
                            .clicked()
                        {
                            picked = Some(*m);
                        }
                    }
                });
                if let Some(m) = picked.filter(|m| *m != method) {
                    method = m;
                    (title, message) = merge_defaults(&d, m);
                }
            });
            if method != MergeMethod::Rebase {
                ui.label(s::COMMIT_TITLE);
                text_field(ui, &mut title, 460.0, false);
                ui.label(s::COMMIT_MESSAGE);
                text_area(ui, &mut message, 460.0, 5);
            }
            if !d.cross_repository {
                checkbox(ui, &mut delete_branch, s::DELETE_BRANCH_AFTER);
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let ok = method == MergeMethod::Rebase || !title.trim().is_empty();
                if ui
                    .add(
                        Button95::new(s::CONFIRM_MERGE)
                            .min_size(BUTTON)
                            .enabled(ok && !busy),
                    )
                    .clicked()
                {
                    confirm = true;
                }
                if ui.add(Button95::new(s::CANCEL).min_size(BUTTON)).clicked() {
                    cancel = true;
                }
            });
        });
    if cancel || r.close_requested {
        return Outcome::Close;
    }
    let keep = PullDialog::Merge {
        method,
        title: title.clone(),
        message: message.clone(),
        delete_branch,
    };
    if confirm {
        let cmd = Command::MergePull {
            slug: slug.clone(),
            number: d.summary.number,
            merge: Merge {
                method,
                title: title.trim().to_string(),
                message,
                // The head the user saw: GitHub refuses if it moved since.
                sha: d.head_sha.clone(),
            },
            delete_branch: (delete_branch && !d.cross_repository).then(|| d.summary.head.clone()),
        };
        return Outcome::Send(cmd, keep);
    }
    Outcome::Keep(keep)
}

type CreateFields = (String, String, String, bool, Vec<String>, bool);

fn create(egui_ctx: &egui::Context, cx: &mut Ctx<'_>, slug: &Slug, f: CreateFields) -> Outcome {
    let (mut title, mut body, mut base, mut draft, mut labels, mut publish) = f;
    let Some(head) = super::sync_toolbar::current_branch(&cx.state.branches).cloned() else {
        return Outcome::Close;
    };
    let candidates = base_candidates(&cx.state.branches, &head.name);
    let repo_labels = cx
        .state
        .pulls
        .meta
        .as_ref()
        .map(|m| m.labels.clone())
        .unwrap_or_default();
    let busy = cx.state.pulls.busy;
    let mut ok = false;
    let mut cancel = false;
    let r = Dialog::new("pull_create", s::CREATE_PULL_TITLE)
        .width(520.0)
        .show(egui_ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(s::PULL_HEAD);
                ui.label(&head.name);
                ui.label(s::PULL_BASE);
                let mut picked = None;
                combo_box(ui, "pull_base", &base, 200.0, |ui| {
                    for b in &candidates {
                        if ui.selectable_label(*b == base, b).clicked() {
                            picked = Some(b.clone());
                        }
                    }
                });
                if let Some(b) = picked {
                    base = b;
                }
            });
            ui.label(s::PULL_TITLE);
            text_field(ui, &mut title, 500.0, false);
            ui.label(s::PULL_DESCRIPTION);
            text_area(ui, &mut body, 500.0, 6);
            if !repo_labels.is_empty() {
                ui.label(s::LABELS);
                ui.horizontal_wrapped(|ui| {
                    for l in &repo_labels {
                        let mut on = labels.contains(&l.name);
                        if checkbox(ui, &mut on, "").changed() {
                            if on {
                                labels.push(l.name.clone());
                            } else {
                                labels.retain(|n| *n != l.name);
                            }
                        }
                        win95::label_chip(ui, &l.name, l.color);
                        ui.add_space(6.0);
                    }
                });
            }
            checkbox(ui, &mut draft, s::CREATE_AS_DRAFT);
            if head.upstream.is_none() {
                checkbox(ui, &mut publish, s::PUBLISH_FIRST);
            }
            let same = base.trim().is_empty() || base == head.name;
            if same && !base.is_empty() {
                ui.label(
                    egui::RichText::new(s::SAME_BRANCH)
                        .color(win95::theme::palette(ui.ctx()).gray_text),
                );
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let ready = !title.trim().is_empty() && !same && !busy;
                if ui
                    .add(
                        Button95::new(s::CREATE_PULL)
                            .min_size(egui::vec2(140.0, 23.0))
                            .enabled(ready),
                    )
                    .clicked()
                {
                    ok = true;
                }
                if ui.add(Button95::new(s::CANCEL).min_size(BUTTON)).clicked() {
                    cancel = true;
                }
            });
        });
    if cancel || r.close_requested {
        return Outcome::Close;
    }
    let keep = PullDialog::Create {
        title: title.clone(),
        body: body.clone(),
        base: base.clone(),
        draft,
        labels: labels.clone(),
        publish,
    };
    if ok {
        // Closed by `PullCreated`.
        let cmd = Command::CreatePull {
            slug: slug.clone(),
            pull: NewPull {
                title: title.trim().to_string(),
                body,
                head: head.name.clone(),
                base,
                draft,
                labels,
            },
            publish: publish && head.upstream.is_none(),
        };
        return Outcome::Send(cmd, keep);
    }
    Outcome::Keep(keep)
}

fn edit_pull(
    egui_ctx: &egui::Context,
    cx: &mut Ctx<'_>,
    slug: &Slug,
    mut title: String,
    mut body: String,
) -> Outcome {
    let Some(number) = cx.state.pulls.selected else {
        return Outcome::Close;
    };
    let busy = cx.state.pulls.busy;
    let (mut save, mut cancel) = (false, false);
    let r = Dialog::new("pull_edit", s::EDIT_PULL_TITLE)
        .width(520.0)
        .show(egui_ctx, |ui| {
            ui.label(s::PULL_TITLE);
            text_field(ui, &mut title, 500.0, false);
            ui.label(s::PULL_DESCRIPTION);
            text_area(ui, &mut body, 500.0, 10);
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let ok = !title.trim().is_empty() && !busy;
                save = ui
                    .add(Button95::new(s::SAVE).min_size(BUTTON).enabled(ok))
                    .clicked();
                cancel = ui.add(Button95::new(s::CANCEL).min_size(BUTTON)).clicked();
            });
        });
    if cancel || r.close_requested {
        return Outcome::Close;
    }
    let keep = PullDialog::EditPull {
        title: title.clone(),
        body: body.clone(),
    };
    if save {
        let cmd = Command::UpdatePull {
            slug: slug.clone(),
            number,
            title: title.trim().to_string(),
            body,
        };
        return Outcome::Send(cmd, keep);
    }
    Outcome::Keep(keep)
}

/// People shown in the People dialog: those checked, then the candidates matching
/// `filter` (case-insensitive), without duplicates.
pub fn people_rows(checked: &[String], candidates: &[String], filter: &str) -> Vec<String> {
    let f = filter.trim().to_lowercase();
    let mut out: Vec<String> = checked.to_vec();
    for c in candidates {
        if (f.is_empty() || c.to_lowercase().contains(&f))
            && !out.iter().any(|o| o.eq_ignore_ascii_case(c))
        {
            out.push(c.clone());
        }
    }
    out
}

fn people(
    egui_ctx: &egui::Context,
    cx: &mut Ctx<'_>,
    slug: &Slug,
    kind: PeopleKind,
    mut checked: Vec<String>,
    mut filter: String,
) -> Outcome {
    let Some(d) = cx.state.pulls.detail.clone() else {
        return Outcome::Close;
    };
    let current: Vec<String> = match kind {
        PeopleKind::Reviewers => d
            .reviewers
            .iter()
            .filter(|r| r.state.is_none())
            .map(|r| r.login.clone())
            .collect(),
        PeopleKind::Assignees => d.assignees.clone(),
    };
    let rows = people_rows(&checked, &cx.state.pulls.assignable, &filter);
    let busy = cx.state.pulls.busy;
    let (mut ok, mut cancel) = (false, false);
    let r = Dialog::new(("pull_people", kind.title()), kind.title())
        .width(360.0)
        .show(egui_ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(s::FILTER);
                text_field(ui, &mut filter, 250.0, false);
            });
            egui::ScrollArea::vertical()
                .max_height(300.0)
                .show(ui, |ui| {
                    for login in &rows {
                        let mut on = checked.iter().any(|c| c.eq_ignore_ascii_case(login));
                        // The author cannot review their own pull request.
                        let own = kind == PeopleKind::Reviewers
                            && login.eq_ignore_ascii_case(&d.summary.author);
                        ui.add_enabled_ui(!own, |ui| {
                            if checkbox(ui, &mut on, &format!("@{login}")).changed() {
                                if on {
                                    checked.push(login.clone());
                                } else {
                                    checked.retain(|c| !c.eq_ignore_ascii_case(login));
                                }
                            }
                        });
                    }
                });
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ok = ui
                    .add(Button95::new(s::OK).min_size(BUTTON).enabled(!busy))
                    .clicked();
                cancel = ui.add(Button95::new(s::CANCEL).min_size(BUTTON)).clicked();
            });
        });
    if cancel || r.close_requested {
        return Outcome::Close;
    }
    // Ask GitHub for the people matching the filter (more than the first 100).
    if cx.state.pulls.assignable_query.as_deref() != Some(filter.trim()) {
        cx.state.pulls.assignable_query = Some(filter.trim().to_string());
        cx.worker.send(Command::LoadAssignable {
            slug: slug.clone(),
            query: filter.trim().to_string(),
        });
    }
    let keep = PullDialog::People {
        kind,
        checked: checked.clone(),
        filter,
    };
    if ok {
        let (add, remove) = github::diff_lists(&current, &checked);
        if add.is_empty() && remove.is_empty() {
            return Outcome::Close;
        }
        let cmd = Command::SetPeople {
            slug: slug.clone(),
            number: d.summary.number,
            kind,
            add,
            remove,
        };
        return Outcome::Send(cmd, keep);
    }
    Outcome::Keep(keep)
}

fn labels(
    egui_ctx: &egui::Context,
    cx: &mut Ctx<'_>,
    slug: &Slug,
    mut checked: Vec<String>,
) -> Outcome {
    let Some(d) = cx.state.pulls.detail.clone() else {
        return Outcome::Close;
    };
    let busy = cx.state.pulls.busy;
    let mut ok = false;
    let mut cancel = false;
    let r = Dialog::new("pull_labels", s::LABELS_TITLE)
        .width(360.0)
        .show(egui_ctx, |ui| {
            if d.repo_labels.is_empty() {
                ui.label(s::NO_LABELS_IN_REPO);
            }
            egui::ScrollArea::vertical()
                .max_height(300.0)
                .show(ui, |ui| {
                    for l in &d.repo_labels {
                        ui.horizontal(|ui| {
                            let mut on = checked.contains(&l.name);
                            if checkbox(ui, &mut on, "").changed() {
                                if on {
                                    checked.push(l.name.clone());
                                } else {
                                    checked.retain(|n| *n != l.name);
                                }
                            }
                            win95::label_chip(ui, &l.name, l.color);
                            if let Some(desc) = &l.description {
                                ui.label(
                                    egui::RichText::new(desc)
                                        .color(win95::theme::palette(ui.ctx()).gray_text),
                                );
                            }
                        });
                    }
                });
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui
                    .add(Button95::new(s::OK).min_size(BUTTON).enabled(!busy))
                    .clicked()
                {
                    ok = true;
                }
                if ui.add(Button95::new(s::CANCEL).min_size(BUTTON)).clicked() {
                    cancel = true;
                }
            });
        });
    if cancel || r.close_requested {
        return Outcome::Close;
    }
    if ok {
        let cmd = Command::SetLabels {
            slug: slug.clone(),
            number: d.summary.number,
            labels: checked.clone(),
        };
        return Outcome::Send(cmd, PullDialog::Labels { checked });
    }
    Outcome::Keep(PullDialog::Labels { checked })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn review_needs_text_except_approvals_and_line_comments() {
        assert!(review_ready(ReviewEvent::Approve, "", 0));
        assert!(!review_ready(ReviewEvent::Comment, " ", 0));
        assert!(review_ready(ReviewEvent::Comment, "", 2));
        assert!(!review_ready(ReviewEvent::RequestChanges, "", 3));
        assert!(review_ready(ReviewEvent::RequestChanges, "Fix it", 0));
    }

    #[test]
    fn base_branches_are_unique_and_exclude_the_head() {
        let b = |name: &str, remote: bool| gitcore::Branch {
            name: name.into(),
            remote,
            is_head: false,
            upstream: None,
            ahead: 0,
            behind: 0,
        };
        let branches = [
            b("main", false),
            b("feat/x", false),
            b("origin/main", true),
            b("origin/develop", true),
            b("origin/feat/x", true),
        ];
        assert_eq!(base_candidates(&branches, "feat/x"), ["develop", "main"]);
    }
}
