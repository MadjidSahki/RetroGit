#![allow(clippy::unwrap_used)]
//! View > Appearance: saved choice, zoom steps, the dialog and its preview.

use std::sync::Arc;

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use github::{Client, MemoryAccounts, TokenProvider};
use retrogit::config::{AppearanceConfig, Config};
use retrogit::state::{AppState, ZoomStep, next_zoom, size_label};
use retrogit::strings as s;
use retrogit::ui::Ctx;
use retrogit::worker::{WorkerDeps, WorkerHandle, spawn};
use win95::Scheme;
use win95::theme::{Appearance, Font};

#[test]
fn the_saved_appearance_tolerates_unknown_or_broken_values() {
    let d = AppearanceConfig::default();
    assert_eq!(d.appearance(), Appearance::default());
    assert_eq!(d.zoom(), 1.0);
    let odd = AppearanceConfig {
        scheme: "Hotdog Stand".into(),
        font: "Comic Sans".into(),
        zoom: 9.0,
    };
    assert_eq!(odd.appearance(), Appearance::default());
    assert_eq!(odd.zoom(), 2.0);
    assert_eq!(
        AppearanceConfig {
            zoom: f32::NAN,
            ..d.clone()
        }
        .zoom(),
        1.0
    );
    assert_eq!(
        AppearanceConfig {
            zoom: 0.1,
            ..d.clone()
        }
        .zoom(),
        0.8
    );
    let dark = AppearanceConfig::from_choice(
        Appearance {
            scheme: Scheme::Dark,
            font: Font::Atkinson,
        },
        1.25,
    );
    assert_eq!(dark.scheme, "Dark");
    assert_eq!(dark.font, "Atkinson Hyperlegible");
    // Old configuration files have no "appearance" entry.
    let cfg: Config = serde_json::from_str(r#"{"recent":[]}"#).unwrap();
    assert_eq!(cfg.appearance, AppearanceConfig::default());
    let back: Config = serde_json::from_str(
        &serde_json::to_string(&Config {
            appearance: dark.clone(),
            ..Config::default()
        })
        .unwrap(),
    )
    .unwrap();
    assert_eq!(back.appearance, dark);
}

#[test]
fn zoom_moves_by_ten_percent_within_bounds() {
    assert_eq!(next_zoom(1.0, ZoomStep::In), 1.1);
    assert_eq!(next_zoom(1.1, ZoomStep::In), 1.2);
    assert_eq!(next_zoom(1.25, ZoomStep::In), 1.35);
    assert_eq!(next_zoom(2.0, ZoomStep::In), 2.0);
    assert_eq!(next_zoom(0.8, ZoomStep::Out), 0.8);
    assert_eq!(next_zoom(1.0, ZoomStep::Out), 0.9);
    assert_eq!(next_zoom(1.7, ZoomStep::Reset), 1.0);
    assert_eq!(size_label(1.0), s::SIZE_SMALL);
    assert_eq!(size_label(1.25), s::SIZE_MEDIUM);
    assert_eq!(size_label(1.5), s::SIZE_LARGE);
    assert_eq!(size_label(1.1), "Custom (110%)");
}

#[test]
fn a_zoom_key_is_saved() {
    let mut st = AppState::new(Config::default());
    st.zoom_key(ZoomStep::In);
    assert_eq!(st.config.appearance.zoom, 1.1);
    assert!(st.config_dirty);
}

#[test]
fn a_new_scheme_forgets_the_syntax_colors_computed_for_the_old_one() {
    let mut st = AppState::new(Config::default());
    st.changes.diff_colors = retrogit::highlight::Colors::Plain;
    st.history.detail_colors = retrogit::highlight::Colors::Plain;
    st.stashes.colors = retrogit::highlight::Colors::Plain;
    st.pulls.file_colors = retrogit::highlight::Colors::Plain;
    st.forget_colors(true);
    let nr = retrogit::highlight::Colors::NotRequested;
    assert_eq!(st.changes.diff_colors, nr);
    assert_eq!(st.history.detail_colors, nr);
    assert_eq!(st.stashes.colors, nr);
    assert_eq!(st.pulls.file_colors, nr);
}

struct World {
    state: AppState,
    worker: WorkerHandle,
    highlighter: retrogit::highlight::Service,
    notices: std::sync::mpsc::Sender<retrogit::protocol::AppError>,
}

fn harness() -> Harness<'static, World> {
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
    let (notices, _rx) = std::sync::mpsc::channel();
    let mut state = AppState::new(Config::default());
    state.open_appearance();
    let w = World {
        state,
        worker,
        highlighter: retrogit::highlight::Service::start(|_| {}),
        notices,
    };
    let h = Harness::builder()
        .with_size(egui::vec2(900.0, 650.0))
        .build_ui_state(
            |ui, w: &mut World| {
                let ctx = ui.ctx().clone();
                let mut cx = Ctx {
                    state: &mut w.state,
                    worker: &w.worker,
                    highlighter: &w.highlighter,
                    notices: &w.notices,
                };
                retrogit::ui::appearance::show(&ctx, &mut cx);
            },
            w,
        );
    win95::theme::install(&h.ctx);
    h
}

