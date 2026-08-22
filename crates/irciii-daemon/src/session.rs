//! The daemon session: one event loop owning the IRC connection, the
//! canonical history, and the state snapshot.
//!
//! The session speaks [`ClientToDaemon`]/[`DaemonToClient`] over tokio mpsc
//! channels, so the same loop serves an in-process client (M3) and the TCP
//! server (M4) — the transport is somebody else's problem.

use irc::proto::{Command, Message, Response};
use irciii_proto::history::{HistoryItem, HistoryKind, WhoEntry, WhoisInfo};
use irciii_proto::wire::{
    ClientCommand, ClientToDaemon, DaemonToClient, SessionSnapshot, TargetInfo, TargetKind,
};
use tokio::sync::mpsc;

use crate::connection::{ConnectParams, Connection};
use crate::translate::translate;

/// Why the session loop returned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionEnd {
    /// The client asked the daemon to exit.
    Exit,
    /// The client channel closed (all clients gone; daemon may keep running).
    ClientGone,
}

/// The daemon session state and event loop.
pub struct Session {
    history: Vec<(u64, HistoryItem)>,
    next_seq: u64,
    snapshot: SessionSnapshot,
    conn: Option<Connection>,
    tx: mpsc::Sender<DaemonToClient>,
    /// LINELEN from ISUPPORT (005), if the server advertises one.
    linelen: Option<usize>,
    /// Byte length of our own `nick!user@host` prefix, once observed.
    our_prefix_len: Option<usize>,
    /// WHOIS reply being assembled from numeric replies, if one is in flight.
    pending_whois: Option<WhoisInfo>,
    /// WHO reply being assembled, if one is in flight: (mask, entries).
    pending_who: Option<(String, Vec<WhoEntry>)>,
    /// MOTD lines being assembled (375/372 until 376/422).
    pending_motd: Option<Vec<String>>,
    /// NAMES burst being assembled: (channel, names), emitted at 366.
    pending_names: Option<(String, Vec<String>)>,
    /// True when the next NAMES reply was asked for with `/names`.
    names_requested: bool,
    /// Parameters of the last (attempted) connection, for auto-reconnect.
    last_params: Option<ConnectParams>,
    /// True when the user asked to disconnect (suppresses auto-reconnect).
    user_disconnected: bool,
    /// Pending auto-reconnect: (attempt number, when to try next).
    reconnect: Option<(usize, tokio::time::Instant)>,
    /// Channels to join once registration completes: the autojoin list of a
    /// `/connect`, or the channels we were in before a reconnect.
    rejoin: Vec<String>,
}

/// Reconnect backoff: immediately, then quick retries for netsplits and
/// server restarts, settling on a patient poll for long outages. The last
/// entry repeats forever.
const RECONNECT_DELAYS_SECS: &[u64] = &[0, 5, 15, 30, 60, 120, 300];

fn reconnect_delay(attempt: usize) -> std::time::Duration {
    let idx = attempt.min(RECONNECT_DELAYS_SECS.len() - 1);
    std::time::Duration::from_secs(RECONNECT_DELAYS_SECS[idx])
}

impl Session {
    /// Create a session that reports to the given client channel.
    pub fn new(tx: mpsc::Sender<DaemonToClient>) -> Self {
        Self {
            history: Vec::new(),
            next_seq: 0,
            snapshot: SessionSnapshot::default(),
            conn: None,
            tx,
            linelen: None,
            our_prefix_len: None,
            pending_whois: None,
            pending_who: None,
            pending_motd: None,
            pending_names: None,
            names_requested: false,
            last_params: None,
            user_disconnected: false,
            reconnect: None,
            rejoin: Vec::new(),
        }
    }

