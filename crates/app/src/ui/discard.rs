//! Confirmation box for discarding working-tree changes.

use gitcore::{Change, FileStatus};
use win95::{Button95, Dialog, Icon};

use super::Ctx;
use crate::strings as s;

pub fn files_question(files: &[FileStatus]) -> String {
    let mut q = match files {
        [one] => format!("Discard changes to {}?", one.path),
        many => format!("Discard changes to {} files?", many.len()),
    };
    if files.iter().any(|f| f.unstaged == Some(Change::Untracked)) {
        q.push_str("\n\n");
        q.push_str(s::DISCARD_UNTRACKED_NOTE);
    }
    q
}

/// Question for "Reset all": every changed file, staged or not, goes back to HEAD.
pub fn reset_question(files: &[FileStatus]) -> String {
    let plural = if files.len() == 1 { "" } else { "s" };
    let mut q = format!(
        "Throw away all staged and unstaged changes ({} file{plural})?",
        files.len()
    );
    // The status cannot tell every such file (gitcore's `reset_trashes` decides), so the
    // note names the rule rather than promising which files go.
    let new_file = |f: &FileStatus| {
        f.unstaged == Some(Change::Untracked)
            || matches!(f.staged, Some(Change::Added | Change::Renamed { .. }))
    };
    if files.iter().any(new_file) {
        q.push_str("\n\n");
        q.push_str(s::RESET_ALL_NEW_NOTE);
    }
    q
}

pub fn lines_question(path: &str, n: usize) -> String {
    let plural = if n == 1 { "" } else { "s" };
    format!("Discard {n} selected line{plural} in {path}?")
}

pub fn hunk_question(path: &str) -> String {
    format!("Discard this hunk of {path}?")
}

/// Show the pending confirmation, if any; send the command on [Discard].
pub fn show(egui_ctx: &egui::Context, cx: &mut Ctx<'_>) {
    let Some(pending) = cx.state.changes.pending_discard.clone() else {
        return;
    };
    let mut choice = None;
    let r = Dialog::new("discard", s::DISCARD_TITLE)
        .width(380.0)
        .show(egui_ctx, |ui| {
            ui.horizontal(|ui| {
                win95::icon::icon(ui, Icon::Warning);
                ui.vertical(|ui| {
                    ui.add(egui::Label::new(&pending.question).wrap());
                    ui.label(s::DISCARD_WARNING);
                });
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.add(Button95::new(s::DISCARD)).clicked() {
                    choice = Some(true);
                }
                if ui.add(Button95::new(s::CANCEL)).clicked() {
                    choice = Some(false);
                }
            });
        });
    match (choice, r.close_requested) {
        (Some(true), _) => {
            if let Some(cmd) = cx.state.changes.confirm_discard() {
                cx.worker.send(cmd);
            }
        }
        (Some(false), _) | (None, true) => cx.state.changes.cancel_discard(),
        (None, false) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gitcore::Change;

    fn f(path: &str, unstaged: Change) -> FileStatus {
        FileStatus {
            path: path.into(),
            staged: None,
            unstaged: Some(unstaged),
        }
    }

    #[test]
    fn questions_name_what_will_be_lost() {
        assert_eq!(
            files_question(&[f("src/lib.rs", Change::Modified)]),
            "Discard changes to src/lib.rs?"
        );
        let two = [f("a", Change::Modified), f("b", Change::Untracked)];
        assert_eq!(
            files_question(&two),
            "Discard changes to 2 files?\n\nUntracked files are moved to the trash."
        );
        assert_eq!(
            lines_question("src/lib.rs", 1),
            "Discard 1 selected line in src/lib.rs?"
        );
        assert_eq!(
            lines_question("src/lib.rs", 3),
            "Discard 3 selected lines in src/lib.rs?"
        );
        assert_eq!(
            hunk_question("src/lib.rs"),
            "Discard this hunk of src/lib.rs?"
        );
    }

    #[test]
    fn the_reset_question_counts_staged_files_and_warns_about_new_ones() {
        let staged = FileStatus {
            path: "a".into(),
            staged: Some(Change::Modified),
            unstaged: None,
        };
        assert_eq!(
            reset_question(std::slice::from_ref(&staged)),
            "Throw away all staged and unstaged changes (1 file)?"
        );
        let added = FileStatus {
            path: "b".into(),
            staged: Some(Change::Added),
            unstaged: None,
        };
        assert_eq!(
            reset_question(&[staged, added]),
            "Throw away all staged and unstaged changes (2 files)?\n\nFiles that are not in the last commit are moved to the trash."
        );
        assert!(reset_question(&[f("c", Change::Untracked)]).ends_with(s::RESET_ALL_NEW_NOTE));
    }
}
