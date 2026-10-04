#![allow(clippy::unwrap_used)]
//! The Explore tab, the search results and the History filter, drawn headless.

use std::path::PathBuf;
use std::sync::Arc;

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use gitcore::{
    BlameBlock, EntryKind, FileContent, GrepMatch, GrepResult, Head, LogEntry, RepoSummary,
    TreeEntry,
};
use github::{Client, MemoryAccounts, TokenProvider};
use retrogit::config::Config;
use retrogit::protocol::{Event, ExploreResult};
use retrogit::state::{AppState, FileView, HistoryAction, Tab};
use retrogit::strings as s;
use retrogit::ui::Ctx;
use retrogit::worker::{WorkerDeps, WorkerHandle, spawn};

struct World {
    state: AppState,
    worker: WorkerHandle,
    highlighter: retrogit::highlight::Service,
    notices: std::sync::mpsc::Sender<retrogit::protocol::AppError>,
}

fn loaded(st: &mut AppState, result: ExploreResult) {
    st.apply(Event::ExploreLoaded {
        repo: PathBuf::from("/tmp/r"),
        result,
    });
}

fn world() -> World {
    let server = mockito::Server::new();
    let worker = spawn(
        WorkerDeps {
            client: Client::with_bases(&server.url(), &server.url()),
            store: Arc::new(MemoryAccounts::default()),
            client_id: String::new(),
            commit_backend: gitcore::CommitBackend::Git2,
            tokens: TokenProvider::without_gh(),
            known_accounts: Vec::new(),
            repo_accounts: Default::default(),
        },
        || {},
    );
    let mut state = AppState::new(Config::default());
    state.apply(Event::RepoOpened(RepoSummary {
        name: "r".into(),
        path: PathBuf::from("/tmp/r"),
        head: Head::Branch("main".into()),
        origin_url: None,
        last_commit: None,
    }));
    state.tab = Tab::Explore;
    let e = |path: &str, kind| TreeEntry {
        path: path.into(),
        kind,
        size: 10,
    };
    loaded(
        &mut state,
        ExploreResult::Refs(vec![gitcore::ExploreRef {
            name: "HEAD".into(),
            kind: gitcore::RefKind::Head,
        }]),
    );
    loaded(
        &mut state,
        ExploreResult::Tree {
            rev: "HEAD".into(),
            commit: "c0ffee1234".into(),
            entries: vec![
                e("src", EntryKind::Dir),
                e("src/main.rs", EntryKind::File),
                e("README.md", EntryKind::File),
            ],
        },
    );
    let (notices, _rx) = std::sync::mpsc::channel();
    World {
        state,
        worker,
        highlighter: retrogit::highlight::Service::start(|_| {}),
        notices,
    }
}

fn harness(w: World) -> Harness<'static, World> {
    Harness::builder()
        .with_size(egui::vec2(1100.0, 700.0))
        .build_ui_state(
            |ui, w: &mut World| {
                let ctx = ui.ctx().clone();
                let mut cx = Ctx {
                    state: &mut w.state,
                    worker: &w.worker,
                    highlighter: &w.highlighter,
                    notices: &w.notices,
                };
                retrogit::ui::main_window::show(ui, &mut cx);
                retrogit::ui::explore::search_dialog(&ctx, &mut cx);
            },
            w,
        )
}

#[test]
fn the_tree_opens_folders_and_files() {
    let mut h = harness(world());
    h.run();
    assert!(h.query_by_label(s::TAB_EXPLORE).is_some());
    assert!(
        h.query_by_label("c0ffee1").is_some(),
        "commit of the version"
    );
    assert!(h.query_by_label("main.rs").is_none(), "folder closed");
    h.get_by_label("src").click();
    h.run();
    h.get_by_label("main.rs").click();
    h.run();
    assert_eq!(h.state().state.explore.file.as_deref(), Some("src/main.rs"));
    loaded(
        &mut h.state_mut().state,
        ExploreResult::File {
            rev: "c0ffee1234".into(),
            path: "src/main.rs".into(),
            content: FileContent::Text("fn main() {}\nlet x = 1;\n".into()),
        },
    );
    h.run();
    assert!(h.query_by_label_contains("let x = 1;").is_some());
    assert!(h.query_by_label(s::FILE_BLAME).is_some());
    h.get_by_label(s::FILE_HISTORY).click();
    h.run();
    assert_eq!(h.state().state.explore.view, FileView::History);
}

