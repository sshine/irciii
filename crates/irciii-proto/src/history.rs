//! Structured chat history items.
//!
//! History is stored structured, never pre-rendered: the UI renders only the
//! visible items on demand, so presentation (e.g. the timestamp format) can
//! change without touching the history itself.

use serde::{Deserialize, Serialize};

/// One item in the chat history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryItem {
    /// Milliseconds since the Unix epoch, local rendering decides the format.
    pub ts_ms: i64,
    /// What happened.
    pub kind: HistoryKind,
}

impl HistoryItem {
    /// Create an item stamped with the current wall-clock time.
    pub fn now(kind: HistoryKind) -> Self {
        Self {
            ts_ms: chrono::Utc::now().timestamp_millis(),
            kind,
        }
    }
}

/// The structured event kinds that can appear in the history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum HistoryKind {
    /// `<nick> text` in a channel or query.
    Privmsg {
        /// Sender nick.
        nick: String,
        /// Channel or query target the message was sent to.
        target: String,
        /// Message body.
        text: String,
    },
    /// `* nick text` (CTCP ACTION).
    Action {
        /// Sender nick.
        nick: String,
        /// Channel or query target.
        target: String,
        /// Action body.
        text: String,
    },
    /// A NOTICE.
    Notice {
        /// Sender (nick or server name).
        from: String,
        /// Target of the notice.
        target: String,
        /// Notice body.
        text: String,
    },
    /// Someone joined a channel.
    Join {
        /// Joining nick.
        nick: String,
        /// Channel joined.
        channel: String,
    },
    /// Someone left a channel.
    Part {
        /// Parting nick.
        nick: String,
        /// Channel parted.
        channel: String,
        /// Optional part reason.
        reason: Option<String>,
    },
    /// Someone quit the network.
    QuitMsg {
        /// Quitting nick.
        nick: String,
        /// Optional quit reason.
        reason: Option<String>,
    },
    /// A nick change.
    NickChange {
        /// Previous nick.
        old: String,
        /// New nick.
        new: String,
    },
    /// A mode change.
    Mode {
        /// Who set the mode.
        by: String,
        /// Channel or nick the mode applies to.
        target: String,
        /// The mode string, e.g. `+o nick`.
        modes: String,
    },
    /// A channel topic (change or reply).
    Topic {
        /// Channel the topic belongs to.
        channel: String,
        /// The topic text.
        topic: String,
        /// Who set it, if known.
        by: Option<String>,
    },
    /// A NAMES reply.
    NamesReply {
        /// Channel the names belong to.
        channel: String,
        /// Nicks in the channel.
        names: Vec<String>,
    },
    /// A raw server line (numerics, MOTD, ...).
    ServerLine {
        /// The line text.
        text: String,
    },
    /// A local client status line (irssi's `-!-` style).
    ClientStatus {
        /// The line text.
        text: String,
    },
    /// An error line.
    Error {
        /// The error text.
        text: String,
    },
}
