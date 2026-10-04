//! The eframe application: drains worker events, draws screens, persists config.

use std::path::PathBuf;

use crate::config::WindowGeometry;
use crate::protocol::Command;
use crate::state::AppState;
use crate::ui::{self, Ctx};
use crate::watch::Watcher;
use crate::worker::WorkerHandle;

enum InstallMsg {
    Progress(u64, Option<u64>),
    Downloaded,
    Done(Result<Option<Vec<String>>, String>),
}

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
    /// Update checks (6g) and their answers.
    updates: Option<crate::update::Checker>,
    update_results:
        Option<std::sync::mpsc::Receiver<(Result<crate::update::Release, String>, bool)>>,
    /// Follows the "Check for updates automatically" setting.
    updates_enabled: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// The installation running: its progress and result, and how to cancel it.
    installing: Option<(
        std::sync::mpsc::Receiver<InstallMsg>,
        std::sync::Arc<std::sync::atomic::AtomicBool>,
    )>,
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
        #[cfg(target_os = "macos")]
        {
            let wake = ctx.clone();
            crate::notify::macos::set_waker(move || wake.request_repaint());
        }
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
            updates: None,
            update_results: None,
            updates_enabled: Default::default(),
            installing: None,
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
            self.open_requested(path);
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

    /// Check GitHub for a newer RetroGit 30 s after start, then every day (if enabled).
    pub fn with_updates(mut self, api: &str, ctx: egui::Context) -> RetroGitApp {
        if let Ok(exe) = std::env::current_exe() {
            let kind = crate::update::install_kind(std::env::consts::OS, &exe, |p| p.exists());
            // What an earlier update left behind (the old copy, the work folder).
            crate::update::cleanup(&kind);
            self.state.update.can_replace = crate::update::can_replace(&kind);
            self.state.update.kind = kind;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let enabled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(
            self.state.config.updates.check,
        ));
        self.updates_enabled = enabled.clone();
        self.updates = Some(crate::update::Checker::start(
            api.to_string(),
            std::time::Duration::from_secs(30),
            std::time::Duration::from_secs(24 * 3600),
            move || enabled.load(std::sync::atomic::Ordering::SeqCst),
            move |result, manual| {
                let _ = tx.send((result, manual));
                ctx.request_repaint();
            },
        ));
        self.update_results = Some(rx);
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
                    crate::notify::show(&title, &body, Some(crate::notify::event_link(e)));
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

    /// Start the installation when asked (and the worker is free), follow it, and quit once
    /// the new version runs.
    fn drive_install(&mut self, ctx: &egui::Context) {
        use std::sync::atomic::Ordering;
        if self.state.take_install(self.worker.is_busy())
            && let Some(release) = self.state.update.available.clone()
        {
            let (tx, rx) = std::sync::mpsc::channel();
            let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let (kind, stop, wake) = (self.state.update.kind.clone(), cancel.clone(), ctx.clone());
            std::thread::spawn(move || {
                let progress_tx = tx.clone();
                let progress_wake = wake.clone();
                let downloaded_tx = tx.clone();
                let result = crate::update::install(
                    &kind,
                    &release,
                    &stop,
                    |done, total| {
                        let _ = progress_tx.send(InstallMsg::Progress(done, total));
                        progress_wake.request_repaint();
                    },
                    move || {
                        let _ = downloaded_tx.send(InstallMsg::Downloaded);
                    },
                );
                let _ = tx.send(InstallMsg::Done(result));
                wake.request_repaint();
            });
            self.installing = Some((rx, cancel));
        }
        if self.state.update.waiting {
            // Ask again soon: the worker may be free.
            ctx.request_repaint_after(std::time::Duration::from_millis(500));
        }
        if let Some((rx, cancel)) = &self.installing {
            if self.state.update.cancel_requested {
                cancel.store(true, Ordering::SeqCst);
            }
            let msgs: Vec<InstallMsg> = rx.try_iter().collect();
            for m in msgs {
                match m {
                    InstallMsg::Progress(done, total) => self.state.update_progress(done, total),
                    InstallMsg::Downloaded => self.state.update_downloaded(),
                    InstallMsg::Done(result) => {
                        if let Err(e) = &result {
                            log::warn!("update failed: {e}");
                        }
                        self.state.update_finished(result);
                        self.installing = None;
                        break;
                    }
                }
            }
        }
        if self.state.update.relaunch.is_some() {
            if let Some(argv) = self.state.take_relaunch(self.worker.is_busy()) {
                // Saved first: the new copy reads the settings at start.
                self.save_config();
                let result = crate::update::start(&argv);
                self.state.relaunched(result);
            } else {
                ctx.request_repaint_after(std::time::Duration::from_millis(500));
            }
        }
        if self.state.update.quit {
            self.save_config();
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    /// A folder to open, or a `retrogit://` notification link (from the command line or the
    /// instance channel).
    fn open_requested(&mut self, path: PathBuf) {
        match crate::cli::route(path) {
            crate::cli::Requested::Link(link) => self.state.open_link(&link),
            crate::cli::Requested::Folder(path) => {
                if let Some(cmd) = self.state.request_repo_switch(Command::OpenRepo(path)) {
                    self.worker.send(cmd);
                }
            }
        }
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

/// Size and position of the window to save (`None` when maximized or unknown).
pub fn viewport_geometry(ctx: &egui::Context) -> Option<WindowGeometry> {
    // Read before `input`: egui's context lock is not reentrant.
    let zoom = ctx.zoom_factor();
    ctx.input(|i| {
        let vp = i.viewport();
        if vp.maximized == Some(true) {
            return None;
        }
        vp.inner_rect
            .map(|inner| WindowGeometry::from_viewport(inner, vp.outer_rect, zoom))
    })
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

/// What brings the window back in front: out of the Dock first, then focused.
pub fn bring_to_front() -> [egui::ViewportCommand; 2] {
    [
        egui::ViewportCommand::Minimized(false),
        egui::ViewportCommand::Focus,
    ]
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
            self.open_requested(path);
            for c in bring_to_front() {
                ctx.send_viewport_cmd(c);
            }
        }
        #[cfg(target_os = "macos")]
        for link in crate::notify::macos::take_clicked() {
            self.state.open_link(&link);
            for c in bring_to_front() {
                ctx.send_viewport_cmd(c);
            }
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
        let answers: Vec<_> = self
            .update_results
            .as_ref()
            .map(|rx| rx.try_iter().collect())
            .unwrap_or_default();
        for (result, manual) in answers {
            self.state
                .update_checked(crate::version::version(), result, manual);
        }
        if std::mem::take(&mut self.state.update.check_requested)
            && let Some(c) = &self.updates
        {
            c.check_now();
        }
        self.drive_install(ctx);
        self.updates_enabled.store(
            self.state.config.updates.check,
            std::sync::atomic::Ordering::SeqCst,
        );
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
        if let Some(g) = viewport_geometry(ctx) {
            self.geometry = Some(g);
        }
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
        ui::update::show(&egui_ctx, &mut cx);
        ui::discard::show(&egui_ctx, &mut cx);
        ui::sync_dialogs::show(&egui_ctx, &mut cx);
        ui::pull_dialogs::show(&egui_ctx, &mut cx);
        ui::git_dialogs::show(&egui_ctx, &mut cx);
        ui::explore::search_dialog(&egui_ctx, &mut cx);
        ui::notifications::show(&egui_ctx, &mut cx);
        ui::notifications::open_links(&egui_ctx, &mut cx);
        ui::message::show(&egui_ctx, &mut cx);
        egui_ctx.output_mut(|o| retain_safe_urls(&mut o.commands));
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if !self.worker.shutdown(std::time::Duration::from_secs(10)) {
            log::warn!("worker still busy at exit");
        }
        self.save_config();
    }
}

/// Drop every request to open an address that is not a web page or mail
/// (`win95::markdown::safe_link`): no `file:` or custom scheme leaves the app.
pub fn retain_safe_urls(cmds: &mut Vec<egui::OutputCommand>) {
    cmds.retain(|c| match c {
        egui::OutputCommand::OpenUrl(o) => {
            let safe = win95::markdown::safe_link(&o.url);
            if !safe {
                log::warn!("not opening {}", o.url);
            }
            safe
        }
        _ => true,
    });
}
