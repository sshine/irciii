//! The hot-reload FFI boundary.
//!
//! These `extern "C"` functions are the only symbols the host loads from the
//! dylib. They take plain-data [`UiState`] by `&mut` — no trait objects cross
//! this boundary, and both sides are always built by the same compiler from
//! the same lockfile (the `hot-reload` feature is dev-only).

use crossterm::event::Event;
use irciii_proto::ui_state::UiState;
use irciii_proto::wire::DaemonToClient;
use ratatui::Frame;

use crate::app;

/// Handle one terminal event.
///
/// # Safety
/// Called across the dylib boundary; `state` and `event` must be valid
/// references from the same build of `irciii-proto`/`crossterm`.
#[unsafe(no_mangle)]
pub extern "C" fn ui_handle_event(state: &mut UiState, event: &Event) {
    app::handle_event(state, event);
}

/// Apply one message from the daemon (runs hooks).
///
/// # Safety
/// See [`ui_handle_event`].
#[unsafe(no_mangle)]
pub extern "C" fn ui_apply_msg(state: &mut UiState, msg: &DaemonToClient) {
    app::apply_msg(state, msg);
}

/// Render one frame.
///
/// # Safety
/// See [`ui_handle_event`]; `frame` must come from the same ratatui build.
#[unsafe(no_mangle)]
pub extern "C" fn ui_render(state: &mut UiState, frame: &mut Frame<'_>) {
    app::render(state, frame);
}
