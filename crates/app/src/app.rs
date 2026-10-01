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
        let ides_ctx = ctx.clone();
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
        RetroGitApp {
            ides: Some(ides),
            notices_tx,
            notices,
            to_open: None,
            _instance: None,
            highlighter,
            highlighted,
            state,
            worker,
            config_path,
            geometry: None,
            watcher: None,
            was_focused: true,
        }
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
        while let Ok(notice) = self.notices.try_recv() {
            self.state.messages.push_back(notice);
        }
        while let Ok(h) = self.highlighted.try_recv() {
            self.state.apply(crate::protocol::Event::ColorsLoaded {
                target: h.target,
                diff: h.diff,
                colors: h.colors,
            });
        }
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
                let outer = vp.outer_rect;
                self.geometry = Some(WindowGeometry {
                    width: inner.width(),
                    height: inner.height(),
                    x: outer.map(|r| r.left()),
                    y: outer.map(|r| r.top()),
                });
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
        ui::sign_in::show(&egui_ctx, &mut cx);
        ui::about::show(&egui_ctx, &mut cx);
        ui::discard::show(&egui_ctx, &mut cx);
        ui::sync_dialogs::show(&egui_ctx, &mut cx);
        ui::pull_dialogs::show(&egui_ctx, &mut cx);
        ui::message::show(&egui_ctx, &mut cx);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if !self.worker.shutdown(std::time::Duration::from_secs(10)) {
            log::warn!("worker still busy at exit");
        }
        self.save_config();
    }
}