    /// The maximum PRIVMSG/NOTICE text length for a target, derived from the
    /// server-reported line length (ISUPPORT LINELEN, default 512) minus the
    /// relayed line's overhead: `:<prefix> PRIVMSG <target> :<text>\r\n`.
    /// Until we have seen our own prefix, assume the worst-case userhost.
    fn max_text_len(&self, target: &str) -> usize {
        let linelen = self.linelen.unwrap_or(512);
        let prefix = self
            .our_prefix_len
            // nick + "!" + 10-byte user + "@" + 63-byte host
            .unwrap_or(self.snapshot.nick.len() + 1 + 10 + 1 + 63);
        linelen
            .saturating_sub(1 + prefix + 1) // ":<prefix> "
            .saturating_sub("PRIVMSG".len() + 1 + target.len() + 2) // "PRIVMSG <target> :"
            .saturating_sub(2) // "\r\n"
            .max(64)
    }

    /// Full history as `(seq, item)` pairs from a given sequence number.
    pub fn history_from(&self, seq: u64) -> Vec<(u64, HistoryItem)> {
        self.history
            .iter()
            .filter(|(s, _)| *s >= seq)
            .cloned()
            .collect()
    }

    /// The current state snapshot.
    pub fn snapshot(&self) -> &SessionSnapshot {
        &self.snapshot
    }

    async fn send(&self, msg: DaemonToClient) {
        // A gone client is detected via the rx side of the loop; ignore here.
        let _ = self.tx.send(msg).await;
    }

    /// Append an event to the history and push it to the client.
    async fn emit(&mut self, kind: HistoryKind) {
        let item = HistoryItem::now(kind);
        let seq = self.next_seq;
        self.next_seq += 1;
        self.history.push((seq, item.clone()));
        self.send(DaemonToClient::Event { seq, item }).await;
    }

    /// Emit a local `-!-` style status line.
    async fn status(&mut self, text: impl Into<String>) {
        self.emit(HistoryKind::ClientStatus { text: text.into() })
            .await;
    }

    /// Push the current snapshot to the client.
    async fn push_state(&self) {
        self.send(DaemonToClient::State(self.snapshot.clone()))
            .await;
    }

    fn our_nick(&self) -> &str {
        &self.snapshot.nick
    }

    /// Run the session loop until the client asks to exit or disappears.
    pub async fn run(&mut self, mut rx: mpsc::Receiver<ClientToDaemon>) -> SessionEnd {
        loop {
            let end = tokio::select! {
                incoming = rx.recv() => match incoming {
                    None => Some(SessionEnd::ClientGone),
                    Some(msg) => self.handle_client(msg).await,
                },
                message = next_or_pending(self.conn.as_mut()), if self.conn.is_some() => {
                    self.handle_irc(message).await;
                    None
                }
                _ = sleep_until_or_pending(self.reconnect.map(|(_, at)| at)), if self.reconnect.is_some() => {
                    self.try_reconnect().await;
                    None
                }
            };
            if let Some(end) = end {
                return end;
            }
        }
    }

    /// One auto-reconnect attempt; on failure, schedule the next per backoff.
    async fn try_reconnect(&mut self) {
        let Some((attempt, _)) = self.reconnect else {
            return;
        };
        let Some(params) = self.last_params.clone() else {
            self.reconnect = None;
            return;
        };
        self.status(format!(
            "reconnecting to {} (attempt {})",
            params.server,
            attempt + 1
        ))
        .await;
        match Connection::connect(params.clone()).await {
            Ok(conn) => {
                self.conn = Some(conn);
                self.reconnect = None;
                self.snapshot.connected_to = Some(params.server);
                self.push_state().await;
                if self.rejoin.is_empty() {
                    self.status("reconnected").await;
                } else {
                    self.status(format!("reconnected; rejoining {}", self.rejoin.join(" ")))
                        .await;
                }
                // Channels are rejoined once registration completes (001).
            }
            Err(err) => {
                let next = reconnect_delay(attempt + 1);
                self.reconnect = Some((attempt + 1, tokio::time::Instant::now() + next));
                self.status(format!(
                    "reconnect failed: {err}; retrying in {}s",
                    next.as_secs()
                ))
                .await;
            }
        }
    }

