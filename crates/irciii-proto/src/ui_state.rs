//! The hot-reload-safe UI state.
//!
//! [`UiState`] is owned by the client host process and passed by `&mut` into
//! the hot-reloadable UI worker. It must stay plain data: no trait objects, no
//! function pointers, no types from the worker crate — anything holding a
//! vtable pointer would dangle across a dylib reload.

use serde::{Deserialize, Serialize};

use crate::history::HistoryItem;
use crate::wire::{ClientToDaemon, SessionSnapshot};

/// How the client should shut down, if at all.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum QuitKind {
    /// Keep running.
    #[default]
    None,
    /// Detach: close the client, leave the daemon running, print the session id.
    Detach,
    /// Exit: terminate daemon and client.
    Exit,
}

/// All state the UI worker operates on. Plain data only (see module docs).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UiState {
    /// Replica of the daemon's history (what the viewport renders from).
    pub history: Vec<HistoryItem>,
    /// Highest daemon sequence number seen (for replay-from on reattach).
    pub last_seq: u64,
    /// Cached rendered heights per history item; `None` = not yet measured.
    /// Invalidated wholesale when `width_gen` changes.
    pub heights: Vec<Option<u16>>,
    /// Generation counter for render options; bumping it invalidates all
    /// cached heights.
    pub opts_gen: u64,
    /// The `opts_gen` the height cache was computed under.
    pub heights_gen: u64,
    /// The viewport width the height cache was computed for.
    pub heights_width: u16,
    /// Scroll offset in display lines from the bottom (0 = pinned to newest).
    pub scroll_offset: usize,
    /// Height of the history viewport at the last render (page size basis).
    pub viewport_height: u16,
    /// Current input line contents.
    pub input: String,
    /// Cursor position in `input`, in bytes (always on a char boundary).
    pub cursor: usize,
    /// Previously submitted input lines (readline-style up/down recall).
    pub input_history: Vec<String>,
    /// Position while browsing `input_history`; `None` = editing a new line.
    pub input_history_pos: Option<usize>,
    /// The in-progress line, stashed while browsing the input history.
    pub input_draft: String,
    /// Replica of the daemon's session snapshot (status bar contents).
    pub snapshot: SessionSnapshot,
    /// Index into the targets list; 0 is the synthetic "(status)" target, so
    /// `active_target == n` means `snapshot.targets[n - 1]` for `n > 0`.
    pub active_target: usize,
    /// Timestamp format for history lines and the status bar clock.
    pub ts_format: String,
    /// Outbound messages queued by the worker, drained to the socket by the host.
    pub pending: Vec<ClientToDaemon>,
    /// Set by the worker when the user asked to detach or exit.
    pub should_quit: QuitKind,
}

impl UiState {
    /// A fresh state with irciii defaults.
    pub fn new() -> Self {
        Self {
            ts_format: "%H:%M:%S".into(),
            ..Self::default()
        }
    }

    /// Name of the active target: `"(status)"` or a channel/query name.
    pub fn active_target_name(&self) -> &str {
        if self.active_target == 0 {
            "(status)"
        } else {
            self.snapshot
                .targets
                .get(self.active_target - 1)
                .map(|t| t.name.as_str())
                .unwrap_or("(status)")
        }
    }

    /// Cycle to the next target (wrapping past the end back to "(status)").
    pub fn cycle_target(&mut self) {
        self.active_target = (self.active_target + 1) % (self.snapshot.targets.len() + 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::{TargetInfo, TargetKind};

    #[test]
    fn cycle_wraps_through_status_and_targets() {
        let mut st = UiState::new();
        assert_eq!(st.active_target_name(), "(status)");
        st.cycle_target();
        assert_eq!(st.active_target, 0, "no targets: stays on (status)");

        st.snapshot.targets.push(TargetInfo {
            name: "#diku".into(),
            kind: TargetKind::Channel,
            modes: "+n".into(),
            topic: None,
        });
        st.cycle_target();
        assert_eq!(st.active_target_name(), "#diku");
        st.cycle_target();
        assert_eq!(st.active_target_name(), "(status)");
    }
}
