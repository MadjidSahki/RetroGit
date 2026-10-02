//! The eframe application: drains worker events, draws screens, persists config.

use std::path::PathBuf;

use crate::config::WindowGeometry;
use crate::protocol::Command;
use crate::state::AppState;
use crate::ui::{self, Ctx};
use crate::watch::Watcher;
use crate::worker::WorkerHandle;

pub struct RetroGitApp {
    state: AppState,
    worker: WorkerHandle,
    config_path: Option<PathBuf>,
    geometry: Option<WindowGeometry>,
    /// Watches the open repository; replaced when another one is opened.
    /// `None` inside means watching failed for that path (not retried every frame).
    watcher: Option<(PathBuf, Option<Watcher>)>,
    was_focused: bool,
    highlighter: crate::highlight::Service,
    highlighted: std::sync::mpsc::Receiver<crate::highlight::Highlighted>,
    /// Result of the IDE detection started at launch.
    ides: Option<std::sync::mpsc::Receiver<Vec<crate::ide::Ide>>>,
    notices_tx: std::sync::mpsc::Sender<crate::protocol::AppError>,
    notices: std::sync::mpsc::Receiver<crate::protocol::AppError>,
    /// Folders sent by `retrogit` from a terminal (see `instance`).
    to_open: Option<std::sync::mpsc::Receiver<PathBuf>>,
    _instance: Option<crate::instance::Server>,
    /// Pull request events from the watcher thread.
    pr_events: Option<std::sync::mpsc::Receiver<Vec<github::PrEvent>>>,
    _pr_watcher: Option<crate::pr_watch::PrWatcher>,
    /// Appearance and zoom last applied to the egui context.
    applied: Option<(win95::theme::Appearance, f32)>,
}

impl RetroGitApp {
    /// `ctx` is used to wake the UI up when background syntax colors are ready.
    pub fn new(
        state: AppState,
        worker: WorkerHandle,
        config_path: Option<PathBuf>,
        ctx: egui::Context,
    ) -> RetroGitApp {
        worker.send(Command::ValidateToken);
        // Cmd/Ctrl +, - and 0 are handled here (bounded steps, saved in the config).
        ctx.options_mut(|o| o.zoom_with_keyboard = false);
        let ides_ctx = ctx.clone();
        let ides_ctx_for_appearance = ctx.clone();
        let (tx, highlighted) = std::sync::mpsc::channel();
        let highlighter = crate::highlight::Service::start(move |h| {
            let _ = tx.send(h);
            ctx.request_repaint();
        });
        let (notices_tx, notices) = std::sync::mpsc::channel();
        let (ides_tx, ides) = std::sync::mpsc::channel();
        let repaint = ides_ctx;
        std::thread::spawn(move || {
            let _ = ides_tx.send(crate::ide::detect());
            repaint.request_repaint();
        });
        let mut app = RetroGitApp {
            ides: Some(ides),
            notices_tx,
            notices,
            to_open: None,
            _instance: None,
            pr_events: None,
            _pr_watcher: None,
            highlighter,
            highlighted,
            state,
            worker,
            config_path,
            geometry: None,
            watcher: None,
            was_focused: true,
            applied: None,
        };
        // Saved scheme, font and zoom in place before the window first shows.
        app.sync_appearance(&ides_ctx_for_appearance);
        app
    }

    /// Accept folders from the `retrogit` command (single instance), and open `initial`.
    pub fn with_instance(
        mut self,
        dir: Option<&std::path::Path>,
        ctx: egui::Context,
        initial: Option<PathBuf>,
    ) -> RetroGitApp {
        if let Some(path) = initial {
            self.worker.send(Command::OpenRepo(path));
        }
        let Some(dir) = dir else { return self };
        let (tx, rx) = std::sync::mpsc::channel();
        match crate::instance::Server::start(dir, move |p| {
            let _ = tx.send(p);
            ctx.request_repaint();
        }) {
            Ok(server) => {
                self._instance = Some(server);
                self.to_open = Some(rx);
            }
            Err(e) => log::warn!("single-instance listener not started: {e}"),
        }
        self
    }

    /// Watch the user's pull requests (all repositories) for notifications.
    pub fn with_pr_watch(
        mut self,
        client: github::Client,
        tokens: github::TokenProvider,
        ctx: egui::Context,
    ) -> RetroGitApp {
        let (tx, rx) = std::sync::mpsc::channel();
        let accounts = self.worker.accounts();
        let watcher = crate::pr_watch::PrWatcher::start(
            client,
            tokens,
            self.worker.accounts(),
            crate::pr_watch::INTERVAL,
            move |events: Vec<github::PrEvent>| {
                let several = accounts.list().len() > 1;
                for e in &events {
                    let (title, body) = crate::notify::notification_text(e, several);
                    crate::notify::show(&title, &body);
                }
                let _ = tx.send(events);
                ctx.request_repaint();
            },
        );
        self.pr_events = Some(rx);
        self._pr_watcher = Some(watcher);
        self
    }

    /// Keep the file watcher on the current repository.
    fn sync_watcher(&mut self) {
        let current = self.state.current.as_ref().map(|c| c.path.clone());
        if self.watcher.as_ref().map(|(p, _)| p) == current.as_ref() {
            return;
        }
        self.watcher = current.map(|path| {
            let w = match Watcher::start(&path, self.worker.refresher()) {
                Ok(w) => Some(w),
                Err(e) => {
                    log::warn!(
                        "cannot watch {}: {e}; refresh on focus and with Refresh",
                        path.display()
                    );
                    None
                }
            };
            (path, w)
        });
    }