    /// Handle one message from the client; `Some` ends the loop.
    async fn handle_client(&mut self, msg: ClientToDaemon) -> Option<SessionEnd> {
        match msg {
            ClientToDaemon::Hello { .. } => {}
            ClientToDaemon::Attach { replay_from_seq } => {
                self.send(DaemonToClient::Snapshot(self.snapshot.clone()))
                    .await;
                let items = self.history_from(replay_from_seq);
                if !items.is_empty() {
                    self.send(DaemonToClient::History { items }).await;
                }
            }
            ClientToDaemon::Command(cmd) => self.handle_command(cmd).await,
            ClientToDaemon::Detach => {
                self.send(DaemonToClient::Bye { exiting: false }).await;
            }
            ClientToDaemon::Exit => {
                if let Some(conn) = &self.conn {
                    let _ = conn
                        .client()
                        .send_quit(concat!("irciii v", env!("CARGO_PKG_VERSION")));
                }
                self.send(DaemonToClient::Bye { exiting: true }).await;
                return Some(SessionEnd::Exit);
            }
        }
        None
    }

    /// Execute a user command.
    async fn handle_command(&mut self, cmd: ClientCommand) {
        match cmd {
            ClientCommand::Connect {
                server,
                port,
                tls,
                nick,
                channels,
            } => {
                let nick = nick.unwrap_or_else(|| {
                    std::env::var("USER").unwrap_or_else(|_| "irciii".to_string())
                });
                self.status(format!("connecting to {server}:{port} (tls: {tls})"))
                    .await;
                let params = ConnectParams {
                    server: server.clone(),
                    port,
                    tls,
                    nick: nick.clone(),
                };
                // A fresh /connect resets any reconnect machinery.
                self.last_params = Some(params.clone());
                self.user_disconnected = false;
                self.reconnect = None;
                self.rejoin = channels;
                match Connection::connect(params).await {
                    Ok(conn) => {
                        self.conn = Some(conn);
                        self.snapshot.connected_to = Some(server);
                        self.snapshot.nick = nick;
                        self.snapshot.targets.clear();
                        self.push_state().await;
                    }
                    Err(err) => {
                        self.emit(HistoryKind::Error {
                            text: format!("connect failed: {err}"),
                        })
                        .await;
                    }
                }
            }
            ClientCommand::Disconnect => {
                // User-initiated: no auto-reconnect.
                self.user_disconnected = true;
                self.reconnect = None;
                self.rejoin.clear();
                if let Some(conn) = self.conn.take() {
                    let _ = conn.client().send_quit("irciii: disconnect");
                    self.snapshot.connected_to = None;
                    self.snapshot.targets.clear();
                    self.snapshot.user_modes.clear();
                    self.push_state().await;
                    self.status("disconnected").await;
                } else {
                    self.status("not connected").await;
                }
            }
            cmd => {
                let Some(conn) = &self.conn else {
                    self.emit(HistoryKind::Error {
                        text: "not connected (use /connect <server> [port])".into(),
                    })
                    .await;
                    return;
                };
                // An owned Sender so command handling can also mutate self
                // (history emission) while sending.
                let client = conn.client().sender();
                let result = match &cmd {
                    ClientCommand::Join { channel, key } => match key {
                        Some(key) => {
                            client.send(Command::JOIN(channel.clone(), Some(key.clone()), None))
                        }
                        None => client.send_join(channel),
                    },
                    ClientCommand::Part { channel, reason } => {
                        client.send(Command::PART(channel.clone(), reason.clone()))
                    }
                    ClientCommand::Nick { nick } => client.send(Command::NICK(nick.clone())),
                    ClientCommand::Msg { target, text } => {
                        // Long lines are split to the server's maximum
                        // message length and sent as separate messages.
                        let mut result = Ok(());
                        for chunk in split_text(text, self.max_text_len(target)) {
                            result = client.send_privmsg(target, &chunk);
                            if result.is_err() {
                                break;
                            }
                            // Servers don't echo our own PRIVMSG (without the
                            // echo-message cap); record it ourselves.
                            let kind = HistoryKind::Privmsg {
                                nick: self.our_nick().to_string(),
                                target: target.clone(),
                                text: chunk,
                            };
                            self.emit(kind).await;
                        }
                        result
                    }
                    ClientCommand::Notice { target, text } => {
                        let mut result = Ok(());
                        for chunk in split_text(text, self.max_text_len(target)) {
                            result = client.send_notice(target, &chunk);
                            if result.is_err() {
                                break;
                            }
                            let kind = HistoryKind::Notice {
                                from: self.our_nick().to_string(),
                                target: target.clone(),
                                text: chunk,
                            };
                            self.emit(kind).await;
                        }
                        result
                    }
                    ClientCommand::Mode { target, modes } => client.send(Command::Raw(
                        "MODE".to_string(),
                        std::iter::once(target.clone())
                            .chain(modes.split_whitespace().map(str::to_string))
                            .collect(),
                    )),
                    ClientCommand::Query { target } => {
                        if !self.snapshot.targets.iter().any(|t| t.name == *target) {
                            self.snapshot.targets.push(TargetInfo {
                                name: target.clone(),
                                kind: TargetKind::Query,
                                modes: String::new(),
                                topic: None,
                                members: Vec::new(),
                            });
                            self.push_state().await;
                        }
                        Ok(())
                    }
                    ClientCommand::Names { channel } => {
                        let channel = channel
                            .clone()
                            .or_else(|| self.snapshot.targets.first().map(|t| t.name.clone()));
                        self.names_requested = true;
                        client.send(Command::NAMES(channel, None))
                    }
                    ClientCommand::Topic { channel, topic } => {
                        client.send(Command::TOPIC(channel.clone(), topic.clone()))
                    }
                    ClientCommand::Whois { nick } => {
                        self.pending_whois = Some(WhoisInfo {
                            nick: nick.clone(),
                            ..WhoisInfo::default()
                        });
                        client.send(Command::WHOIS(None, nick.clone()))
                    }
                    ClientCommand::Who { mask } => {
                        let label = mask.clone().unwrap_or_else(|| "*".to_string());
                        self.pending_who = Some((label, Vec::new()));
                        client.send(Command::WHO(mask.clone(), None))
                    }
                    ClientCommand::Quote { raw } => match raw.parse::<Message>() {
                        Ok(message) => client.send(message),
                        Err(err) => {
                            self.emit(HistoryKind::Error {
                                text: format!("bad raw line: {err}"),
                            })
                            .await;
                            return;
                        }
                    },
                    ClientCommand::Connect { .. } | ClientCommand::Disconnect => {
                        unreachable!("handled above")
                    }
                };
                if let Err(err) = result {
                    self.emit(HistoryKind::Error {
                        text: format!("send failed: {err}"),
                    })
                    .await;
                }
            }
        }
    }

