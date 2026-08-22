//! Shared plain-data types for irciii: chat history items, the client↔daemon
//! wire protocol (postcard-framed), session discovery files, and the
//! hot-reload-safe UI state.

pub mod framing;
pub mod history;
pub mod ignore;
pub mod session_file;
pub mod ui_state;
pub mod wire;
