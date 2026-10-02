#![allow(clippy::unwrap_used)]

use std::path::PathBuf;

use gitcore::{
    Change, Choice, ConflictFile, ConflictKind, FileStatus, Head, Operation, RepoSummary,
};
use retrogit::config::Config;
use retrogit::protocol::Event;
use retrogit::state::{AppState, ConflictConfirm};
use retrogit::strings as s;

const MARKED: &str = "a\n<<<<<<< HEAD\nmine\n=======\ntheirs\n>>>>>>> x\nb\n<<<<<<< HEAD\nm2\n=======\nt2\n>>>>>>> x\n";

fn file(path: &str, working: &str) -> ConflictFile {
    ConflictFile {
        path: path.into(),
        kind: ConflictKind::Content,
        mine: Some("a\nmine\nb\nm2\n".into()),
        theirs: Some("a\ntheirs\nb\nt2\n".into()),
        working: Some(working.into()),
        operation: Some(Operation::Merge),
    }
}

fn conflicted(paths: &[&str]) -> Event {
    Event::StatusLoaded(
        paths
            .iter()
            .map(|p| FileStatus {
                path: p.to_string(),
                staged: None,
                unstaged: Some(Change::Conflicted),
            })
            .collect(),
    )
}

fn state() -> AppState {
    let mut st = AppState::new(Config::default());
    st.apply(Event::RepoOpened(RepoSummary {
        name: "r".into(),
        path: PathBuf::from("/tmp/r"),
        head: Head::Branch("main".into()),
        origin_url: None,
        last_commit: None,
    }));
    st.apply(Event::OperationChanged(Some(Operation::Merge)));
    st.apply(conflicted(&["a.rs", "b.rs"]));
    st
}

fn open(st: &mut AppState, path: &str) {
    assert!(st.changes.open_conflict(path));
    st.apply(Event::ConflictLoaded(Box::new(file(path, MARKED))));
}

#[test]
fn opening_a_conflicted_file_starts_from_the_marked_text() {
    let mut st = state();
    st.changes.shown = Some(("x".into(), gitcore::Side::Unstaged));
    open(&mut st, "a.rs");
    assert!(
        st.changes.shown.is_none(),
        "the diff gives way to the editor"
    );
    let ed = st.changes.conflict.as_ref().unwrap();
    assert_eq!(ed.result, MARKED);
    assert_eq!(ed.conflicts_left(), 2);
    // A file loaded without being asked for is ignored.
    st.apply(Event::ConflictLoaded(Box::new(file("b.rs", MARKED))));
    assert_eq!(st.changes.conflict.as_ref().unwrap().file.path, "a.rs");
}

#[test]
fn choices_apply_to_the_current_block_and_move_on() {
    let mut st = state();
    open(&mut st, "a.rs");
    let ed = st.changes.conflict.as_mut().unwrap();
    ed.next();
    assert_eq!(ed.current, 1);
    ed.next();
    assert_eq!(ed.current, 1, "stays on the last block");
    ed.previous();
    ed.choose(Choice::Theirs);
    assert!(
        ed.result.starts_with("a\ntheirs\nb\n<<<<<<<"),
        "{}",
        ed.result
    );
    assert_eq!(ed.current, 0, "the next block is now number 0");
    ed.choose(Choice::Both);
    assert_eq!(ed.result, "a\ntheirs\nb\nm2\nt2\n");
    assert_eq!(ed.conflicts_left(), 0);
    assert!(ed.edited);
}

#[test]
fn edits_are_never_overwritten_by_a_reload() {
    let mut st = state();
    open(&mut st, "a.rs");
    st.changes.conflict.as_mut().unwrap().edit("typed\n".into());
    // Background refresh, file unchanged on disk: nothing happens.
    st.apply(Event::ConflictLoaded(Box::new(file("a.rs", MARKED))));
    let ed = st.changes.conflict.as_ref().unwrap();
    assert_eq!(ed.result, "typed\n");
    assert!(ed.on_disk.is_none());
    // Changed on disk (by an IDE): offered, not applied.
    st.apply(Event::ConflictLoaded(Box::new(file(
        "a.rs",
        "from the IDE\n",
    ))));
    let ed = st.changes.conflict.as_mut().unwrap();
    assert_eq!(ed.result, "typed\n");
    assert!(ed.on_disk.is_some());
    ed.reload();
    assert_eq!(ed.result, "from the IDE\n");
    assert!(!ed.edited);
}

#[test]
fn an_unedited_editor_follows_the_disk() {
    let mut st = state();
    open(&mut st, "a.rs");
    st.apply(Event::ConflictLoaded(Box::new(file(
        "a.rs",
        "new\n<<<<<<< a\nx\n=======\ny\n>>>>>>> b\n",
    ))));
    assert!(
        st.changes
            .conflict
            .as_ref()
            .unwrap()
            .result
            .starts_with("new\n")
    );
}

