//! Dispatch into the UI worker: statically linked by default, via a
//! hot-reloaded dylib with the `hot-reload` feature.

use crossterm::event::Event;
use irciii_proto::ui_state::UiState;
use irciii_proto::wire::DaemonToClient;
use ratatui::Frame;

/// The UI worker entry points, resolved statically or dynamically.
pub struct Ui {
    #[cfg(feature = "hot-reload")]
    reloader: crate::reload::Reloader,
}

#[cfg(not(feature = "hot-reload"))]
impl Ui {
    /// Statically linked worker.
    pub fn new() -> Self {
        Self {}
    }

    /// Reload the worker if a newer build exists (no-op when static).
    pub fn maybe_reload(&mut self) -> bool {
        false
    }

    /// Handle one terminal event.
    pub fn handle_event(&mut self, state: &mut UiState, event: &Event) {
        irciii_ui::api::ui_handle_event(state, event);
    }

    /// Apply one daemon message.
    pub fn apply_msg(&mut self, state: &mut UiState, msg: &DaemonToClient) {
        irciii_ui::api::ui_apply_msg(state, msg);
    }

    /// Render one frame.
    pub fn render(&mut self, state: &mut UiState, frame: &mut Frame<'_>) {
        irciii_ui::api::ui_render(state, frame);
    }
}

#[cfg(feature = "hot-reload")]
impl Ui {
    /// Dylib-loaded worker (dev only).
    pub fn new() -> Self {
        Self {
            reloader: crate::reload::Reloader::new(),
        }
    }

    /// Reload the worker if a newer build exists; true if reloaded.
    pub fn maybe_reload(&mut self) -> bool {
        self.reloader.maybe_reload()
    }

    /// Handle one terminal event.
    pub fn handle_event(&mut self, state: &mut UiState, event: &Event) {
        self.reloader.handle_event(state, event);
    }

    /// Apply one daemon message.
    pub fn apply_msg(&mut self, state: &mut UiState, msg: &DaemonToClient) {
        self.reloader.apply_msg(state, msg);
    }

    /// Render one frame.
    pub fn render(&mut self, state: &mut UiState, frame: &mut Frame<'_>) {
        self.reloader.render(state, frame);
    }
}

impl Default for Ui {
    fn default() -> Self {
        Self::new()
    }
}
