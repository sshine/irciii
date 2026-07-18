//! The client↔daemon wire protocol.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::history::HistoryItem;

/// Protocol version; both sides send it in their `Hello` and refuse a mismatch.
pub const PROTO_VERSION: u16 = 1;

/// Messages sent from the client (TUI) to the daemon.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClientToDaemon {
    /// First frame: protocol handshake.
    Hello {
        /// The client's [`PROTO_VERSION`].
        proto_version: u16,
    },
    /// Request event replay starting at a sequence number (0 = full history).
    Attach {
        /// First sequence number the client wants replayed.
        replay_from_seq: u64,
    },
    /// A user command to execute.
    Command(ClientCommand),
    /// Detach: close this client connection but keep the daemon running.
    Detach,
    /// Exit: send QUIT to all servers, then terminate the daemon.
    Exit,
}

/// User commands executed by the daemon.
///
/// `/quit` and `/exit` both map to [`ClientToDaemon::Exit`], so there is no
/// `Quit` variant here; `/disconnect` is how you leave a server while keeping
/// the session alive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClientCommand {
    /// Connect to an IRC server.
    Connect {
        /// Server host name.
        server: String,
        /// Server port (e.g. 6697 for TLS).
        port: u16,
        /// Whether to use TLS.
        tls: bool,
        /// Optional nick override.
        nick: Option<String>,
    },
    /// Join a channel.
    Join {
        /// Channel name.
        channel: String,
        /// Optional channel key.
        key: Option<String>,
    },
    /// Part a channel.
    Part {
        /// Channel name.
        channel: String,
        /// Optional part reason.
        reason: Option<String>,
    },
    /// Change nick.
    Nick {
        /// The new nick.
        nick: String,
    },
    /// Send a PRIVMSG.
    Msg {
        /// Channel or nick to message.
        target: String,
        /// Message body.
        text: String,
    },
    /// Send a NOTICE.
    Notice {
        /// Channel or nick to notice.
        target: String,
        /// Notice body.
        text: String,
    },
    /// Set or query modes.
    Mode {
        /// Channel or nick the mode applies to.
        target: String,
        /// Mode string, possibly empty to query.
        modes: String,
    },
    /// Open a query buffer with a nick.
    Query {
        /// The nick to query.
        target: String,
    },
    /// Request NAMES for a channel (or the current one).
    Names {
        /// Channel, or `None` for the active channel.
        channel: Option<String>,
    },
    /// Set or query a channel topic.
    Topic {
        /// Channel name.
        channel: String,
        /// New topic, or `None` to query.
        topic: Option<String>,
    },
    /// WHOIS a nick; the reply is collected into one structured item.
    Whois {
        /// The nick to look up.
        nick: String,
    },
    /// Send a raw IRC line.
    Quote {
        /// The raw line.
        raw: String,
    },
    /// Disconnect from the current server, keeping the session alive.
    Disconnect,
}

/// Messages sent from the daemon to the client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DaemonToClient {
    /// First frame: protocol handshake.
    Hello {
        /// The daemon's [`PROTO_VERSION`].
        proto_version: u16,
        /// The daemon's session id (printed on `/detach`).
        session_id: Uuid,
    },
    /// Full session state snapshot (sent on attach and on state changes).
    Snapshot(SessionSnapshot),
    /// A chunk of history replay.
    History {
        /// `(sequence number, item)` pairs in order.
        items: Vec<(u64, HistoryItem)>,
    },
    /// A single live event.
    Event {
        /// Sequence number of the event.
        seq: u64,
        /// The event itself.
        item: HistoryItem,
    },
    /// Updated session state (nick, modes, targets, topic...).
    State(SessionSnapshot),
    /// The daemon is closing this connection.
    Bye {
        /// True when the daemon itself is exiting (as opposed to a detach or
        /// being displaced by another client).
        exiting: bool,
    },
}

/// Snapshot of the daemon's session state, enough to draw the status bar.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionSnapshot {
    /// Server currently connected to, if any.
    pub connected_to: Option<String>,
    /// Current nick.
    pub nick: String,
    /// Current user modes, e.g. `+Ziw`.
    pub user_modes: String,
    /// Channels and query buffers, in join order.
    pub targets: Vec<TargetInfo>,
}

/// A channel or query buffer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TargetInfo {
    /// Channel name or query nick.
    pub name: String,
    /// Whether this is a channel or a query.
    pub kind: TargetKind,
    /// Channel modes, e.g. `+n` (empty for queries).
    pub modes: String,
    /// Channel topic, if known.
    pub topic: Option<String>,
    /// Nicks present in the channel (without membership prefixes), used for
    /// tab completion.
    pub members: Vec<String>,
}

/// The kind of a target buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TargetKind {
    /// A channel.
    Channel,
    /// A private query with a nick.
    Query,
}