#[test]
fn leaving_an_edited_file_asks_first() {
    let mut st = state();
    open(&mut st, "a.rs");
    st.changes.conflict.as_mut().unwrap().edit("typed\n".into());
    assert!(!st.changes.open_conflict("b.rs"), "asks");
    assert_eq!(
        st.changes.conflict.as_ref().unwrap().confirm,
        Some(ConflictConfirm::Discard(Some("b.rs".into())))
    );
    assert_eq!(st.changes.discard_conflict_edits().as_deref(), Some("b.rs"));
    assert!(st.changes.conflict.is_none());
    assert_eq!(st.changes.conflict_path.as_deref(), Some("b.rs"));
    st.apply(Event::ConflictLoaded(Box::new(file("b.rs", MARKED))));
    st.changes.close_conflict();
    assert!(st.changes.conflict.is_none(), "unedited: closes at once");
}

#[test]
fn resolving_moves_to_the_next_file_then_says_what_is_left_to_do() {
    let mut st = state();
    open(&mut st, "a.rs");
    st.apply(conflicted(&["b.rs"]));
    assert!(
        st.changes.conflict.is_none(),
        "a.rs is no longer conflicted"
    );
    st.apply(Event::ConflictResolved("a.rs".into()));
    assert_eq!(st.changes.conflict_path.as_deref(), Some("b.rs"));
    assert!(st.changes.load_conflict);
    st.changes.load_conflict = false;
    st.apply(Event::ConflictLoaded(Box::new(file("b.rs", MARKED))));
    st.apply(conflicted(&[]));
    st.apply(Event::ConflictResolved("b.rs".into()));
    assert_eq!(st.messages.back().unwrap().message, s::ALL_RESOLVED_MERGE);
}

#[test]
fn syntax_colors_arrive_in_the_background_for_each_pane() {
    use retrogit::highlight::{Colors, Span, Target};
    let mut st = state();
    open(&mut st, "a.rs");
    let ed = st.changes.conflict.as_ref().unwrap();
    assert_eq!(ed.mine_colors, Colors::NotRequested);
    let mine = retrogit::state::text_as_diff("a.rs", ed.file.mine.as_deref().unwrap());
    let result = retrogit::state::text_as_diff("a.rs", &ed.result);
    assert_eq!(mine.line_count(), 4);
    let spans = |n: usize| {
        Some(vec![Some(
            (0..n)
                .map(|_| {
                    vec![Span {
                        text: "x".into(),
                        color: egui::Color32::RED,
                    }]
                })
                .collect(),
        )])
    };
    st.apply(Event::ColorsLoaded {
        target: Target::ConflictMine,
        diff: mine.clone(),
        colors: spans(4),
        dark: false,
    });
    st.apply(Event::ColorsLoaded {
        target: Target::ConflictResult,
        diff: result.clone(),
        colors: spans(result.line_count()),
        dark: false,
    });
    let ed = st.changes.conflict.as_mut().unwrap();
    assert!(matches!(ed.mine_colors, Colors::Ready(_)));
    assert!(matches!(ed.result_colors, Colors::Ready(_)));
    // Typing makes the result's colors stale; old results are dropped.
    ed.edit("typed\n".into());
    assert_eq!(ed.result_colors, Colors::NotRequested);
    st.apply(Event::ColorsLoaded {
        target: Target::ConflictResult,
        diff: result,
        colors: spans(13),
        dark: false,
    });
    assert_eq!(
        st.changes.conflict.as_ref().unwrap().result_colors,
        Colors::NotRequested,
        "colors of an older text are ignored"
    );
}

#[test]
fn opening_another_repository_with_edits_asks_first() {
    let mut st = state();
    open(&mut st, "a.rs");
    st.changes.conflict.as_mut().unwrap().edit("typed\n".into());
    let other = PathBuf::from("/tmp/other");
    assert!(!st.changes.request_open_repo(&other), "asks");
    assert_eq!(
        st.changes.conflict.as_ref().unwrap().confirm,
        Some(ConflictConfirm::OpenRepo(other.clone()))
    );
    st.changes.conflict.as_mut().unwrap().confirm = None;
    st.changes.conflict.as_mut().unwrap().edited = false;
    assert!(
        st.changes.request_open_repo(&other),
        "nothing typed: at once"
    );
}

#[test]
fn big_files_are_not_sent_to_the_highlighter() {
    let mut st = state();
    open(&mut st, "a.rs");
    let ed = st.changes.conflict.as_mut().unwrap();
    assert_eq!(ed.colors_to_request().len(), 3, "the three panes");
    assert!(ed.colors_to_request().is_empty(), "asked once");
    ed.edit("x\n".repeat(retrogit::highlight::MAX_LINES + 1));
    assert!(ed.colors_to_request().is_empty(), "too large: plain");
    assert_eq!(ed.result_colors, retrogit::highlight::Colors::Plain);
}