#[test]
fn binary_and_large_files_say_so() {
    let mut w = world();
    w.state.explore.open_file("README.md");
    loaded(
        &mut w.state,
        ExploreResult::File {
            rev: "c0ffee1234".into(),
            path: "README.md".into(),
            content: FileContent::TooLarge(8 * 1024 * 1024),
        },
    );
    let mut h = harness(w);
    h.run();
    assert!(
        h.query_by_label_contains("File too large (8.0 MB)")
            .is_some()
    );
}

#[test]
fn blame_shows_who_and_offers_the_parent() {
    let mut w = world();
    w.state.explore.open_file("src/main.rs");
    w.state.explore.view = FileView::Blame;
    loaded(
        &mut w.state,
        ExploreResult::File {
            rev: "c0ffee1234".into(),
            path: "src/main.rs".into(),
            content: FileContent::Text("one\ntwo\nthree\n".into()),
        },
    );
    let block = |commit: &str, author: &str, start, lines: &[&str]| BlameBlock {
        commit: commit.repeat(40),
        author: author.into(),
        time: 1_700_000_000,
        summary: format!("by {author}"),
        start,
        count: lines.len(),
        orig_path: "src/main.rs".into(),
        orig_start: start,
        boundary: false,
        lines: lines.iter().map(|l| l.to_string()).collect(),
        previous: Some(("p".repeat(40), "src/main.rs".into())),
    };
    loaded(
        &mut w.state,
        ExploreResult::Blame {
            rev: "c0ffee1234".into(),
            path: "src/main.rs".into(),
            blocks: vec![
                block("a", "Ada", 1, &["one", "two"]),
                block("b", "Bob", 3, &["three"]),
            ],
        },
    );
    let mut h = harness(w);
    h.run();
    assert!(h.query_by_label_contains("Ada").is_some());
    assert!(h.query_by_label_contains("Bob").is_some());
    assert!(h.query_by_label_contains("three").is_some());
    assert!(
        h.query_by_label(s::BLAME_BACK).is_none(),
        "nothing to go back to"
    );
    let b = h.state().state.explore.blame.as_ref().unwrap()[1].clone();
    h.state_mut().state.explore.blame_parent(&b);
    h.run();
    assert!(h.query_by_label(s::BLAME_BACK).is_some());
}

#[test]
fn search_results_open_the_file_at_the_line() {
    let mut w = world();
    w.state.explore.start_search("needle", false, "");
    loaded(
        &mut w.state,
        ExploreResult::Grep {
            rev: "HEAD".into(),
            text: "needle".into(),
            result: GrepResult {
                matches: vec![GrepMatch {
                    path: "src/main.rs".into(),
                    line: 12,
                    text: "let needle = 1;".into(),
                }],
                truncated: true,
            },
        },
    );
    let mut h = harness(w);
    h.run();
    assert!(
        h.query_by_label_contains("Showing the first 1000 matches")
            .is_some()
    );
    h.get_by_label_contains("let needle = 1;").click();
    h.run();
    let e = &h.state().state.explore;
    assert_eq!(
        (e.file.as_deref(), e.goto_line),
        (Some("src/main.rs"), Some(12))
    );
    h.get_by_label(s::BACK_TO_FILES).click();
    h.run();
    assert!(h.state().state.explore.search.is_none());
}

#[test]
fn the_search_window_starts_a_search() {
    let mut w = world();
    w.state.explore.search_form = Some(Default::default());
    let mut h = harness(w);
    h.run();
    assert!(h.query_by_label(s::MATCH_CASE).is_some());
    h.state_mut()
        .state
        .explore
        .search_form
        .as_mut()
        .unwrap()
        .text = "needle".into();
    h.run();
    h.get_by_label(s::SEARCH).click();
    h.run();
    let e = &h.state().state.explore;
    assert!(e.search_form.is_none());
    assert!(e.search.as_ref().unwrap().running);
}

fn entry(id: &str, summary: &str) -> LogEntry {
    LogEntry {
        id: id.repeat(40),
        short_id: id.repeat(7),
        parents: vec![],
        author: "Ada".into(),
        email: "a@b".into(),
        time: 0,
        summary: summary.into(),
        refs: vec![],
    }
}

