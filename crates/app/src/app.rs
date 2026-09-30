//! The eframe application: drains worker events, draws screens, persists config.

use std::path::PathBuf;

use crate::config::WindowGeometry;
use crate::protocol::Command;
use crate::state::AppState;
use crate::ui::{self, Ctx};
use crate::worker::WorkerHandle;

pub struct RetroGitApp {
    state: AppState,
    worker: WorkerHandle,
    config_path: Option<PathBuf>,
    geometry: Option<WindowGeometry>,
}

impl RetroGitApp {
    pub fn new(state: AppState, worker: WorkerHandle, config_path: Option<PathBuf>) -> RetroGitApp {
        worker.send(Command::ValidateToken);
        RetroGitApp {
            state,
            worker,
            config_path,
            geometry: None,
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

impl eframe::App for RetroGitApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        while let Ok(ev) = self.worker.events.try_recv() {
            self.state.apply(ev);
        }
        if self.state.config_dirty {
            self.save_config();
        }
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
        };
        ui::main_window::show(ui, &mut cx);
        ui::clone_dialog::show(&egui_ctx, &mut cx);
        ui::sign_in::show(&egui_ctx, &mut cx);
        ui::about::show(&egui_ctx, &mut cx);
        ui::message::show(&egui_ctx, &mut cx);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if !self.worker.shutdown(std::time::Duration::from_secs(3)) {
            log::warn!("worker still busy at exit");
        }
        self.save_config();
    }
}
