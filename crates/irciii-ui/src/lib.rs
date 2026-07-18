//! The hot-reloadable UI worker for irciii: all rendering (history view,
//! status bar), command/input parsing, and the hook registry. Compiled both as
//! a normal lib (static builds) and a cdylib (dev-time hot reloading).

pub mod api;
pub mod app;
pub mod history_view;
pub mod hooks;
pub mod input;
pub mod render;
pub mod status_bar;
pub mod text_input;
