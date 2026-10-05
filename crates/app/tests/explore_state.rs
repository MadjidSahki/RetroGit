#![allow(clippy::unwrap_used)]
//! Explore state: what to load, tree rows, blame ages, late or foreign answers.

use std::path::PathBuf;

use gitcore::{BlameBlock, EntryKind, FileContent, Head, RepoSummary, TreeEntry};
use retrogit::config::Config;
use retrogit::protocol::{Event, ExploreRequest, ExploreResult};
use retrogit::state::{AppState, ExploreView, FileView, age_ranks, tree_rows};

fn entry(path: &str, kind: EntryKind) -> TreeEntry {
    TreeEntry {
        path: path.into(),
        kind,
        size: 1,
    }
}

fn entries() -> Vec<TreeEntry> {
    vec![
        entry("Cargo.toml", EntryKind::File),
        entry("src", EntryKind::Dir),
        entry("src/main.rs", EntryKind::File),
        entry("src/ui", EntryKind::Dir),
        entry("src/ui/view.rs", EntryKind::File),
        entry("README.md", EntryKind::File),
    ]
}

#[test]
fn tree_rows_show_open_folders_first_and_filter_by_name() {
    let mut open = std::collections::BTreeSet::new();
    let rows = |open: &std::collections::BTreeSet<String>, filter: &str| -> Vec<(String, usize)> {
        tree_rows(&entries(), open, filter)
            .into_iter()
            .map(|r| (r.name, r.depth))
            .collect()
    };
    assert_eq!(
        rows(&open, ""),
        [
            ("src".into(), 0),
            ("Cargo.toml".into(), 0),
            ("README.md".into(), 0)
        ]
    );
    open.insert("src".to_string());
    assert_eq!(
        rows(&open, ""),
        [
            ("src".into(), 0),
            ("ui".into(), 1),
            ("main.rs".into(), 1),
            ("Cargo.toml".into(), 0),
            ("README.md".into(), 0)
        ]
    );
    // A filter shows matching files with their folders, all open.
    assert_eq!(
        rows(&Default::default(), "VIEW"),
        [("src".into(), 0), ("ui".into(), 1), ("view.rs".into(), 2)]
    );
}

fn block(time: i64) -> BlameBlock {
    BlameBlock {
        commit: format!("{time}"),
        author: "a".into(),
        time,
        summary: String::new(),
        start: 1,
        count: 1,
        orig_path: "f".into(),
        orig_start: 1,
        boundary: false,
        lines: vec![],
        previous: Some(("p".into(), "f".into())),
    }
}

#[test]
fn blame_ages_rank_newest_first() {
    let ranks = age_ranks(&[block(100), block(300), block(200), block(300)]);
    assert_eq!(ranks, [0.0, 1.0, 0.5, 1.0]);
    assert_eq!(age_ranks(&[block(5)]), [1.0]);
}

