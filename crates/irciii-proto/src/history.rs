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
        /// The `user@host` part of the sender prefix, if present.
        userhost: Option<String>,
        /// Channel joined.
        channel: String,
    },
    /// Someone left a channel.
    Part {
        /// Parting nick.
        nick: String,
        /// The `user@host` part of the sender prefix, if present.
        userhost: Option<String>,
        /// Channel parted.
        channel: String,
        /// Optional part reason.
        reason: Option<String>,
    },
    /// Someone quit the network.
    QuitMsg {
        /// Quitting nick.
        nick: String,
        /// The `user@host` part of the sender prefix, if present.
        userhost: Option<String>,
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
    /// A NAMES reply (aggregated across the 353 burst, emitted at 366).
    NamesReply {
        /// Channel the names belong to.
        channel: String,
        /// Nicks in the channel.
        names: Vec<String>,
        /// Whether the user asked (`/names`) as opposed to a join burst;
        /// oversized requested lists open a modal in the client.
        requested: bool,
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
    /// A collected WHOIS reply, rendered as one block.
    Whois(WhoisInfo),
    /// A collected WHO reply, rendered as one block.
    Who {
        /// The mask the WHO was about (channel or pattern).
        mask: String,
        /// One entry per matched user.
        entries: Vec<WhoEntry>,
    },
    /// The server MOTD, rendered as one card.
    Motd {
        /// Server the MOTD came from.
        server: String,
        /// The MOTD lines.
        lines: Vec<String>,
    },
    /// The `/help` command overview, rendered as one block.
    Help,
}

impl HistoryKind {
    /// Whether this item is a *card*: a structured, block-rendered reply
    /// (no timestamp gutter, bordered) that `/clear` removes from the log.
    /// Cards: whois, who, names, motd, help.
    pub fn is_card(&self) -> bool {
        matches!(
            self,
            HistoryKind::Whois(_)
                | HistoryKind::Who { .. }
                | HistoryKind::Help
                | HistoryKind::NamesReply { .. }
                | HistoryKind::Motd { .. }
        )
    }
}

/// One row of a WHO reply (RPL_WHOREPLY, 352).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WhoEntry {
    /// Nickname.
    pub nick: String,
    /// `user@host`.
    pub userhost: String,
    /// Flags: H/G (here/gone), * (oper), @/+ (channel status).
    pub flags: String,
    /// Realname (gecos).
    pub realname: String,
}

/// A WHOIS reply assembled from its numeric replies (RFC 1459 / modern
/// ircdocs: 301, 307, 311, 312, 313, 317, 318, 319, 330, 338, 378, 379, 671).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WhoisInfo {
    /// The nick the WHOIS was about.
    pub nick: String,
    /// `user@host` from RPL_WHOISUSER (311).
    pub userhost: Option<String>,
    /// Realname from RPL_WHOISUSER (311).
    pub realname: Option<String>,
    /// Server and its info string from RPL_WHOISSERVER (312).
    pub server: Option<String>,
    /// Services account from RPL_WHOISACCOUNT (330).
    pub account: Option<String>,
    /// Channels from RPL_WHOISCHANNELS (319), with membership prefixes.
    pub channels: Vec<String>,
    /// Seconds idle from RPL_WHOISIDLE (317).
    pub idle_secs: Option<u64>,
    /// Signon unix time from RPL_WHOISIDLE (317).
    pub signon_ts: Option<i64>,
    /// Away message from RPL_AWAY (301).
    pub away: Option<String>,
    /// Free-form extra lines (313 operator, 307 registered, 338 actual host,
    /// 378 connecting from, 379 modes, 671 secure connection, ...).
    pub lines: Vec<String>,
    /// Error instead of a reply (e.g. 401 no such nick).
    pub error: Option<String>,
}
