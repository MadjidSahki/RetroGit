#![allow(clippy::unwrap_used)]

use std::path::PathBuf;

use gitcore::{Head, OpOutcome, RepoSummary, StashEntry, TodoAction, TodoItem};
use retrogit::config::Config;
use retrogit::protocol::Event;
use retrogit::state::{AppState, GitDialog, Tab, move_item};
use retrogit::strings as s;

fn state() -> AppState {
    let mut st = AppState::new(Config::default());
    st.apply(Event::RepoOpened(RepoSummary {
        name: "r".into(),
        path: PathBuf::from("/tmp/r"),
        head: Head::Branch("main".into()),
        origin_url: None,
        last_commit: None,
    }));
    st
}

fn item(id: &str) -> TodoItem {
    TodoItem {
        action: TodoAction::Pick,
        id: id.into(),
        summary: id.into(),
    }
}

#[test]
fn a_finished_operation_notes_it_and_conflicts_lead_to_changes() {
    let mut st = state();
    st.tab = Tab::History;
    st.apply(Event::OpFinished {
        outcome: OpOutcome::Done,
        note: s::NOTE_CHERRY_PICKED.into(),
    });
    assert_eq!(st.sync.note.as_deref(), Some(s::NOTE_CHERRY_PICKED));
    assert_eq!(st.tab, Tab::History);
    st.apply(Event::OpFinished {
        outcome: OpOutcome::Conflicts,
        note: s::NOTE_CHERRY_PICKED.into(),
    });
    assert_eq!(st.tab, Tab::Changes);
    assert_eq!(st.messages.back().unwrap().message, s::INFO_CONFLICTS);
}

#[test]
fn the_rebase_list_opens_the_rebase_dialog() {
    let mut st = state();
    st.apply(Event::RebaseListLoaded {
        base: "b".into(),
        items: vec![item("1"), item("2"), item("3")],
        pushed: 1,
    });
    let Some(GitDialog::Rebase { items, pushed, .. }) = &mut st.git_dialog else {
        panic!("{:?}", st.git_dialog)
    };
    assert_eq!(*pushed, 1);
    move_item(items, 2, true);
    let ids: Vec<&str> = items.iter().map(|i| i.id.as_str()).collect();
    assert_eq!(ids, ["1", "3", "2"]);
    move_item(items, 0, true);
    assert_eq!(items[0].id, "1", "the first cannot go up");
    move_item(items, 2, false);
    assert_eq!(items[2].id, "2", "the last cannot go down");
}

#[test]
fn reset_info_fills_the_open_reset_dialog() {
    let mut st = state();
    st.git_dialog = Some(GitDialog::Reset {
        id: "abc".into(),
        mode: gitcore::ResetMode::Mixed,
        hard_confirmed: false,
        drops_pushed: false,
    });
    st.apply(Event::ResetInfo {
        id: "abc".into(),
        drops_pushed: true,
    });
    assert!(matches!(
        st.git_dialog,
        Some(GitDialog::Reset {
            drops_pushed: true,
            ..
        })
    ));
    st.apply(Event::ResetInfo {
        id: "other".into(),
        drops_pushed: false,
    });
    assert!(
        matches!(
            st.git_dialog,
            Some(GitDialog::Reset {
                drops_pushed: true,
                ..
            })
        ),
        "for another commit: ignored"
    );
}

#[test]
fn stashes_and_tags_are_kept_and_the_selection_follows() {
    let mut st = state();
    let entry = |i: usize, m: &str| StashEntry {
        index: i,
        id: format!("id{i}"),
        message: m.into(),
        branch: "main".into(),
        time: 0,
    };
    st.apply(Event::StashesLoaded(vec![entry(0, "b"), entry(1, "a")]));
    st.stashes.select(1);
    st.apply(Event::StashFilesLoaded {
        index: 1,
        files: vec![gitcore::ChangedFile {
            path: "x".into(),
            change: gitcore::Change::Modified,
        }],
    });
    assert_eq!(st.stashes.files.len(), 1);
    st.apply(Event::StashFilesLoaded {
        index: 0,
        files: vec![],
    });
    assert_eq!(st.stashes.files.len(), 1, "for another stash: ignored");
    // The list changed (a drop): the selection is reset.
    st.apply(Event::StashesLoaded(vec![entry(0, "b")]));
    assert_eq!(st.stashes.selected, None);
    st.apply(Event::TagsLoaded(vec![gitcore::Tag {
        name: "v1".into(),
        commit: "c".into(),
        annotated: false,
        message: String::new(),
    }]));
    assert_eq!(st.tags.len(), 1);
}

fn log_entry(parents: usize) -> gitcore::LogEntry {
    gitcore::LogEntry {
        id: "c0ffee".into(),
        short_id: "c0ffee".into(),
        parents: (0..parents).map(|i| format!("p{i}")).collect(),
        author: "a".into(),
        email: "a@b".into(),
        time: 0,
        summary: "s".into(),
        refs: Vec::new(),
    }
}

#[test]
fn history_menu_actions_open_dialogs_or_send_commands() {
    use retrogit::protocol::Command;
    use retrogit::state::{GitDialog, HistoryAction};
    let mut st = retrogit::state::AppState::new(retrogit::config::Config::default());
    let plain = log_entry(1);
    assert!(matches!(
        st.history_action(HistoryAction::CherryPick, &plain),
        Some(Command::CherryPick(id)) if id == "c0ffee"
    ));
    assert!(matches!(
        st.history_action(HistoryAction::Revert, &plain),
        Some(Command::Revert { mainline: None, .. })
    ));
    assert!(st.git_dialog.is_none());
    assert!(
        st.history_action(HistoryAction::Revert, &log_entry(2))
            .is_none()
    );
    assert!(matches!(
        st.git_dialog,
        Some(GitDialog::RevertMerge { parent: 1, .. })
    ));
    assert!(matches!(
        st.history_action(HistoryAction::Reset, &plain),
        Some(Command::LoadResetInfo(_))
    ));
    assert!(matches!(
        st.git_dialog,
        Some(GitDialog::Reset {
            mode: gitcore::ResetMode::Mixed,
            hard_confirmed: false,
            ..
        })
    ));
    assert!(matches!(
        st.history_action(HistoryAction::RebaseFrom, &plain),
        Some(Command::LoadRebaseList(Some(id))) if id == "c0ffee"
    ));
    assert!(
        st.history_action(HistoryAction::CreateTag, &plain)
            .is_none()
    );
    assert!(matches!(
        st.git_dialog,
        Some(GitDialog::CreateTag {
            annotated: true,
            ..
        })
    ));
}
