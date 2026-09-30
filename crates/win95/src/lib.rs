//! Windows 95 look-and-feel for egui. No business logic lives here.

pub mod bevel;
pub mod button;
pub mod dialog;
pub mod icon;
pub mod list_view;
pub mod panel;
pub mod progress;
pub mod status_bar;
pub mod tabs;
pub mod text_field;
pub mod theme;
pub mod title_bar;
pub mod window_frame;

pub use bevel::Bevel;
pub use button::Button95;
pub use dialog::{Dialog, DialogResponse};
pub use icon::Icon;
pub use list_view::{Cell, Column, ListResponse, ListView};
pub use panel::bevel_frame;
pub use progress::ProgressBar95;
pub use status_bar::status_bar;
pub use tabs::tabs;
pub use text_field::text_field;
pub use title_bar::{TitleAction, TitleBar};
pub use window_frame::resize_edges;
