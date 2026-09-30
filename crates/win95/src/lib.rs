//! Windows 95 look-and-feel for egui. No business logic lives here.

pub mod bevel;
pub mod button;
pub mod dialog;
pub mod icon;
pub mod panel;
pub mod status_bar;
pub mod theme;
pub mod title_bar;
pub mod window_frame;

pub use bevel::Bevel;
pub use button::Button95;
pub use dialog::{Dialog, DialogResponse};
pub use icon::Icon;
pub use panel::bevel_frame;
pub use status_bar::status_bar;
pub use title_bar::{TitleAction, TitleBar};
pub use window_frame::resize_edges;