#[test]
fn the_history_filter_shows_what_it_found() {
    let mut w = world();
    w.state.tab = Tab::History;
    w.state.apply(Event::LogLoaded {
        skip: 0,
        entries: vec![entry("1", "one"), entry("2", "two")],
    });
    w.state.history.filter.query = "fix".into();
    w.state.history.start_filter();
    loaded(
        &mut w.state,
        ExploreResult::LogSearch {
            kind: gitcore::LogSearch::Message,
            query: "fix".into(),
            entries: vec![entry("3", "the fix")],
            truncated: false,
        },
    );
    let mut h = harness(w);
    h.run();
    assert!(h.query_by_label_contains("1 commit found").is_some());
    assert!(h.query_by_label(s::SEARCH_IN).is_some());
    h.get_by_label(s::CLEAR).click();
    h.run();
    assert!(h.state().state.history.filter.results.is_none());
}

#[test]
fn browse_files_at_a_commit_opens_explore_there() {
    let mut w = world();
    w.state.tab = Tab::History;
    let e = entry("9", "old");
    assert!(w.state.history_action(HistoryAction::Browse, &e).is_none());
    assert_eq!(w.state.tab, Tab::Explore);
    assert_eq!(w.state.explore.rev, e.id);
}

#[test]
fn code_rows_reach_the_bottom_of_their_frame() {
    let mut w = world();
    w.state.explore.open_file("README.md");
    let text: String = (1..=400).map(|i| format!("line {i}\n")).collect();
    loaded(
        &mut w.state,
        ExploreResult::File {
            rev: "c0ffee1234".into(),
            path: "README.md".into(),
            content: FileContent::Text(text),
        },
    );
    let mut h = harness(w);
    h.run();
    let mut clip = None;
    let mut bottom = 0.0f32;
    for c in &h.output().shapes {
        if let egui::Shape::Text(t) = &c.shape
            && t.galley.job.text.contains("  line ")
        {
            clip = Some(c.clip_rect);
            bottom = bottom.max(t.pos.y + t.galley.size().y);
        }
    }
    let clip = clip.unwrap();
    assert!(
        bottom >= clip.bottom() - 17.0,
        "last row ends at {bottom}, the area at {}",
        clip.bottom()
    );
}

#[test]
fn clicking_a_symbolic_link_opens_it() {
    let mut w = world();
    loaded(
        &mut w.state,
        ExploreResult::Tree {
            rev: "HEAD".into(),
            commit: "c0ffee1234".into(),
            entries: vec![TreeEntry {
                path: "link".into(),
                kind: EntryKind::Symlink,
                size: 9,
            }],
        },
    );
    let mut h = harness(w);
    h.run();
    h.get_by_label("link").click();
    h.run();
    assert_eq!(h.state().state.explore.file.as_deref(), Some("link"));
}

/// Whether the row of `text` is drawn (only the visible rows are).
fn row_shown(h: &Harness<'static, World>, text: &str) -> bool {
    h.output()
        .shapes
        .iter()
        .any(|c| matches!(&c.shape, egui::Shape::Text(t) if t.galley.job.text.ends_with(text)))
}

#[test]
fn opening_the_same_search_result_again_scrolls_to_it_again() {
    let mut w = world();
    w.state.explore.open_match("README.md", 300);
    let text: String = (1..=400).map(|i| format!("line {i}\n")).collect();
    loaded(
        &mut w.state,
        ExploreResult::File {
            rev: "c0ffee1234".into(),
            path: "README.md".into(),
            content: FileContent::Text(text),
        },
    );
    let mut h = harness(w);
    h.run();
    h.run();
    assert!(row_shown(&h, "  line 300"), "scrolled to the match");
    // The user scrolls back to the top.
    h.event(egui::Event::PointerMoved(egui::pos2(800.0, 400.0)));
    h.event(egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: egui::vec2(0.0, 100_000.0),
        phase: egui::TouchPhase::Move,
        modifiers: egui::Modifiers::NONE,
    });
    // The wheel scrolls smoothly over several frames.
    h.run_steps(30);
    assert!(row_shown(&h, "  line 1"), "back at the top");
    assert!(!row_shown(&h, "  line 300"));
    h.state_mut().state.explore.open_match("README.md", 300);
    h.run();
    h.run();
    assert!(row_shown(&h, "  line 300"), "scrolled to the match again");
}