    /// Apply the saved appearance when it changed (and at the first frame).
    fn sync_appearance(&mut self, ctx: &egui::Context) {
        let saved = &self.state.config.appearance;
        let want = (saved.appearance(), saved.zoom());
        if self.applied == Some(want) {
            return;
        }
        let dark = want.0.scheme.palette().dark;
        apply_appearance(ctx, &self.state.config.appearance);
        self.highlighter.set_dark(dark);
        if self.applied.map(|(a, _)| a.scheme.palette().dark) != Some(dark) {
            self.state.forget_colors(dark);
        }
        self.applied = Some(want);
    }

    fn zoom_keys(&mut self, ctx: &egui::Context) {
        use crate::state::ZoomStep;
        let cmd = egui::Modifiers::COMMAND;
        let step = ctx.input_mut(|i| {
            if i.consume_key(cmd, egui::Key::Plus)
                || i.consume_key(cmd, egui::Key::Equals)
                || i.consume_key(cmd | egui::Modifiers::SHIFT, egui::Key::Equals)
            {
                Some(ZoomStep::In)
            } else if i.consume_key(cmd, egui::Key::Minus) {
                Some(ZoomStep::Out)
            } else if i.consume_key(cmd, egui::Key::Num0) {
                Some(ZoomStep::Reset)
            } else {
                None
            }
        });
        if let Some(step) = step {
            self.state.zoom_key(step);
        }
    }

    fn save_config(&mut self) {
        if let Some(g) = self.geometry {
            self.state.config.window = Some(g);
        }
        if let Some(path) = &self.config_path
            && let Err(e) = self.state.config.save_to(path)
        {
            log::warn!("could not save config: {e}");
        }
        self.state.config_dirty = false;
    }
}

/// Scheme, font and zoom of `saved` on `ctx`; the minimum window size follows the zoom.
/// Called before the first frame (fonts and zoom take effect at the next pass).
pub fn apply_appearance(ctx: &egui::Context, saved: &crate::config::AppearanceConfig) {
    win95::theme::apply(ctx, saved.appearance());
    ctx.set_zoom_factor(saved.zoom());
    ctx.send_viewport_cmd(egui::ViewportCommand::MinInnerSize(egui::Vec2::from(
        crate::config::MIN_WINDOW,
    )));
}

impl eframe::App for RetroGitApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        while let Ok(ev) = self.worker.events.try_recv() {
            self.state.apply(ev);
        }
        let requested: Vec<PathBuf> = self
            .to_open
            .as_ref()
            .map(|rx| rx.try_iter().collect())
            .unwrap_or_default();
        for path in requested {
            self.worker.send(Command::OpenRepo(path));
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
        if let Some(found) = self.ides.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.state.ides = found;
            self.ides = None;
        }
        let pr_events: Vec<Vec<github::PrEvent>> = self
            .pr_events
            .as_ref()
            .map(|rx| rx.try_iter().collect())
            .unwrap_or_default();
        for events in pr_events {
            self.state.apply(crate::protocol::Event::PrEvents(events));
        }
        while let Ok(notice) = self.notices.try_recv() {
            self.state.messages.push_back(notice);
        }
        while let Ok(h) = self.highlighted.try_recv() {
            self.state.apply(crate::protocol::Event::ColorsLoaded {
                target: h.target,
                diff: h.diff,
                colors: h.colors,
                dark: h.dark,
            });
        }
        self.zoom_keys(ctx);
        self.sync_appearance(ctx);
        if self.state.config_dirty {
            self.save_config();
        }
        self.sync_watcher();
        let focused = ctx.input(|i| i.viewport().focused.unwrap_or(true));
        if focused && !self.was_focused && self.state.current.is_some() {
            self.worker.send(Command::RefreshStatus);
        }
        self.was_focused = focused;
        ctx.input(|i| {
            let vp = i.viewport();
            if vp.maximized != Some(true)
                && let Some(inner) = vp.inner_rect
            {
                self.geometry = Some(WindowGeometry::from_viewport(
                    inner,
                    vp.outer_rect,
                    ctx.zoom_factor(),
                ));
            }
        });
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let egui_ctx = ui.ctx().clone();
        let mut cx = Ctx {
            state: &mut self.state,
            worker: &self.worker,
            highlighter: &self.highlighter,
            notices: &self.notices_tx,
        };
        ui::main_window::show(ui, &mut cx);
        ui::clone_dialog::show(&egui_ctx, &mut cx);
        ui::accounts::show(&egui_ctx, &mut cx);
        ui::sign_in::show(&egui_ctx, &mut cx);
        ui::about::show(&egui_ctx, &mut cx);
        ui::appearance::show(&egui_ctx, &mut cx);
        ui::discard::show(&egui_ctx, &mut cx);
        ui::sync_dialogs::show(&egui_ctx, &mut cx);
        ui::pull_dialogs::show(&egui_ctx, &mut cx);
        ui::git_dialogs::show(&egui_ctx, &mut cx);
        ui::notifications::show(&egui_ctx, &mut cx);
        ui::message::show(&egui_ctx, &mut cx);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if !self.worker.shutdown(std::time::Duration::from_secs(10)) {
            log::warn!("worker still busy at exit");
        }
        self.save_config();
    }
}
