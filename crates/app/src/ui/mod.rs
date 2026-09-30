//! Screens. Each function draws from `AppState` and sends `Command`s to the worker.

pub mod about;
pub mod changes;
pub mod clone_dialog;
pub mod diff_view;
pub mod discard;
pub mod main_window;
pub mod message;
pub mod sign_in;

use crate::state::AppState;
use crate::worker::WorkerHandle;

/// Everything a screen needs.
pub struct Ctx<'a> {
    pub state: &'a mut AppState,
    pub worker: &'a WorkerHandle,
}