    /// Handle one message (or stream end) from the IRC server.
    async fn handle_irc(&mut self, message: Result<Option<Message>, String>) {
        let message = match message {
            Ok(Some(message)) => message,
            end => {
                self.conn = None;
                self.snapshot.connected_to = None;
                // Remember channels for the rejoin after a reconnect.
                let channels: Vec<String> = self
                    .snapshot
                    .targets
                    .iter()
                    .filter(|t| t.kind == TargetKind::Channel)
                    .map(|t| t.name.clone())
                    .collect();
                self.snapshot.targets.clear();
                self.push_state().await;
                match end {
                    Err(err) => {
                        self.emit(HistoryKind::Error {
                            text: format!("connection error: {err}"),
                        })
                        .await;
                    }
                    _ => self.status("connection closed by server").await,
                }
                // Unexpected disconnect: start the reconnect backoff (first
                // attempt immediately).
                if !self.user_disconnected && self.last_params.is_some() {
                    self.rejoin = channels;
                    self.reconnect = Some((0, tokio::time::Instant::now()));
                }
                return;
            }
        };
        self.track_state(&message).await;
        if self.absorb_whois(&message).await
            || self.absorb_who(&message).await
            || self.absorb_motd(&message).await
            || self.absorb_names(&message).await
        {
            return;
        }
        if let Some(kind) = translate(&message) {
            self.emit(kind).await;
        }
    }

