//! Updates: the release found, the update window, the check asked by the user.

use super::AppState;
use crate::protocol::{AppError, Severity};
use crate::strings as s;
use crate::update::{InstallKind, Release, newer};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateView {
    /// A newer release, not skipped.
    pub available: Option<Release>,
    /// The update window is shown.
    pub open: bool,
    /// A check asked by the user is running.
    pub checking: bool,
    /// `true` once: the UI asks the checker now.
    pub check_requested: bool,
    /// How this copy was installed (set at start).
    pub kind: InstallKind,
    /// Install and restart was clicked (the app starts the installation).
    pub install_requested: bool,
    /// Waiting for a Git operation to finish before installing.
    pub waiting: bool,
    pub installing: bool,
    /// Bytes downloaded, and the total when known.
    pub progress: Option<(u64, Option<u64>)>,
    pub cancel_requested: bool,
    /// Why the last installation failed (shown in the window).
    pub error: Option<String>,
    /// The new version was started: this one saves its settings and closes.
    pub quit: bool,
    /// This copy's folder can be written (set at start).
    pub can_replace: bool,
}

impl Default for UpdateView {
    fn default() -> Self {
        UpdateView {
            available: None,
            open: false,
            checking: false,
            check_requested: false,
            kind: InstallKind::Development,
            install_requested: false,
            waiting: false,
            installing: false,
            progress: None,
            cancel_requested: false,
            error: None,
            quit: false,
            can_replace: false,
        }
    }
}

impl AppState {
    /// Help > Check for updates.
    pub fn request_update_check(&mut self) {
        self.update.checking = true;
        self.update.check_requested = true;
    }

    /// The checker answered (`manual`: asked by the user, who always gets an answer).
    pub fn update_checked(&mut self, current: &str, result: Result<Release, String>, manual: bool) {
        if manual {
            self.update.checking = false;
        }
        match result {
            Ok(r) => {
                // A manual check offers a skipped version again.
                let skipped = if manual {
                    None
                } else {
                    self.config.updates.skipped.as_deref()
                };
                if newer(current, &r.version, skipped) {
                    self.update.available = Some(r);
                    if manual {
                        self.update.open = true;
                    }
                } else if manual {
                    self.messages.push_back(AppError::new(
                        Severity::Info,
                        &s::RETROGIT_UP_TO_DATE.replace("{version}", current),
                    ));
                }
            }
            Err(e) => {
                log::info!("update check failed: {e}");
                if manual {
                    let mut err = AppError::new(Severity::Warning, s::ERR_UPDATE_CHECK);
                    err.detail = Some(e);
                    self.messages.push_back(err);
                }
            }
        }
    }

    /// `true` once when the installation may start (Install was clicked and the worker is
    /// not running a Git operation).
    pub fn take_install(&mut self, worker_busy: bool) -> bool {
        let u = &mut self.update;
        if !u.install_requested || u.installing {
            return false;
        }
        if worker_busy {
            u.waiting = true;
            return false;
        }
        u.install_requested = false;
        u.waiting = false;
        u.installing = true;
        u.cancel_requested = false;
        u.error = None;
        u.progress = None;
        true
    }

    pub fn update_progress(&mut self, done: u64, total: Option<u64>) {
        self.update.progress = Some((done, total));
    }

    pub fn update_finished(&mut self, result: Result<(), String>) {
        let u = &mut self.update;
        u.installing = false;
        u.progress = None;
        match result {
            Ok(()) => u.quit = true,
            Err(e) => u.error = Some(e),
        }
    }

    pub fn skip_update(&mut self) {
        if let Some(r) = self.update.available.take() {
            self.config.updates.skipped = Some(r.version);
            self.config_dirty = true;
        }
        self.update.open = false;
    }

    pub fn set_update_checks(&mut self, on: bool) {
        if self.config.updates.check != on {
            self.config.updates.check = on;
            self.config_dirty = true;
        }
    }
}