#[test]
fn the_dialog_previews_then_applies_or_cancels() {
    let mut h = harness();
    h.run();
    assert!(h.query_by_label(s::PREVIEW_WINDOW_TEXT).is_some());
    assert!(h.query_by_label(s::PREVIEW_DISABLED).is_some());
    assert!(h.query_by_label(s::FONT_SAMPLE).is_some());
    assert!(h.query_by_label_contains("Windows Standard").is_some());
    // Pick Dark: nothing saved until Apply.
    h.state_mut()
        .state
        .appearance_dialog
        .as_mut()
        .unwrap()
        .scheme = Scheme::Dark;
    h.run();
    assert_eq!(h.state().state.config.appearance.scheme, "Windows Standard");
    h.get_by_label(s::APPLY).click();
    h.run();
    assert_eq!(h.state().state.config.appearance.scheme, "Dark");
    assert!(
        h.state().state.appearance_dialog.is_some(),
        "Apply keeps the window"
    );
    // Cancel goes back to what it was before the window opened.
    h.get_by_label(s::CANCEL).click();
    h.run();
    assert!(h.state().state.appearance_dialog.is_none());
    assert_eq!(h.state().state.config.appearance.scheme, "Windows Standard");
    // OK saves and closes.
    h.state_mut().state.open_appearance();
    h.state_mut().state.appearance_dialog.as_mut().unwrap().font = Font::Atkinson;
    h.state_mut().state.appearance_dialog.as_mut().unwrap().zoom = 1.25;
    h.run();
    h.get_by_label(s::OK).click();
    h.run();
    assert!(h.state().state.appearance_dialog.is_none());
    assert_eq!(
        h.state().state.config.appearance.font,
        "Atkinson Hyperlegible"
    );
    assert_eq!(h.state().state.config.appearance.zoom, 1.25);
    assert!(h.state().state.config_dirty);
}

#[test]
fn dark_schemes_get_dark_syntax_colors_and_late_light_ones_are_dropped() {
    use retrogit::highlight::{Colors, Service, Target, highlight};
    let code = ["fn main() { let x = \"text\"; }\n"];
    let light = highlight("a.rs", &code, false).unwrap();
    let dark = highlight("a.rs", &code, true).unwrap();
    assert_ne!(light, dark);
    // The service tags each result with the theme it used.
    let (tx, rx) = std::sync::mpsc::channel();
    let service = Service::start(move |h| {
        let _ = tx.send(h);
    });
    service.set_dark(true);
    let diff = retrogit::state::text_as_diff("a.rs", code[0]);
    service.request(Target::Changes, diff.clone());
    let got = rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
    assert!(got.dark);
    // A result computed before the switch is dropped.
    let mut st = AppState::new(Config::default());
    st.changes.diff = Some(diff.clone());
    st.forget_colors(true);
    st.apply(retrogit::protocol::Event::ColorsLoaded {
        target: Target::Changes,
        diff: diff.clone(),
        colors: None,
        dark: false,
    });
    assert_eq!(st.changes.diff_colors, Colors::NotRequested);
    st.apply(retrogit::protocol::Event::ColorsLoaded {
        target: Target::Changes,
        diff,
        colors: None,
        dark: true,
    });
    assert_eq!(st.changes.diff_colors, Colors::Plain);
}

#[test]
fn the_window_geometry_is_saved_in_unzoomed_points() {
    use retrogit::config::WindowGeometry;
    let inner = egui::Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(600.0, 400.0));
    let outer = egui::Rect::from_min_size(egui::pos2(10.0, 0.0), egui::vec2(600.0, 420.0));
    // At 150 %, egui reports a 900x600 window as 600x400 points.
    let g = WindowGeometry::from_viewport(inner, Some(outer), 1.5);
    assert_eq!(
        g,
        WindowGeometry {
            width: 900.0,
            height: 600.0,
            x: Some(15.0),
            y: Some(0.0),
        }
    );
}

#[test]
fn the_saved_appearance_is_in_place_for_the_first_frame() {
    let ctx = egui::Context::default();
    win95::theme::install(&ctx);
    let saved = AppearanceConfig::from_choice(
        Appearance {
            scheme: Scheme::Dark,
            font: Font::Atkinson,
        },
        1.5,
    );
    retrogit::app::apply_appearance(&ctx, &saved);
    let mut first_font = None;
    let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
        first_font = ui.ctx().fonts(|f| {
            f.definitions().families[&egui::FontFamily::Proportional]
                .first()
                .cloned()
        });
    });
    out.textures_delta.clear();
    assert_eq!(first_font.as_deref(), Some("Atkinson Hyperlegible"));
    assert_eq!(ctx.zoom_factor(), 1.5);
    assert_eq!(win95::theme::palette(&ctx), Scheme::Dark.palette());
    // The minimum size is in zoomed points: the layout always gets 520x360 points.
    let commands = &out.viewport_output[&egui::ViewportId::ROOT].commands;
    assert!(
        commands.contains(&egui::ViewportCommand::MinInnerSize(egui::vec2(
            520.0, 360.0
        ))),
        "{commands:?}"
    );
}

#[test]
fn the_window_geometry_is_read_without_locking_egui_twice() {
    let ctx = egui::Context::default();
    ctx.set_zoom_factor(1.5);
    let mut input = egui::RawInput::default();
    let vp = input.viewports.entry(egui::ViewportId::ROOT).or_default();
    vp.inner_rect = Some(egui::Rect::from_min_size(
        egui::pos2(0.0, 30.0),
        egui::vec2(600.0, 400.0),
    ));
    vp.outer_rect = Some(egui::Rect::from_min_size(
        egui::pos2(0.0, 0.0),
        egui::vec2(600.0, 430.0),
    ));
    let mut got = None;
    for _ in 0..2 {
        let mut out = ctx.run_ui(input.clone(), |ui| {
            got = retrogit::app::viewport_geometry(ui.ctx());
        });
        out.textures_delta.clear();
    }
    let g = got.unwrap();
    assert_eq!((g.width, g.height), (900.0, 600.0));
}