    /// Absorb NAMES numerics (353 burst lines, 366 end) into one aggregated
    /// NAMES card; returns true when the message was consumed.
    async fn absorb_names(&mut self, message: &Message) -> bool {
        let (code, args): (u16, &[String]) = match &message.command {
            Command::Response(response, args) => (*response as u16, args),
            _ => return false,
        };
        match code {
            // RPL_NAMREPLY: <me> <symbol> <channel> :<names>
            353 if args.len() >= 4 => {
                let channel = &args[2];
                // A different channel's burst begins: flush the previous one.
                if let Some((pending, _)) = &self.pending_names
                    && pending != channel
                    && let Some((channel, names)) = self.pending_names.take()
                {
                    self.emit(HistoryKind::NamesReply {
                        channel,
                        names,
                        requested: false,
                    })
                    .await;
                }
                let (_, names) = self
                    .pending_names
                    .get_or_insert_with(|| (channel.clone(), Vec::new()));
                names.extend(args[3].split_whitespace().map(str::to_string));
                true
            }
            // RPL_ENDOFNAMES
            366 => {
                if let Some((channel, names)) = self.pending_names.take() {
                    let requested = std::mem::take(&mut self.names_requested);
                    self.emit(HistoryKind::NamesReply {
                        channel,
                        names,
                        requested,
                    })
                    .await;
                }
                true
            }
            _ => false,
        }
    }

    /// Absorb MOTD numerics (375 start, 372 lines, 376/422 end) into one
    /// MOTD card; returns true when the message was consumed.
    async fn absorb_motd(&mut self, message: &Message) -> bool {
        let (code, args): (u16, &[String]) = match &message.command {
            Command::Response(response, args) => (*response as u16, args),
            _ => return false,
        };
        match code {
            // RPL_MOTDSTART
            375 => {
                self.pending_motd = Some(Vec::new());
                true
            }
            // RPL_MOTD: "<me> :- <text>"
            372 => {
                let Some(lines) = &mut self.pending_motd else {
                    return false;
                };
                let line = args.get(1).map(String::as_str).unwrap_or_default();
                let line = line.strip_prefix("- ").unwrap_or(line);
                lines.push(line.to_string());
                true
            }
            // RPL_ENDOFMOTD / ERR_NOMOTD
            376 | 422 => {
                if let Some(lines) = self.pending_motd.take() {
                    let server = self
                        .snapshot
                        .connected_to
                        .clone()
                        .unwrap_or_else(|| "server".to_string());
                    self.emit(HistoryKind::Motd { server, lines }).await;
                    return true;
                }
                // 422 without a start still ends up as a card-less line.
                code == 376
            }
            _ => false,
        }
    }

    /// Absorb WHO reply numerics (352 rows, 315 end) into the pending WHO
    /// block; returns true when the message was consumed.
    async fn absorb_who(&mut self, message: &Message) -> bool {
        if self.pending_who.is_none() {
            return false;
        }
        let (code, args): (u16, &[String]) = match &message.command {
            Command::Response(response, args) => (*response as u16, args),
            Command::Raw(cmd, args) => match cmd.parse() {
                Ok(code) => (code, args),
                Err(_) => return false,
            },
            _ => return false,
        };
        match code {
            // RPL_WHOREPLY: <me> <channel> <user> <host> <server> <nick> <flags> :<hop> <realname>
            352 if args.len() >= 7 => {
                if let Some((_, entries)) = &mut self.pending_who {
                    let realname = args
                        .get(7)
                        .and_then(|hop_real| hop_real.split_once(' '))
                        .map(|(_hop, realname)| realname.to_string())
                        .unwrap_or_default();
                    entries.push(WhoEntry {
                        nick: args[5].clone(),
                        userhost: format!("{}@{}", args[2], args[3]),
                        flags: args[6].clone(),
                        realname,
                    });
                }
                true
            }
            // RPL_ENDOFWHO
            315 => {
                if let Some((mask, entries)) = self.pending_who.take() {
                    self.emit(HistoryKind::Who { mask, entries }).await;
                }
                true
            }
            _ => false,
        }
    }

