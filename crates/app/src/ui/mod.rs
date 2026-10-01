//! Screens. Each function draws from `AppState` and sends `Command`s to the worker.

pub mod about;
pub mod changes;
pub mod clone_dialog;
pub mod diff_view;
pub mod discard;
pub mod history;
pub mod main_window;
pub mod message;
pub mod sign_in;
pub mod sync_dialogs;
pub mod sync_toolbar;

use crate::state::AppState;
use crate::worker::WorkerHandle;

/// Everything a screen needs.
pub struct Ctx<'a> {
    pub state: &'a mut AppState,
    pub worker: &'a WorkerHandle,
    /// Background syntax highlighting.
    pub highlighter: &'a crate::highlight::Service,
    /// Message boxes produced by background jobs started from the UI.
    pub notices: &'a std::sync::mpsc::Sender<crate::protocol::AppError>,
}
