#![allow(clippy::unwrap_used)]
//! The main screens paint with the chosen scheme: no Windows Standard color in Dark.

use std::path::PathBuf;
use std::sync::Arc;

use egui::{Color32, Shape};
use gitcore::{
    Change, ChangedFile, CommitDetail, DiffLine, FileDiff, FileStatus, Head, Hunk, LineKind,
    LogEntry, RefKind, RefLabel, RepoSummary, Side,
};
use github::{Client, MemoryAccounts, TokenProvider};
use retrogit::config::Config;
use retrogit::protocol::Event;
use retrogit::state::{AppState, Tab};
use retrogit::ui::Ctx;
use retrogit::worker::{WorkerDeps, spawn};
use win95::Scheme;
use win95::theme::{self, Appearance, Font};

fn collect(shape: &Shape, fills: &mut Vec<Color32>, texts: &mut Vec<Color32>) {
    match shape {
        Shape::Vec(v) => v.iter().for_each(|s| collect(s, fills, texts)),
        // Images (the logo) are tinted white: not a fill color.
        Shape::Rect(r) if r.brush.is_some() => {}
        Shape::Rect(r) => fills.push(r.fill),
        Shape::Circle(c) => fills.extend([c.fill, c.stroke.color]),
        Shape::LineSegment { stroke, .. } => fills.push(stroke.color),
        Shape::Mesh(m) if m.texture_id != egui::TextureId::default() => {}
        Shape::Mesh(m) => fills.extend(m.vertices.iter().map(|v| v.color)),
        Shape::Text(t) => {
            texts.extend(t.galley.job.sections.iter().map(|s| s.format.color));
            fills.extend(t.galley.job.sections.iter().map(|s| s.format.background));
        }
        _ => {}
    }
}

fn diff() -> FileDiff {
    let line = |kind, text: &str| DiffLine {
        kind,
        old_no: Some(1),
        new_no: Some(1),
        text: format!("{text}\n"),
        raw: format!("{text}\n").into_bytes(),
        no_newline_at_eof: false,
    };
    FileDiff {
        path: "src/main.rs".into(),
        side: Side::Unstaged,
        binary: false,
        hunks: vec![Hunk {
            header: "@@ -1,2 +1,2 @@".into(),
            old_start: 1,
            old_lines: 2,
            new_start: 1,
            new_lines: 2,
            lines: vec![
                line(LineKind::Context, "fn main() {"),
                line(LineKind::Removed, "    old();"),
                line(LineKind::Added, "    new();"),
            ],
        }],
    }
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
    st.apply(Event::StatusLoaded(vec![FileStatus {
        path: "src/main.rs".into(),
        staged: None,
        unstaged: Some(Change::Modified),
    }]));
    st.apply(Event::OperationChanged(Some(
        gitcore::Operation::CherryPick,
    )));
    st.changes.shown = Some(("src/main.rs".into(), Side::Unstaged));
    st.apply(Event::DiffLoaded(diff()));
    st.apply(Event::LogLoaded {
        skip: 0,
        entries: ["one", "two"]
            .iter()
            .enumerate()
            .map(|(i, s)| LogEntry {
                id: format!("{i}{}", "0".repeat(39)),
                short_id: format!("{i}000000"),
                parents: if i == 0 {
                    vec![format!("1{}", "0".repeat(39))]
                } else {
                    Vec::new()
                },
                author: "Ada".into(),
                email: "a@b".into(),
                time: 0,
                summary: (*s).into(),
                refs: if i == 0 {
                    [
                        RefKind::Head,
                        RefKind::LocalBranch,
                        RefKind::RemoteBranch,
                        RefKind::Tag,
                    ]
                    .into_iter()
                    .map(|kind| RefLabel {
                        name: format!("{kind:?}"),
                        kind,
                    })
                    .collect()
                } else {
                    Vec::new()
                },
            })
            .collect(),
    });
    st
}

fn paint(tab: Tab, scheme: Scheme) -> (Vec<Color32>, Vec<Color32>) {
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
    let highlighter = retrogit::highlight::Service::start(|_| {});
    let (notices, _rx) = std::sync::mpsc::channel();
    let mut st = state();
    st.tab = tab;
    if tab == Tab::History {
        let id = st.history.entries[0].id.clone();
        st.select_commit(&id);
        st.apply(Event::CommitLoaded(CommitDetail {
            id: id.clone(),
            short_id: "0000000".into(),
            parents: Vec::new(),
            author: "Ada".into(),
            email: "a@b".into(),
            time: 0,
            committer: "Ada".into(),
            message: "one\n\nbody".into(),
            files: vec![ChangedFile {
                path: "src/main.rs".into(),
                change: Change::Modified,
            }],
        }));
        st.apply(Event::CommitFileDiffLoaded { id, diff: diff() });
    }
    let ctx = egui::Context::default();
    theme::install(&ctx);
    theme::apply(
        &ctx,
        Appearance {
            scheme,
            font: Font::W95fa,
        },
    );
    let mut out = None;
    for _ in 0..3 {
        let mut o = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1100.0, 700.0),
                )),
                ..Default::default()
            },
            |ui| {
                let mut cx = Ctx {
                    state: &mut st,
                    worker: &worker,
                    highlighter: &highlighter,
                    notices: &notices,
                };
                retrogit::ui::main_window::show(ui, &mut cx);
            },
        );
        o.textures_delta.clear();
        out = Some(o);
    }
    let (mut fills, mut texts) = (Vec::new(), Vec::new());
    for s in out.unwrap().shapes {
        collect(&s.shape, &mut fills, &mut texts);
    }
    (fills, texts)
}

fn rgb(hex: u32) -> Color32 {
    Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

#[test]
fn dark_screens_paint_no_standard_color() {
    let standard_only = [
        0xC0C0C0, 0xDFDFDF, 0x000080, 0x1084D0, 0x808080, 0xE6FFE6, 0xFFE6E6, 0xE0E0F0, 0xFFFFC0,
        0xFFFF80, 0xC0FFC0, 0xC0D8FF, 0xFFD8A0,
    ];
    for tab in [Tab::Changes, Tab::History, Tab::Stashes] {
        let (fills, texts) = paint(tab, Scheme::Dark);
        let mut bad = Vec::new();
        for hex in standard_only {
            if fills.contains(&rgb(hex)) || texts.contains(&rgb(hex)) {
                bad.push(format!("#{hex:06X}"));
            }
        }
        if fills.contains(&Color32::WHITE) {
            bad.push("white fill".into());
        }
        if texts.contains(&Color32::BLACK) {
            bad.push("black text".into());
        }
        assert!(bad.is_empty(), "{tab:?} in Dark paints {}", bad.join(", "));
        let dark = Scheme::Dark.palette();
        assert!(
            fills.contains(&dark.window),
            "{tab:?}: the wells use the Dark window color"
        );
    }
    let (fills, _) = paint(Tab::Changes, Scheme::Dark);
    let dark = Scheme::Dark.palette();
    assert!(
        fills.contains(&dark.added) && fills.contains(&dark.removed),
        "diff colors"
    );
    let (fills, _) = paint(Tab::History, Scheme::Dark);
    assert!(
        fills.contains(&dark.ref_head) && fills.contains(&dark.lanes[0]),
        "graph colors"
    );
}