    /// Absorb WHOIS numeric replies into the pending [`WhoisInfo`]; returns
    /// true when the message was consumed. Codes per RFC 1459 and the modern
    /// ircdocs: 301 away, 311 user, 312 server, 317 idle, 318 end, 319
    /// channels, 330 account, 401 no-such-nick, plus free-form informational
    /// lines (307, 313, 338, 378, 379, 671, ...).
    async fn absorb_whois(&mut self, message: &Message) -> bool {
        let Some(whois) = &mut self.pending_whois else {
            return false;
        };
        // Known numerics come as Response; ones the irc crate doesn't model
        // (330, 338, 378, 379, 671, ...) come as Raw with a numeric name.
        let (code, args): (u16, &[String]) = match &message.command {
            Command::Response(response, args) => (*response as u16, args),
            Command::Raw(cmd, args) => match cmd.parse() {
                Ok(code) => (code, args),
                Err(_) => return false,
            },
            _ => return false,
        };
        // Whois numerics carry the queried nick as the second argument.
        let about_nick = args
            .get(1)
            .is_some_and(|n| n.eq_ignore_ascii_case(&whois.nick));
        if !about_nick {
            return false;
        }
        let rest = |from: usize| args.get(from..).unwrap_or_default().join(" ");
        match code {
            301 => whois.away = Some(rest(2)),
            311 => {
                if args.len() >= 4 {
                    whois.userhost = Some(format!("{}@{}", args[2], args[3]));
                }
                whois.realname = args.last().cloned();
            }
            312 => whois.server = Some(rest(2)),
            317 => {
                whois.idle_secs = args.get(2).and_then(|s| s.parse().ok());
                whois.signon_ts = args.get(3).and_then(|s| s.parse().ok());
            }
            319 => {
                whois
                    .channels
                    .extend(rest(2).split_whitespace().map(str::to_string));
            }
            330 => whois.account = args.get(2).cloned(),
            401 => whois.error = Some(rest(2)),
            318 => {
                let info = self.pending_whois.take().unwrap_or_default();
                self.emit(HistoryKind::Whois(info)).await;
            }
            // Informational one-liners: operator (313), registered (307),
            // actual host (338), connecting from (378), modes (379),
            // secure connection (671), and anything similarly shaped.
            307 | 313 | 335 | 338 | 378 | 379 | 671 => whois.lines.push(rest(2)),
            _ => return false,
        }
        true
    }

