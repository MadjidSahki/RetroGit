//! The "Changes" screen: file lists, diff, commit form.

use egui::{Panel, RichText};
use gitcore::{Change, FileStatus, Head, Selection, Side};
use win95::{
    Bevel, Button95, Cell, Column, ListView, ProgressBar95, bevel_frame, checkbox, text_area,
    text_field,
};

use super::{Ctx, diff_view};
use crate::protocol::Command;
use crate::strings as s;

pub fn show(ui: &mut egui::Ui, cx: &mut Ctx<'_>) {
    Panel::top("changes_header")
        .frame(egui::Frame::NONE)
        .show(ui, |ui| {
            header(ui, cx);
            operation_banner(ui, cx);
        });
    Panel::bottom("commit_box")
        .frame(egui::Frame::NONE.inner_margin(egui::Margin::same(2)))
        .show(ui, |ui| commit_box(ui, cx));
    Panel::left("changed_files")
        .frame(egui::Frame::NONE.inner_margin(egui::Margin::same(2)))
        .resizable(true)
        .default_size(260.0)
        .show(ui, |ui| file_lists(ui, cx));
    egui::CentralPanel::default()
        .frame(egui::Frame::NONE.inner_margin(egui::Margin::same(2)))
        .show(ui, |ui| {
            if cx.state.changes.conflict_path.is_some() {
                super::conflict_view::show(ui, cx)
            } else {
                diff_view::show(ui, cx)
            }
        });
    toggle_with_space(ui, cx);
}

fn header(ui: &mut egui::Ui, cx: &mut Ctx<'_>) {
    let Some(c) = &cx.state.current else { return };
    let branch = match &c.head {
        Head::Branch(b) => b.clone(),
        Head::Unborn(b) => format!("{b} {}", s::NO_COMMITS),
        Head::Detached(id) => format!("{id} {}", s::DETACHED),
    };
    let last = c
        .last_commit
        .as_ref()
        .map(|lc| format!(" · {} \"{}\"", lc.short_id, lc.summary))
        .unwrap_or_default();
    ui.horizontal(|ui| {
        ui.label(RichText::new(format!("{} · {branch}{last}", c.name)).color(win95::theme::NAVY));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.add(Button95::new(s::REFRESH)).clicked() {
                cx.worker.send(Command::RefreshStatus);
            }
        });
    });
    ui.add_space(2.0);
}

/// Yellow banner while a merge or rebase waits for conflict resolution.
fn operation_banner(ui: &mut egui::Ui, cx: &mut Ctx<'_>) {
    let Some(op) = cx.state.operation else { return };
    let (text, abort) = match op {
        gitcore::Operation::Merge => (s::MERGE_IN_PROGRESS, s::ABORT_MERGE),
        gitcore::Operation::Rebase => (s::REBASE_IN_PROGRESS, s::ABORT_REBASE),
        gitcore::Operation::CherryPick => (s::CHERRY_PICK_IN_PROGRESS, s::ABORT_CHERRY_PICK),
        gitcore::Operation::Revert => (s::REVERT_IN_PROGRESS, s::ABORT_REVERT),
    };
    egui::Frame::NONE
        .fill(egui::Color32::from_rgb(0xFF, 0xFF, 0xC0))
        .inner_margin(egui::Margin::same(4))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(text).color(win95::theme::BLACK));
                if ui
                    .add(Button95::new(abort).min_size(egui::vec2(100.0, 20.0)))
                    .clicked()
                    && cx.state.changes.request_abort()
                {
                    cx.worker.send(Command::AbortOperation);
                }
                if op != gitcore::Operation::Merge {
                    let label = if op == gitcore::Operation::Rebase {
                        s::CONTINUE_REBASE
                    } else {
                        s::CONTINUE
                    };
                    if ui
                        .add(Button95::new(label).min_size(egui::vec2(110.0, 20.0)))
                        .clicked()
                    {
                        cx.worker.send(Command::ContinueOperation);
                    }
                    if ui
                        .add(Button95::new(s::SKIP).min_size(egui::vec2(70.0, 20.0)))
                        .clicked()
                    {
                        cx.state.git_dialog = Some(crate::state::GitDialog::ConfirmSkip);
                    }
                }
            });
        });
    ui.add_space(2.0);
}

/// "Signed with GPG key ABCD" / "Commits will NOT be signed" for the commit form.
pub fn signing_label(cfg: Option<&gitcore::SigningConfig>) -> (String, egui::Color32) {
    match cfg {
        Some(c) if c.enabled => {
            let kind = match c.format {
                gitcore::SigningFormat::Gpg => "GPG",
                gitcore::SigningFormat::Ssh => "SSH",
                gitcore::SigningFormat::X509 => "X.509",
            };
            let key = c
                .key
                .as_deref()
                .map(|k| format!(" key {k}"))
                .unwrap_or_default();
            (
                format!("{} {kind}{key}", s::SIGNED_WITH),
                egui::Color32::from_rgb(0, 0x60, 0),
            )
        }
        _ => (
            s::NOT_SIGNED.to_string(),
            egui::Color32::from_rgb(0xA0, 0, 0),
        ),
    }
}

