//! Toolbar button and list of pull request notifications.

use win95::{Button95, Dialog};

use super::Ctx;
use crate::protocol::Command;
use crate::state::{NotificationTarget, split_repo};
use crate::strings as s;

/// "Notifications (3)" while some are unread.
pub fn button_label(unread: usize) -> String {
    if unread == 0 {
        s::NOTIFICATIONS.to_string()
    } else {
        format!("{} ({unread})", s::NOTIFICATIONS)
    }
}

pub fn toolbar_button(ui: &mut egui::Ui, cx: &mut Ctx<'_>) {
    let label = button_label(cx.state.notifications.unread);
    if ui
        .add(Button95::new(label).min_size(egui::vec2(110.0, 22.0)))
        .clicked()
    {
        cx.state.open_notifications();
    }
}

pub fn show(egui_ctx: &egui::Context, cx: &mut Ctx<'_>) {
    if !cx.state.notifications.open {
        return;
    }
    let mut picked = None;
    let mut close = false;
    let mut clear = false;
    let r = Dialog::new("notifications", s::NOTIFICATIONS_TITLE)
        .width(520.0)
        .show(egui_ctx, |ui| {
            let items = &cx.state.notifications.items;
            if items.is_empty() {
                ui.label(s::NO_NOTIFICATIONS);
            }
            egui::ScrollArea::vertical()
                .max_height(360.0)
                .show(ui, |ui| {
                    for (i, e) in items.iter().enumerate() {
                        let (title, body) =
                            crate::notify::notification_text(e, cx.state.accounts.len() > 1);
                        if ui
                            .selectable_label(false, format!("{title}  {body}"))
                            .clicked()
                        {
                            picked = Some(i);
                        }
                    }
                });
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui
                    .add(Button95::new(s::CLOSE).min_size(egui::vec2(90.0, 23.0)))
                    .clicked()
                {
                    close = true;
                }
                if ui
                    .add(Button95::new(s::CLEAR).min_size(egui::vec2(90.0, 23.0)))
                    .clicked()
                {
                    clear = true;
                }
            });
        });
    if clear {
        cx.state.notifications.items.clear();
    }
    if close || r.close_requested {
        cx.state.notifications.open = false;
    }
    let Some(e) = picked.and_then(|i| cx.state.notifications.items.get(i).cloned()) else {
        return;
    };
    cx.state.notifications.open = false;
    let target = cx.state.notification_target(&e, slug_of);
    open_target(egui_ctx, cx, &e.repo, target);
}

fn slug_of(path: &std::path::Path) -> Option<crate::protocol::Slug> {
    gitcore::Repo::open(path).ok().and_then(|r| r.github_slug())
}

fn open_target(egui_ctx: &egui::Context, cx: &mut Ctx<'_>, repo: &str, target: NotificationTarget) {
    match target {
        NotificationTarget::Current(number) => cx.state.show_pull(number),
        NotificationTarget::Local(path, number) => {
            cx.state.pulls.open_after_switch = split_repo(repo).map(|slug| (slug, number));
            if let Some(cmd) = cx.state.request_repo_switch(Command::OpenRepo(path)) {
                cx.worker.send(cmd);
            }
        }
        NotificationTarget::Browser(url) => egui_ctx.open_url(egui::OpenUrl::new_tab(url)),
    }
}

/// Open the pull requests of clicked notifications (links left by the system).
pub fn open_links(egui_ctx: &egui::Context, cx: &mut Ctx<'_>) {
    for link in cx.state.take_links() {
        match cx.state.link_target(&link, slug_of) {
            Some((repo, target)) => open_target(egui_ctx, cx, &repo, target),
            None => log::warn!("ignored notification link (not a valid pull request link): {link}"),
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_button_counts_unread_notifications() {
        assert_eq!(super::button_label(0), "Notifications");
        assert_eq!(super::button_label(3), "Notifications (3)");
    }
}