    /// Update the snapshot from a server message; pushes state when it changes.
    async fn track_state(&mut self, message: &Message) {
        let from_us = message.source_nickname() == Some(self.our_nick());
        // Learn our own `nick!user@host` prefix length for message splitting.
        if from_us && let Some(prefix) = &message.prefix {
            self.our_prefix_len = Some(prefix.to_string().len());
        }
        let mut changed = false;
        match &message.command {
            Command::Response(Response::RPL_WELCOME, args) => {
                if let Some(nick) = args.first() {
                    self.snapshot.nick = nick.clone();
                    changed = true;
                }
                // Registration complete: join the autojoin/rejoin channels.
                if !self.rejoin.is_empty()
                    && let Some(conn) = &self.conn
                {
                    for channel in self.rejoin.drain(..) {
                        let _ = conn.client().send_join(channel);
                    }
                }
            }
            // ISUPPORT tokens; LINELEN caps the raw line length.
            Command::Response(Response::RPL_ISUPPORT, args) => {
                for token in args {
                    if let Some(len) = token.strip_prefix("LINELEN=")
                        && let Ok(len) = len.parse()
                    {
                        self.linelen = Some(len);
                    }
                }
            }
            Command::JOIN(channel, ..) => {
                if from_us && !self.snapshot.targets.iter().any(|t| t.name == *channel) {
                    self.snapshot.targets.push(TargetInfo {
                        name: channel.clone(),
                        kind: TargetKind::Channel,
                        modes: String::new(),
                        topic: None,
                        members: Vec::new(),
                    });
                }
                if let (Some(nick), Some(target)) = (
                    message.source_nickname(),
                    self.snapshot
                        .targets
                        .iter_mut()
                        .find(|t| t.name == *channel),
                ) && !target.members.iter().any(|m| m == nick)
                {
                    target.members.push(nick.to_string());
                }
                changed = true;
            }
            Command::PART(channel, _) => {
                if from_us {
                    self.snapshot.targets.retain(|t| t.name != *channel);
                } else if let (Some(nick), Some(target)) = (
                    message.source_nickname(),
                    self.snapshot
                        .targets
                        .iter_mut()
                        .find(|t| t.name == *channel),
                ) {
                    target.members.retain(|m| m != nick);
                }
                changed = true;
            }
            Command::QUIT(_) => {
                if let Some(nick) = message.source_nickname() {
                    for target in &mut self.snapshot.targets {
                        target.members.retain(|m| m != nick);
                    }
                    changed = true;
                }
            }
            Command::KICK(channel, kicked, _) => {
                if kicked.split(',').any(|k| k == self.snapshot.nick) {
                    self.snapshot.targets.retain(|t| t.name != *channel);
                } else if let Some(target) = self
                    .snapshot
                    .targets
                    .iter_mut()
                    .find(|t| t.name == *channel)
                {
                    target
                        .members
                        .retain(|m| !kicked.split(',').any(|k| k == m));
                }
                changed = true;
            }
            Command::NICK(new) => {
                if from_us {
                    self.snapshot.nick = new.clone();
                }
                if let Some(old) = message.source_nickname() {
                    for target in &mut self.snapshot.targets {
                        for member in &mut target.members {
                            if member == old {
                                *member = new.clone();
                            }
                        }
                    }
                }
                changed = true;
            }
            // NAMES replies populate channel membership (prefixes stripped).
            Command::Response(Response::RPL_NAMREPLY, args) if args.len() >= 4 => {
                if let Some(target) = self.snapshot.targets.iter_mut().find(|t| t.name == args[2]) {
                    for name in args[3].split_whitespace() {
                        let nick = name.trim_start_matches(['@', '+', '%', '&', '~']);
                        if !nick.is_empty() && !target.members.iter().any(|m| m == nick) {
                            target.members.push(nick.to_string());
                        }
                    }
                    changed = true;
                }
            }
            Command::UserMODE(target, modes) if *target == self.snapshot.nick => {
                for mode in modes {
                    apply_mode_string(&mut self.snapshot.user_modes, &mode.to_string());
                }
                changed = true;
            }
            Command::ChannelMODE(channel, modes) => {
                if let Some(target) = self
                    .snapshot
                    .targets
                    .iter_mut()
                    .find(|t| t.name == *channel)
                {
                    for mode in modes {
                        apply_mode_string(&mut target.modes, &mode.to_string());
                    }
                    changed = true;
                }
            }
            // RPL_CHANNELMODEIS: "<nick> <channel> <modes> [args]"
            Command::Response(Response::RPL_CHANNELMODEIS, args) if args.len() >= 3 => {
                if let Some(target) = self.snapshot.targets.iter_mut().find(|t| t.name == args[1]) {
                    target.modes = args[2].clone();
                    changed = true;
                }
            }
            // RPL_UMODEIS: "<nick> <modes>"
            Command::Response(Response::RPL_UMODEIS, args) if args.len() >= 2 => {
                self.snapshot.user_modes = args[1].clone();
                changed = true;
            }
            Command::TOPIC(channel, topic) => {
                if let Some(target) = self
                    .snapshot
                    .targets
                    .iter_mut()
                    .find(|t| t.name == *channel)
                {
                    target.topic = topic.clone();
                    changed = true;
                }
            }
            Command::Response(Response::RPL_TOPIC, args) if args.len() >= 3 => {
                if let Some(target) = self.snapshot.targets.iter_mut().find(|t| t.name == args[1]) {
                    target.topic = Some(args[2].clone());
                    changed = true;
                }
            }
            _ => {}
        }
        if changed {
            self.push_state().await;
        }
    }
}

