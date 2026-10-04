//! Several GitHub accounts: the Accounts window, the account of a repository, and what
//! the status bar says.

use win95::{Button95, Dialog};

use super::Ctx;
use crate::protocol::Command;
use crate::state::AppState;
use crate::strings as s;

/// Status bar text about accounts: the open GitHub repository's account, else how many
/// accounts are signed in.
pub fn who_text(st: &AppState) -> String {
    if st.current.is_some() {
        if st.github_slug().is_none() {
            return s::GIT_CREDENTIALS.to_string();
        }
        return match &st.repo_account {
            None => s::CHECKING_ACCOUNT.to_string(),
            Some(Some(login)) => format!("{} @{login}", s::ACCOUNT_LABEL),
            Some(None) => s::GIT_CREDENTIALS.to_string(),
        };
    }
    match st.accounts.len() {
        0 => s::NOT_SIGNED_IN.to_string(),
        1 => format!("{} @{}", s::ACCOUNT_LABEL, st.accounts[0].login),
        n => format!("{n} {}", s::ACCOUNTS_COUNT),
    }
}

/// URL to clone: github.com SSH URLs become HTTPS (RetroGit clones over HTTPS with the
/// account's token); other SSH URLs are refused, as cloning has no SSH support.
pub fn clone_url_for(url: &str) -> Result<String, &'static str> {
    let url = url.trim();
    if let Some((owner, repo)) = gitcore::parse_github_slug(url) {
        return Ok(format!("https://github.com/{owner}/{repo}.git"));
    }
    if url.starts_with("https://") || url.starts_with("http://") {
        return Ok(url.to_string());
    }
    if url.starts_with("ssh://") || url.contains('@') && url.contains(':') {
        return Err(s::ERR_SSH_CLONE);
    }
    Err(s::ERR_CLONE_URL)
}

/// Folder name for cloning `url`: its last path segment without `.git`.
pub fn folder_from_url(url: &str) -> Option<String> {
    let url = url.trim().trim_end_matches('/');
    let path = url.rsplit_once("://").map_or(url, |(_, rest)| rest);
    let last = path.rsplit(['/', ':']).next()?;
    let name = last.trim_end_matches(".git");
    let host_only = !path.contains(['/', ':']);
    (!name.is_empty() && !host_only).then(|| name.to_string())
}

pub fn show(egui_ctx: &egui::Context, cx: &mut Ctx<'_>) {
    accounts_window(egui_ctx, cx);
    repo_account_window(egui_ctx, cx);
}

fn accounts_window(egui_ctx: &egui::Context, cx: &mut Ctx<'_>) {
    if !cx.state.accounts_dialog || cx.state.sign_in.is_some() {
        return;
    }
    let mut remove = None;
    let mut add = false;
    let mut close = false;
    let r = Dialog::new("accounts", s::ACCOUNTS_TITLE)
        .width(440.0)
        .show(egui_ctx, |ui| {
            if cx.state.accounts.is_empty() {
                ui.label(s::NO_ACCOUNTS);
            }
            egui::Grid::new("accounts_grid")
                .num_columns(3)
                .show(ui, |ui| {
                    for a in &cx.state.accounts {
                        ui.label(format!("@{}", a.login));
                        if a.valid {
                            ui.label(a.name.clone().unwrap_or_default());
                        } else {
                            ui.label(
                                egui::RichText::new(s::SIGN_IN_AGAIN)
                                    .color(win95::theme::palette(ui.ctx()).error),
                            );
                        }
                        if ui.add(Button95::new(s::REMOVE)).clicked() {
                            remove = Some(a.login.clone());
                        }
                        ui.end_row();
                    }
                });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                add = ui
                    .add(Button95::new(s::ADD_ACCOUNT).min_size(egui::vec2(120.0, 23.0)))
                    .clicked();
                close = ui
                    .add(Button95::new(s::CLOSE).min_size(egui::vec2(90.0, 23.0)))
                    .clicked();
            });
        });
    if let Some(login) = remove {
        cx.worker.send(Command::RemoveAccount(login));
    }
    if add {
        // The sign-in window adds the account; this one comes back when it closes.
        cx.state.sign_in = Some(Default::default());
    }
    if close || r.close_requested {
        cx.state.accounts_dialog = false;
    }
}

fn repo_account_window(egui_ctx: &egui::Context, cx: &mut Ctx<'_>) {
    if !cx.state.repo_account_dialog {
        return;
    }
    let Some(slug) = cx.state.github_slug() else {
        cx.state.repo_account_dialog = false;
        return;
    };
    let key = format!("{}/{}", slug.0, slug.1).to_lowercase();
    let manual = cx
        .state
        .config
        .repo_accounts
        .get(&key)
        .filter(|r| r.manual)
        .map(|r| r.login.clone());
    let mut pick: Option<Option<String>> = None;
    let mut close = false;
    let title = format!("{} {}/{}", s::REPO_ACCOUNT_TITLE, slug.0, slug.1);
    let r = Dialog::new("repo_account", &title)
        .width(380.0)
        .show(egui_ctx, |ui| {
            ui.label(s::REPO_ACCOUNT_HELP);
            ui.add_space(4.0);
            if ui
                .selectable_label(manual.is_none(), s::AUTOMATIC)
                .clicked()
            {
                pick = Some(None);
            }
            for a in cx.state.accounts.iter().filter(|a| a.valid) {
                let on = manual
                    .as_deref()
                    .is_some_and(|m| m.eq_ignore_ascii_case(&a.login));
                if ui.selectable_label(on, format!("@{}", a.login)).clicked() {
                    pick = Some(Some(a.login.clone()));
                }
            }
            ui.add_space(8.0);
            close = ui
                .add(Button95::new(s::CLOSE).min_size(egui::vec2(90.0, 23.0)))
                .clicked();
        });
    if let Some(login) = pick {
        cx.state.repo_account = None;
        cx.state.pulls.stale = true;
        cx.worker.send(Command::SetRepoAccount {
            slug: slug.clone(),
            login,
        });
        if let Some(number) = cx.state.pulls.reload_selected() {
            cx.worker.send(Command::LoadPull { slug, number });
        }
    }
    if close || r.close_requested {
        cx.state.repo_account_dialog = false;
    }
}