/// `[M]`, `[A]`, ... and the displayed path (`old -> new` for renames).
pub fn describe(path: &str, change: &Change) -> String {
    let (code, shown) = match change {
        Change::Added => ("A", path.to_string()),
        Change::Modified => ("M", path.to_string()),
        Change::Deleted => ("D", path.to_string()),
        Change::Renamed { from } => ("R", format!("{from} -> {path}")),
        Change::TypeChange => ("T", path.to_string()),
        Change::Untracked => ("?", path.to_string()),
        Change::Conflicted => ("!", path.to_string()),
    };
    format!("[{code}] {shown}")
}

/// Paths a whole-file stage/unstage must touch (both sides of a rename).
pub fn paths_of(file: &FileStatus, side: Side) -> Vec<String> {
    let change = match side {
        Side::Staged => &file.staged,
        Side::Unstaged => &file.unstaged,
    };
    match change {
        Some(Change::Renamed { from }) => vec![file.path.clone(), from.clone()],
        _ => vec![file.path.clone()],
    }
}

/// `*.ext` pattern for a path, if it has an extension.
pub fn extension_pattern(path: &str) -> Option<String> {
    let name = path.rsplit('/').next()?;
    let (stem, ext) = name.rsplit_once('.')?;
    (!stem.is_empty() && !ext.is_empty()).then(|| format!("*.{ext}"))
}

fn toggle_file(cx: &Ctx<'_>, file: &FileStatus, side: Side) {
    if file.unstaged == Some(Change::Conflicted) {
        return;
    }
    for path in paths_of(file, side) {
        let cmd = match side {
            Side::Unstaged => Command::Stage {
                path,
                selection: Selection::All,
                shown: None,
            },
            Side::Staged => Command::Unstage {
                path,
                selection: Selection::All,
                shown: None,
            },
        };
        cx.worker.send(cmd);
    }
}

/// One batch command for Stage all / Unstage all (conflicts are skipped, renames include
/// both paths).
pub fn all_files_command(files: &[FileStatus], side: Side) -> Command {
    let paths: Vec<String> = files
        .iter()
        .filter(|f| f.unstaged != Some(Change::Conflicted))
        .flat_map(|f| paths_of(f, side))
        .collect();
    match side {
        Side::Unstaged => Command::StageFiles(paths),
        Side::Staged => Command::UnstageFiles(paths),
    }
}

/// Unstaged files whose changes can be thrown away (not conflicts).
fn discardable(files: &[FileStatus]) -> Vec<FileStatus> {
    files
        .iter()
        .filter(|f| f.unstaged != Some(Change::Conflicted))
        .cloned()
        .collect()
}

fn request_discard_files(cx: &mut Ctx<'_>, files: &[FileStatus]) {
    let question = super::discard::files_question(files);
    let paths = files.iter().map(|f| f.path.clone()).collect();
    cx.state
        .changes
        .request_discard(Command::DiscardFiles(paths), question);
}

fn select_file(cx: &mut Ctx<'_>, path: &str, side: Side) {
    let c = &mut cx.state.changes;
    if c.conflict_path.is_some() {
        c.close_conflict();
        if c.conflict_path.is_some() {
            return; // edited: the user is asked first
        }
    }
    if c.shown.as_ref() != Some(&(path.to_string(), side)) {
        c.shown = Some((path.to_string(), side));
        c.diff = None;
        c.selected_lines.clear();
    }
    cx.worker.send(Command::LoadDiff {
        path: path.to_string(),
        side,
    });
}

const FILE_COLUMNS: &[Column] = &[Column {
    title: "File",
    width: 1000.0,
}];

fn file_lists(ui: &mut egui::Ui, cx: &mut Ctx<'_>) {
    let staged: Vec<FileStatus> = cx.state.changes.staged().cloned().collect();
    let conflicted: Vec<FileStatus> = cx.state.changes.conflicted().cloned().collect();
    let unstaged: Vec<FileStatus> = cx
        .state
        .changes
        .unstaged()
        .filter(|f| f.unstaged != Some(Change::Conflicted))
        .cloned()
        .collect();
    if !conflicted.is_empty() {
        conflicts_group(ui, cx, &conflicted);
        ui.add_space(4.0);
    }
    let half = ((ui.available_height() - 60.0) / 2.0).max(60.0);
    group(ui, cx, &staged, Side::Staged, half);
    ui.add_space(4.0);
    group(ui, cx, &unstaged, Side::Unstaged, half);
}