/// Await the next message on a connection; pends forever when disconnected
/// (the select! guard prevents that arm from being polled). Errors are
/// stringified so the session can surface the reason to the user.
async fn next_or_pending(conn: Option<&mut Connection>) -> Result<Option<Message>, String> {
    match conn {
        Some(conn) => conn.next_message().await.map_err(|err| err.to_string()),
        None => std::future::pending().await,
    }
}

/// Sleep until the given instant; pend forever when there is none (the
/// select! guard prevents that arm from being polled).
async fn sleep_until_or_pending(at: Option<tokio::time::Instant>) {
    match at {
        Some(at) => tokio::time::sleep_until(at).await,
        None => std::future::pending().await,
    }
}

/// Split message text into chunks of at most `max_bytes`, breaking at the
/// last space inside the budget when possible, and always on char boundaries.
fn split_text(text: &str, max_bytes: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut rest = text;
    while rest.len() > max_bytes {
        // Largest char boundary within the budget.
        let mut cut = max_bytes;
        while !rest.is_char_boundary(cut) {
            cut -= 1;
        }
        // Prefer the last space in the window so words stay whole — unless
        // the cut already lands on a word boundary.
        let breaks_word = !rest[cut..].starts_with(' ');
        let cut = match rest[..cut].rfind(' ') {
            Some(space) if breaks_word && space > 0 => space,
            _ => cut,
        };
        chunks.push(rest[..cut].trim_end().to_string());
        rest = rest[cut..].trim_start();
    }
    if !rest.is_empty() || chunks.is_empty() {
        chunks.push(rest.to_string());
    }
    chunks
}

/// Apply a mode change like `+o`/`-i` to a mode string like `+Ziw`.
fn apply_mode_string(modes: &mut String, change: &str) {
    let mut chars = change.chars();
    let adding = match chars.next() {
        Some('+') => true,
        Some('-') => false,
        _ => return,
    };
    let mut current: Vec<char> = modes.trim_start_matches('+').chars().collect();
    for c in chars.take_while(|c| !c.is_whitespace()) {
        if adding {
            if !current.contains(&c) {
                current.push(c);
            }
        } else {
            current.retain(|&m| m != c);
        }
    }
    current.sort_unstable();
    *modes = if current.is_empty() {
        String::new()
    } else {
        format!("+{}", current.into_iter().collect::<String>())
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_text_short_passthrough() {
        assert_eq!(split_text("hello", 100), vec!["hello"]);
        assert_eq!(split_text("", 100), vec![""]);
    }

    #[test]
    fn split_text_prefers_word_boundaries() {
        let chunks = split_text("aaa bbb ccc ddd", 7);
        assert_eq!(chunks, vec!["aaa bbb", "ccc ddd"]);
        for chunk in &chunks {
            assert!(chunk.len() <= 7);
        }
    }

    #[test]
    fn split_text_hard_breaks_long_words_on_char_boundaries() {
        let chunks = split_text(&"é".repeat(10), 5); // é = 2 bytes
        assert!(chunks.iter().all(|c| c.len() <= 5 && !c.is_empty()));
        assert_eq!(chunks.concat(), "é".repeat(10));
    }

    #[test]
    fn mode_string_add_remove() {
        let mut modes = String::new();
        apply_mode_string(&mut modes, "+Zi");
        apply_mode_string(&mut modes, "+w");
        assert_eq!(modes, "+Ziw");
        apply_mode_string(&mut modes, "-i");
        assert_eq!(modes, "+Zw");
        apply_mode_string(&mut modes, "-Zw");
        assert_eq!(modes, "");
    }
}
