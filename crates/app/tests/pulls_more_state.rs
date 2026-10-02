#![allow(clippy::unwrap_used)]

use github::DiffSide;
use retrogit::pr_diff::parse_patch;
use retrogit::state::{
    LineSelection, PeopleKind, apply_disabled_reason, extend_selection, selection_target,
};
use retrogit::strings as s;

const PATCH: &str = "@@ -1,4 +1,5 @@\n a\n-b\n+B\n+C\n c\n d\n@@ -20 +21 @@\n-x\n+y";

#[test]
fn a_selection_stays_in_its_hunk_and_on_its_side() {
    let d = parse_patch("a.rs", Some(PATCH));
    let one = LineSelection {
        hunk: 0,
        from: 2,
        to: 2,
    };
    // Shift+click further down in the same hunk.
    assert_eq!(
        extend_selection(one, 0, 4),
        LineSelection {
            hunk: 0,
            from: 2,
            to: 4
        }
    );
    // Upwards too.
    assert_eq!(
        extend_selection(one, 0, 0),
        LineSelection {
            hunk: 0,
            from: 0,
            to: 2
        }
    );
    // Another hunk: unchanged.
    assert_eq!(extend_selection(one, 1, 1), one);
    // New side: B (2), C (3), c (4) -> lines 2..4, removed "b" ignored.
    let t = selection_target(
        &d,
        LineSelection {
            hunk: 0,
            from: 1,
            to: 4,
        },
    )
    .unwrap();
    assert_eq!(
        (t.start, t.end, t.side),
        (2, 4, DiffSide::Right),
        "first line b is old side: starts at B"
    );
    let t = selection_target(
        &d,
        LineSelection {
            hunk: 0,
            from: 2,
            to: 4,
        },
    )
    .unwrap();
    assert_eq!((t.start, t.end, t.side), (2, 4, DiffSide::Right));
    assert_eq!(t.lines, ["B", "C", "c"]);
    // Old side alone.
    let t = selection_target(
        &d,
        LineSelection {
            hunk: 0,
            from: 1,
            to: 1,
        },
    )
    .unwrap();
    assert_eq!((t.start, t.end, t.side), (2, 2, DiffSide::Left));
}

#[test]
fn suggestions_can_be_applied_only_on_the_checked_out_pull_request() {
    let reason = |branch: Option<&str>, outdated, side| {
        apply_disabled_reason("feat/x", 7, branch, outdated, side)
    };
    assert_eq!(reason(Some("feat/x"), false, DiffSide::Right), None);
    assert_eq!(
        reason(Some("pr/7"), false, DiffSide::Right),
        None,
        "fork checkout"
    );
    assert_eq!(
        reason(Some("main"), false, DiffSide::Right),
        Some(s::WHY_CHECKOUT_FIRST)
    );
    assert_eq!(
        reason(None, false, DiffSide::Right),
        Some(s::WHY_CHECKOUT_FIRST)
    );
    assert_eq!(
        reason(Some("feat/x"), true, DiffSide::Right),
        Some(s::WHY_OUTDATED_SUGGESTION)
    );
    assert_eq!(
        reason(Some("feat/x"), false, DiffSide::Left),
        Some(s::WHY_OUTDATED_SUGGESTION)
    );
}

#[test]
fn people_kinds_have_titles() {
    assert_eq!(PeopleKind::Reviewers.title(), s::REVIEWERS_TITLE);
    assert_eq!(PeopleKind::Assignees.title(), s::ASSIGNEES_TITLE);
}

#[test]
fn the_people_dialog_lists_checked_people_then_matching_candidates() {
    use retrogit::ui::pull_dialogs::people_rows;
    let rows = people_rows(
        &["ada".to_string()],
        &[
            "Ada".to_string(),
            "bob".to_string(),
            "carol".to_string(),
            "eve".to_string(),
        ],
        "o",
    );
    assert_eq!(
        rows,
        ["ada", "bob", "carol"],
        "checked first, no duplicate, filtered"
    );
}

#[test]
fn a_suggestion_prefill_keeps_blank_lines() {
    let d = parse_patch("a.rs", Some("@@ -1,2 +1,3 @@\n a\n+foo\n+\n b"));
    let t = selection_target(
        &d,
        LineSelection {
            hunk: 0,
            from: 1,
            to: 2,
        },
    )
    .unwrap();
    assert_eq!(
        retrogit::state::suggestion_prefill(&t),
        "```suggestion\nfoo\n\n```\n"
    );
}

#[test]
fn suggestions_of_a_forks_pull_request_cannot_be_pushed_from_here() {
    assert_eq!(
        retrogit::state::apply_disabled_reason_for(
            true,
            "x",
            7,
            Some("pr/7"),
            false,
            DiffSide::Right
        ),
        Some(s::WHY_FORK_SUGGESTION)
    );
    assert_eq!(
        retrogit::state::apply_disabled_reason_for(
            false,
            "x",
            7,
            Some("x"),
            false,
            DiffSide::Right
        ),
        None
    );
}

#[test]
fn opening_another_file_clears_the_selection() {
    use retrogit::protocol::Event;
    let mut st = retrogit::state::AppState::new(retrogit::config::Config::default());
    st.pulls.slug = Some(("o".into(), "r".into()));
    st.pulls.select(7);
    let file = |p: &str| github::PrFile {
        path: p.into(),
        previous_path: None,
        status: "modified".into(),
        additions: 1,
        deletions: 0,
        patch: Some("@@ -1 +1 @@\n-a\n+b".into()),
    };
    st.apply(Event::PullFilesLoaded {
        slug: ("o".into(), "r".into()),
        number: 7,
        files: vec![file("a.rs"), file("b.rs")],
    });
    st.pulls.open_file("a.rs");
    st.pulls.selection = Some(LineSelection {
        hunk: 0,
        from: 0,
        to: 1,
    });
    st.pulls.open_file("b.rs");
    assert_eq!(st.pulls.selection, None);
    st.pulls.selection = Some(LineSelection {
        hunk: 0,
        from: 0,
        to: 1,
    });
    st.apply(Event::PullFilesLoaded {
        slug: ("o".into(), "r".into()),
        number: 7,
        files: vec![file("b.rs")],
    });
    assert_eq!(st.pulls.selection, None, "files reloaded");
}