/// Files to resolve, above the staged and unstaged lists; a click opens the editor.
fn conflicts_group(ui: &mut egui::Ui, cx: &mut Ctx<'_>, files: &[FileStatus]) {
    ui.label(
        RichText::new(format!("{} ({})", s::CONFLICTS, files.len()))
            .color(egui::Color32::from_rgb(0xA0, 0, 0)),
    );
    let selected = cx
        .state
        .changes
        .conflict_path
        .as_ref()
        .and_then(|p| files.iter().position(|f| &f.path == p));
    let height = (files.len() as f32 * 18.0 + 6.0).min(120.0);
    let resp = ListView::new("conflicted_files", FILE_COLUMNS, files.len())
        .header(false)
        .height(height)
        .show(ui, selected, |row, _| {
            Cell::from(format!("! {}", files[row].path))
        });
    if let Some(row) = resp.clicked.or(resp.double_clicked) {
        let path = files[row].path.clone();
        if cx.state.changes.open_conflict(&path) {
            cx.worker.send(Command::LoadConflict(path));
        }
    }
}

fn group(ui: &mut egui::Ui, cx: &mut Ctx<'_>, files: &[FileStatus], side: Side, height: f32) {
    let (title, all_label) = match side {
        Side::Staged => (s::STAGED_CHANGES, s::UNSTAGE_ALL),
        Side::Unstaged => (s::CHANGES, s::STAGE_ALL),
    };
    ui.horizontal(|ui| {
        ui.label(format!("{title} ({})", files.len()));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let b = Button95::new(all_label)
                .min_size(egui::vec2(80.0, 20.0))
                .enabled(!files.is_empty());
            if ui.add(b).clicked() {
                cx.worker.send(all_files_command(files, side));
            }
            if side == Side::Unstaged {
                let discardable = discardable(files);
                let b = Button95::new(s::DISCARD_ALL)
                    .min_size(egui::vec2(80.0, 20.0))
                    .enabled(!discardable.is_empty());
                if ui.add(b).clicked() {
                    request_discard_files(cx, &discardable);
                }
            }
        });
    });
    let selected = cx
        .state
        .changes
        .shown
        .as_ref()
        .filter(|(_, s)| *s == side)
        .and_then(|(p, _)| files.iter().position(|f| &f.path == p));
    let mut menu_action: Option<(usize, MenuAction)> = None;
    let id = match side {
        Side::Staged => "staged_files",
        Side::Unstaged => "unstaged_files",
    };
    let resp = ListView::new(id, FILE_COLUMNS, files.len())
        .header(false)
        .height(height)
        .context_menu(|row, ui| {
            let toggle = if side == Side::Staged {
                s::UNSTAGE
            } else {
                s::STAGE
            };
            if ui.button(toggle).clicked() {
                menu_action = Some((row, MenuAction::Toggle));
            }
            if side == Side::Unstaged {
                if files[row].unstaged != Some(Change::Conflicted)
                    && ui.button(s::DISCARD_MENU).clicked()
                {
                    menu_action = Some((row, MenuAction::Discard));
                }
                if ui.button(s::IGNORE_FILE).clicked() {
                    menu_action = Some((row, MenuAction::IgnorePath));
                }
                if let Some(pattern) = extension_pattern(&files[row].path) {
                    let label = s::IGNORE_EXT.replace("{ext}", pattern.trim_start_matches("*."));
                    if ui.button(label).clicked() {
                        menu_action = Some((row, MenuAction::Ignore(pattern)));
                    }
                }
            }
        })
        .show(ui, selected, |row, _| {
            let f = &files[row];
            let change = match side {
                Side::Staged => f.staged.as_ref(),
                Side::Unstaged => f.unstaged.as_ref(),
            };
            Cell::from(change.map(|c| describe(&f.path, c)).unwrap_or_default())
        });
    if let Some(row) = resp.double_clicked {
        toggle_file(cx, &files[row], side);
    } else if let Some(row) = resp.clicked {
        select_file(cx, &files[row].path, side);
    }
    match menu_action {
        Some((row, MenuAction::Toggle)) => toggle_file(cx, &files[row], side),
        Some((row, MenuAction::IgnorePath)) => cx
            .worker
            .send(Command::AddToGitignore(files[row].path.clone())),
        Some((_, MenuAction::Ignore(pattern))) => cx.worker.send(Command::AddToGitignore(pattern)),
        Some((row, MenuAction::Discard)) => {
            request_discard_files(cx, std::slice::from_ref(&files[row]))
        }
        None => {}
    }
}

