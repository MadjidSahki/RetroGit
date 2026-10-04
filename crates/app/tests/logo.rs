//! The logo: window icon, title bar icon and About box.

use retrogit::ui::logo;

#[test]
fn the_icons_are_square_rgba_images() {
    let icon = logo::window_icon();
    assert_eq!((icon.width, icon.height), (256, 256));
    assert_eq!(icon.rgba.len(), 256 * 256 * 4);
    assert_eq!(logo::ICON_32.len(), 32 * 32 * 4);
    assert_eq!(logo::LOGO_128.len(), 128 * 128 * 4);
    // Opaque teal background (the logo has no transparency).
    let [r, g, b, a] = [icon.rgba[0], icon.rgba[1], icon.rgba[2], icon.rgba[3]];
    assert!(
        r < 10 && g > 100 && b > 100 && a == 255,
        "{:?}",
        &icon.rgba[..4]
    );
}

#[test]
fn the_textures_are_loaded_once_per_context() {
    let ctx = egui::Context::default();
    let a = logo::icon_texture(&ctx);
    let b = logo::icon_texture(&ctx);
    assert_eq!(a.id(), b.id());
    assert_eq!(a.size(), [32, 32]);
    assert_eq!(logo::logo_texture(&ctx).size(), [128, 128]);
}

#[test]
fn the_main_title_bar_and_the_about_box_show_the_logo() {
    use std::sync::Arc;
    let server = mockito::Server::new();
    let worker = retrogit::worker::spawn(
        retrogit::worker::WorkerDeps {
            client: github::Client::with_bases(&server.url(), &server.url()),
            store: Arc::new(github::MemoryAccounts::default()),
            client_id: String::new(),
            commit_backend: gitcore::CommitBackend::Git2,
            tokens: github::TokenProvider::without_gh(),
            known_accounts: Vec::new(),
            repo_accounts: Default::default(),
        },
        || {},
    );
    let highlighter = retrogit::highlight::Service::start(|_| {});
    let (notices, _rx) = std::sync::mpsc::channel();
    let mut state = retrogit::state::AppState::new(retrogit::config::Config::default());
    state.about = true;
    let ctx = egui::Context::default();
    win95::theme::install(&ctx);
    let mut frame = || {
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            let egui_ctx = ui.ctx().clone();
            let mut cx = retrogit::ui::Ctx {
                state: &mut state,
                worker: &worker,
                highlighter: &highlighter,
                notices: &notices,
            };
            retrogit::ui::main_window::show(ui, &mut cx);
            retrogit::ui::about::show(&egui_ctx, &mut cx);
        });
        out.textures_delta.clear();
        out
    };
    // Windows and areas are measured, not shown, on their first frame.
    let _ = frame();
    let _ = frame();
    let out = frame();
    let textures: Vec<egui::TextureId> = out
        .shapes
        .iter()
        .filter_map(|s| match &s.shape {
            egui::Shape::Mesh(m) => Some(m.texture_id),
            // egui::Image paints a rectangle filled with the texture.
            egui::Shape::Rect(r) => r.brush.as_ref().map(|b| b.fill_texture_id),
            _ => None,
        })
        .collect();
    assert!(
        textures.contains(&logo::icon_texture(&ctx).id()),
        "title bar icon"
    );
    assert!(
        textures.contains(&logo::logo_texture(&ctx).id()),
        "About logo"
    );
}
