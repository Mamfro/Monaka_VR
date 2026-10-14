//! What the viewer draws over the eyes, on its own D3D11 device: debug lines in tracking space
//! ([`lines`]), each eye drawn with the pose and field of view it was rendered with ([`eye`]), so
//! the runtime reprojects the drawing together with the image; and the in-headset menus, drawn
//! with egui ([`canvas`]): the debug menu ([`menu`]), the panels beside it ([`panels`], the
//! [`stats`] among them) and Monaka's settings ([`settings`]). The 2D window shows the same
//! pages as windows of its own ([`window`]) over the picture ([`blit`]).

pub mod blit;
pub mod canvas;
pub mod console_view;
pub mod eye;
pub mod fake_vr;
mod gpu;
pub mod lines;
pub mod menu;
pub mod panels;
pub mod quad;
pub mod settings;
pub mod stats;
pub mod views;
pub mod window;

/// The egui the pages are made with, for the viewer's own windows in the 2D window.
pub use egui;

/// What went wrong, for the viewer's log.
pub type Result<T> = std::result::Result<T, String>;