enum MenuAction {
    Toggle,
    Discard,
    IgnorePath,
    Ignore(String),
}

/// Space toggles the displayed file when no text field has the keyboard.
fn toggle_with_space(ui: &egui::Ui, cx: &mut Ctx<'_>) {
    if ui.ctx().egui_wants_keyboard_input() || !ui.input(|i| i.key_pressed(egui::Key::Space)) {
        return;
    }
    let Some((path, side)) = cx.state.changes.shown.clone() else {
        return;
    };
    if let Some(file) = cx
        .state
        .changes
        .files
        .iter()
        .find(|f| f.path == path)
        .cloned()
    {
        toggle_file(cx, &file, side);
    }
}

fn commit_box(ui: &mut egui::Ui, cx: &mut Ctx<'_>) {
    let committing = cx.state.changes.committing;
    let signing = cx.state.signing.clone();
    let can_commit = cx.state.changes.can_commit();
    let focus = std::mem::take(&mut cx.state.changes.focus_summary);
    let c = &mut cx.state.changes;
    let mut amend_toggled = false;
    let mut commit_clicked = false;
    bevel_frame(ui, Bevel::Sunken, win95::theme::SILVER, 4, |ui| {
        ui.set_width(ui.available_width());
        ui.add_enabled_ui(!committing, |ui| {
            ui.horizontal(|ui| {
                ui.label(s::SUMMARY);
                let r = text_field(
                    ui,
                    &mut c.summary,
                    (ui.available_width() - 190.0).max(120.0),
                    false,
                );
                if focus {
                    r.request_focus();
                }
                let before = c.amend;
                checkbox(ui, &mut c.amend, s::AMEND);
                amend_toggled = c.amend && !before;
            });
            if c.amend && c.head_pushed {
                ui.label(
                    RichText::new(s::AMEND_PUSHED_WARNING)
                        .color(egui::Color32::from_rgb(0x80, 0, 0)),
                );
            }
            ui.horizontal(|ui| {
                ui.label(s::DESCRIPTION);
                text_area(
                    ui,
                    &mut c.description,
                    (ui.available_width() - 100.0).max(120.0),
                    3,
                );
                ui.vertical(|ui| {
                    commit_clicked = ui
                        .add(Button95::new(s::COMMIT).enabled(can_commit))
                        .clicked();
                });
            });
            ui.horizontal(|ui| {
                let (text, color) = signing_label(signing.as_ref());
                ui.label(egui::RichText::new(text).color(color));
            });
        });
        if committing {
            ui.horizontal(|ui| {
                ui.label(s::COMMITTING);
                ui.add(ProgressBar95::new(None).width(200.0));
            });
        }
    });
    if amend_toggled {
        cx.worker.send(Command::LoadAmendInfo);
    }
    if commit_clicked && can_commit {
        let c = &mut cx.state.changes;
        c.committing = true;
        let message = c.commit_message();
        let amend = c.amend;
        cx.worker.send(Command::Commit { message, amend });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn describes_changes_with_win95_style_codes() {
        assert_eq!(describe("a.rs", &Change::Modified), "[M] a.rs");
        assert_eq!(
            describe(
                "new.rs",
                &Change::Renamed {
                    from: "old.rs".into()
                }
            ),
            "[R] old.rs -> new.rs"
        );
        assert_eq!(describe("x", &Change::Untracked), "[?] x");
    }

    #[test]
    fn renames_touch_both_paths() {
        let f = FileStatus {
            path: "new.rs".into(),
            staged: Some(Change::Renamed {
                from: "old.rs".into(),
            }),
            unstaged: None,
        };
        assert_eq!(
            paths_of(&f, Side::Staged),
            vec!["new.rs".to_string(), "old.rs".to_string()]
        );
        assert_eq!(paths_of(&f, Side::Unstaged), vec!["new.rs".to_string()]);
    }

    #[test]
    fn signing_label_says_whether_commits_are_signed() {
        let cfg = gitcore::SigningConfig {
            enabled: true,
            format: gitcore::SigningFormat::Gpg,
            key: Some("ABCD1234".into()),
        };
        assert_eq!(signing_label(Some(&cfg)).0, "Signed with GPG key ABCD1234");
        let off = gitcore::SigningConfig {
            enabled: false,
            ..cfg
        };
        assert_eq!(signing_label(Some(&off)).0, "Commits will NOT be signed");
        assert_eq!(signing_label(None).0, "Commits will NOT be signed");
    }

    #[test]
    fn extension_patterns() {
        assert_eq!(extension_pattern("logs/app.log").as_deref(), Some("*.log"));
        assert_eq!(extension_pattern("Makefile"), None);
        assert_eq!(extension_pattern(".env"), None);
        assert_eq!(extension_pattern("dir.d/file"), None);
    }
}