fn opened() -> AppState {
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

fn loaded(st: &mut AppState, result: ExploreResult) {
    st.apply(Event::ExploreLoaded {
        repo: PathBuf::from("/tmp/r"),
        result,
    });
}

#[test]
fn explore_asks_for_what_it_shows_once() {
    let mut st = opened();
    let reqs = st.explore.needs();
    assert_eq!(
        reqs,
        [
            ExploreRequest::Refs,
            ExploreRequest::Tree { rev: "HEAD".into() }
        ]
    );
    assert!(st.explore.needs().is_empty(), "asked once");
    loaded(
        &mut st,
        ExploreResult::Tree {
            rev: "HEAD".into(),
            commit: "c1".into(),
            entries: entries(),
        },
    );
    assert_eq!(st.explore.commit.as_deref(), Some("c1"));
    st.explore.open_file("src/main.rs");
    assert_eq!(
        st.explore.needs(),
        [ExploreRequest::File {
            rev: "c1".into(),
            path: "src/main.rs".into()
        }],
        "file requests use the commit of the tree shown"
    );
    loaded(
        &mut st,
        ExploreResult::File {
            rev: "c1".into(),
            path: "src/main.rs".into(),
            content: FileContent::Text("x\n".into()),
        },
    );
    st.explore.view = FileView::Blame;
    assert_eq!(
        st.explore.needs(),
        [ExploreRequest::Blame {
            rev: "c1".into(),
            path: "src/main.rs".into()
        }]
    );
    st.explore.view = FileView::History;
    assert_eq!(
        st.explore.needs(),
        [ExploreRequest::FileHistory {
            rev: "c1".into(),
            path: "src/main.rs".into()
        }]
    );
    // Another version: everything again, the file kept if it is still there.
    st.explore.set_rev("v1");
    assert_eq!(
        st.explore.needs(),
        [ExploreRequest::Tree { rev: "v1".into() }]
    );
    loaded(
        &mut st,
        ExploreResult::Tree {
            rev: "v1".into(),
            commit: "c0".into(),
            entries: entries(),
        },
    );
    assert_eq!(
        st.explore.needs(),
        [ExploreRequest::FileHistory {
            rev: "c0".into(),
            path: "src/main.rs".into()
        }],
        "the file is still there: its history at the new version"
    );
}

fn with_tree(st: &mut AppState) {
    st.explore.needs();
    loaded(
        st,
        ExploreResult::Tree {
            rev: "HEAD".into(),
            commit: "HEAD".into(),
            entries: entries(),
        },
    );
}

#[test]
fn late_and_foreign_answers_are_ignored() {
    let mut st = opened();
    with_tree(&mut st);
    st.explore.open_file("a.rs");
    st.explore.needs();
    // For another file: dropped.
    loaded(
        &mut st,
        ExploreResult::File {
            rev: "HEAD".into(),
            path: "b.rs".into(),
            content: FileContent::Text("b".into()),
        },
    );
    assert_eq!(st.explore.content, None);
    // From another repository: dropped.
    st.apply(Event::ExploreLoaded {
        repo: PathBuf::from("/elsewhere"),
        result: ExploreResult::File {
            rev: "HEAD".into(),
            path: "a.rs".into(),
            content: FileContent::Text("x".into()),
        },
    });
    assert_eq!(st.explore.content, None);
    loaded(
        &mut st,
        ExploreResult::File {
            rev: "HEAD".into(),
            path: "a.rs".into(),
            content: FileContent::Text("a\n".into()),
        },
    );
    assert_eq!(st.explore.content, Some(FileContent::Text("a\n".into())));
    assert!(st.explore.code.is_some(), "text to color");
}

#[test]
fn a_version_that_disappeared_falls_back_to_head() {
    let mut st = opened();
    st.explore.set_rev("gone");
    st.explore.needs();
    loaded(
        &mut st,
        ExploreResult::Failed {
            request: ExploreRequest::Tree { rev: "gone".into() },
            message: "unknown revision".into(),
        },
    );
    assert_eq!(st.explore.rev, "HEAD");
    assert!(!st.messages.is_empty(), "the user is told");
    assert!(
        st.explore
            .needs()
            .contains(&ExploreRequest::Tree { rev: "HEAD".into() })
    );
}

#[test]
fn blame_the_parent_and_back() {
    let mut st = opened();
    with_tree(&mut st);
    st.explore.open_file("a.rs");
    st.explore.view = FileView::Blame;
    st.explore.needs();
    loaded(
        &mut st,
        ExploreResult::File {
            rev: "HEAD".into(),
            path: "a.rs".into(),
            content: FileContent::Text("x\n".into()),
        },
    );
    st.explore.needs();
    let b = BlameBlock {
        commit: "c2".into(),
        previous: Some(("c1".into(), "old.rs".into())),
        ..block(1)
    };
    st.explore.blame_parent(&b);
    assert_eq!(
        st.explore.needs(),
        [ExploreRequest::Blame {
            rev: "c1".into(),
            path: "old.rs".into()
        }],
        "the parent and the path git names (renames)"
    );
    assert!(st.explore.can_go_back());
    st.explore.blame_back();
    assert_eq!(
        st.explore.blame_target(),
        Some(("HEAD".to_string(), "a.rs".to_string()))
    );
}

#[test]
fn switching_repository_resets_explore() {
    let mut st = opened();
    st.explore.set_rev("v1");
    st.explore.open_file("a.rs");
    st.apply(Event::RepoOpened(RepoSummary {
        name: "s".into(),
        path: PathBuf::from("/tmp/s"),
        head: Head::Branch("main".into()),
        origin_url: None,
        last_commit: None,
    }));
    assert_eq!(st.explore, ExploreView::default());
}

#[test]
fn a_late_answer_of_another_file_search_is_ignored() {
    let mut st = opened();
    let grep = |text: &str, match_case: bool, paths: &str, n: usize| ExploreResult::Grep {
        rev: "HEAD".into(),
        text: text.into(),
        match_case,
        paths: paths.into(),
        result: gitcore::GrepResult {
            matches: (1..=n)
                .map(|line| gitcore::GrepMatch {
                    path: "a.rs".into(),
                    line,
                    text: "x".into(),
                })
                .collect(),
            truncated: false,
        },
    };
    st.explore.start_search("x", false, "");
    st.explore.start_search("x", true, "");
    loaded(&mut st, grep("x", false, "", 3));
    let sv = st.explore.search.as_ref().unwrap();
    assert!(
        sv.running,
        "the answer of the case-insensitive search is not this one"
    );
    assert!(sv.result.is_none());
    st.explore.start_search("x", true, "*.rs");
    loaded(&mut st, grep("x", true, "", 2));
    assert!(st.explore.search.as_ref().unwrap().running, "other paths");
    loaded(&mut st, grep("x", true, "*.rs", 1));
    let sv = st.explore.search.as_ref().unwrap();
    assert!(!sv.running);
    assert_eq!(sv.result.as_ref().unwrap().matches.len(), 1);
}

#[test]
fn search_results_and_the_history_filter() {
    let mut st = opened();
    st.explore.start_search("needle", false, "");
    assert!(st.explore.search.as_ref().unwrap().running);
    loaded(
        &mut st,
        ExploreResult::Grep {
            rev: "HEAD".into(),
            text: "needle".into(),
            match_case: false,
            paths: String::new(),
            result: gitcore::GrepResult {
                matches: vec![],
                truncated: false,
            },
        },
    );
    assert!(!st.explore.search.as_ref().unwrap().running);
    assert!(st.explore.search.as_ref().unwrap().result.is_some());
    // History filter.
    st.history.filter.query = "fix".into();
    let req = st.history.start_filter().unwrap();
    assert_eq!(
        req,
        ExploreRequest::LogSearch {
            kind: gitcore::LogSearch::Message,
            query: "fix".into()
        }
    );
    loaded(
        &mut st,
        ExploreResult::LogSearch {
            kind: gitcore::LogSearch::Message,
            query: "fix".into(),
            entries: vec![],
            truncated: false,
        },
    );
    assert!(!st.history.filter.running);
    assert!(st.history.filter.results.is_some());
    st.history.clear_filter();
    assert!(st.history.filter.results.is_none());
}

#[test]
fn a_commit_or_a_fetch_reloads_the_version_and_its_refs() {
    let mut st = opened();
    with_tree(&mut st);
    loaded(&mut st, ExploreResult::Refs(vec![]));
    st.explore.open_file("src/main.rs");
    st.explore.needs();
    loaded(
        &mut st,
        ExploreResult::File {
            rev: "HEAD".into(),
            path: "src/main.rs".into(),
            content: FileContent::Text("old\n".into()),
        },
    );
    // Same repository again (after a commit, checkout, pull): HEAD may have moved.
    st.apply(Event::RepoOpened(RepoSummary {
        name: "r".into(),
        path: PathBuf::from("/tmp/r"),
        head: Head::Branch("main".into()),
        origin_url: None,
        last_commit: None,
    }));
    let reqs = st.explore.needs();
    assert!(
        reqs.contains(&ExploreRequest::Refs)
            && reqs.contains(&ExploreRequest::Tree { rev: "HEAD".into() }),
        "{reqs:?}"
    );
    loaded(
        &mut st,
        ExploreResult::Tree {
            rev: "HEAD".into(),
            commit: "new".into(),
            entries: entries(),
        },
    );
    assert_eq!(st.explore.content, None, "the old content is dropped");
    assert_eq!(st.explore.file.as_deref(), Some("src/main.rs"));
    assert_eq!(
        st.explore.needs(),
        [ExploreRequest::File {
            rev: "new".into(),
            path: "src/main.rs".into()
        }]
    );
}

#[test]
fn the_ref_list_comes_back_after_a_version_disappeared() {
    let mut st = opened();
    st.explore.needs();
    loaded(&mut st, ExploreResult::Refs(vec![]));
    st.explore.set_rev("gone");
    st.explore.needs();
    loaded(
        &mut st,
        ExploreResult::Failed {
            request: ExploreRequest::Tree { rev: "gone".into() },
            message: "x".into(),
        },
    );
    assert!(st.explore.needs().contains(&ExploreRequest::Refs));
}

#[test]
fn editing_the_history_filter_while_it_runs_does_not_lose_the_answer() {
    let mut st = opened();
    st.history.filter.query = "foo".into();
    st.history.start_filter();
    st.history.filter.query = "foob".into();
    st.history.filter.kind = gitcore::LogSearch::Author;
    loaded(
        &mut st,
        ExploreResult::LogSearch {
            kind: gitcore::LogSearch::Message,
            query: "foo".into(),
            entries: vec![],
            truncated: false,
        },
    );
    assert!(!st.history.filter.running);
    assert!(st.history.filter.results.is_some());
}

#[test]
fn binary_files_are_not_blamed() {
    let mut st = opened();
    with_tree(&mut st);
    st.explore.view = FileView::Blame;
    st.explore.open_file("src/main.rs");
    assert_eq!(
        st.explore.needs(),
        [ExploreRequest::File {
            rev: "HEAD".into(),
            path: "src/main.rs".into()
        }],
        "content first"
    );
    loaded(
        &mut st,
        ExploreResult::File {
            rev: "HEAD".into(),
            path: "src/main.rs".into(),
            content: FileContent::Binary,
        },
    );
    assert!(st.explore.needs().is_empty(), "no blame of a binary file");
}

#[test]
fn tree_rows_are_built_once_until_something_changes() {
    let mut st = opened();
    with_tree(&mut st);
    let first = st.explore.rows().len();
    st.explore.entries.push(entry("zzz.txt", EntryKind::File));
    assert_eq!(st.explore.rows().len(), first, "cached");
    st.explore.toggle_dir("src");
    assert_eq!(
        st.explore.rows().len(),
        first + 3,
        "open folder and the new file"
    );
}

#[test]
fn a_failure_for_another_file_does_not_touch_the_open_one() {
    let mut st = opened();
    with_tree(&mut st);
    st.explore.view = FileView::Blame;
    st.explore.open_file("README.md");
    st.explore.needs();
    loaded(
        &mut st,
        ExploreResult::File {
            rev: "HEAD".into(),
            path: "README.md".into(),
            content: FileContent::Text("r\n".into()),
        },
    );
    st.explore.needs();
    loaded(
        &mut st,
        ExploreResult::Failed {
            request: ExploreRequest::Blame {
                rev: "HEAD".into(),
                path: "src/main.rs".into(),
            },
            message: "late".into(),
        },
    );
    assert_eq!(st.explore.blame, None);
    assert_eq!(st.explore.load_error, None);
    loaded(
        &mut st,
        ExploreResult::Failed {
            request: ExploreRequest::Blame {
                rev: "HEAD".into(),
                path: "README.md".into(),
            },
            message: "bad".into(),
        },
    );
    assert_eq!(
        st.explore.load_error.as_deref(),
        Some("bad"),
        "shown in place of Loading..."
    );
}

#[test]
fn file_history_diffs_have_their_own_colors() {
    let mut st = opened();
    let d = retrogit::state::text_as_diff("a.rs", "x\n");
    st.explore.history_diff = Some(d.clone());
    st.explore.code = Some(d.clone());
    st.apply(Event::ColorsLoaded {
        target: retrogit::highlight::Target::ExploreHistory,
        diff: d,
        colors: None,
        dark: false,
    });
    assert_eq!(
        st.explore.history_colors,
        retrogit::highlight::Colors::Plain
    );
    assert_eq!(st.explore.colors, retrogit::highlight::Colors::NotRequested);
}
