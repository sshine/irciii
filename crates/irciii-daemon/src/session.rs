//! The daemon session: one event loop owning the IRC connection, the
//! canonical history, and the state snapshot.
//!
//! The session speaks [`ClientToDaemon`]/[`DaemonToClient`] over tokio mpsc
//! channels, so the same loop serves an in-process client (M3) and the TCP
//! server (M4) — the transport is somebody else's problem.

use irc::proto::{Command, Message, Response};
use irciii_proto::history::{HistoryItem, HistoryKind};
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
        }
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
            };
            if let Some(end) = end {
                return end;
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
                    let _ = conn.client().send_quit("irciii: exiting");
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
            } => {
                let nick = nick.unwrap_or_else(|| {
                    std::env::var("USER").unwrap_or_else(|_| "irciii".to_string())
                });
                self.status(format!("connecting to {server}:{port} (tls: {tls})"))
                    .await;
                match Connection::connect(ConnectParams {
                    server: server.clone(),
                    port,
                    tls,
                    nick: nick.clone(),
                })
                .await
                {
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
                let client = conn.client();
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
                        let result = client.send_privmsg(target, text);
                        if result.is_ok() {
                            // Servers don't echo our own PRIVMSG (without the
                            // echo-message cap); record it ourselves.
                            let kind = HistoryKind::Privmsg {
                                nick: self.our_nick().to_string(),
                                target: target.clone(),
                                text: text.clone(),
                            };
                            self.emit(kind).await;
                        }
                        result
                    }
                    ClientCommand::Notice { target, text } => {
                        let result = client.send_notice(target, text);
                        if result.is_ok() {
                            let kind = HistoryKind::Notice {
                                from: self.our_nick().to_string(),
                                target: target.clone(),
                                text: text.clone(),
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
                            });
                            self.push_state().await;
                        }
                        Ok(())
                    }
                    ClientCommand::Names { channel } => {
                        let channel = channel
                            .clone()
                            .or_else(|| self.snapshot.targets.first().map(|t| t.name.clone()));
                        client.send(Command::NAMES(channel, None))
                    }
                    ClientCommand::Topic { channel, topic } => {
                        client.send(Command::TOPIC(channel.clone(), topic.clone()))
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
                return;
            }
        };
        self.track_state(&message).await;
        if let Some(kind) = translate(&message) {
            self.emit(kind).await;
        }
    }

    /// Update the snapshot from a server message; pushes state when it changes.
    async fn track_state(&mut self, message: &Message) {
        let from_us = message.source_nickname() == Some(self.our_nick());
        let mut changed = false;
        match &message.command {
            Command::Response(Response::RPL_WELCOME, args) => {
                if let Some(nick) = args.first() {
                    self.snapshot.nick = nick.clone();
                    changed = true;
                }
            }
            Command::JOIN(channel, ..) if from_us => {
                if !self.snapshot.targets.iter().any(|t| t.name == *channel) {
                    self.snapshot.targets.push(TargetInfo {
                        name: channel.clone(),
                        kind: TargetKind::Channel,
                        modes: String::new(),
                        topic: None,
                    });
                    changed = true;
                }
            }
            Command::PART(channel, _) if from_us => {
                self.snapshot.targets.retain(|t| t.name != *channel);
                changed = true;
            }
            Command::NICK(new) if from_us => {
                self.snapshot.nick = new.clone();
                changed = true;
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
