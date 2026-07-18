//! The hook system.
//!
//! Every IRC event that reaches the client passes through the hook registry.
//! A hook can render into the UI (emit a history line) or send a message back
//! (queue a command to the daemon). Hooks are registered in source code (in
//! [`registry`]) and live entirely inside this hot-reloadable crate: the
//! registry is rebuilt on every dylib reload, so hooks must be stateless or
//! derive their state from what they see.

use irciii_proto::history::{HistoryItem, HistoryKind};
use irciii_proto::wire::{ClientCommand, ClientToDaemon};

/// Context passed to hooks: lets a hook emit UI lines and send commands.
pub struct HookCtx<'a> {
    /// Our current nick (so hooks can detect highlights, self-messages...).
    nick: &'a str,
    /// Lines emitted by hooks, appended to the local history by the caller.
    pub emitted: Vec<HistoryKind>,
    /// Commands queued by hooks, sent to the daemon by the caller.
    pub sent: Vec<ClientToDaemon>,
}

impl<'a> HookCtx<'a> {
    /// Create a context for one event dispatch.
    pub fn new(nick: &'a str) -> Self {
        Self {
            nick,
            emitted: Vec::new(),
            sent: Vec::new(),
        }
    }

    /// Our current nick.
    pub fn nick(&self) -> &str {
        self.nick
    }

    /// Emit a line into the local UI.
    pub fn emit(&mut self, kind: HistoryKind) {
        self.emitted.push(kind);
    }

    /// Send a command back to the daemon (and thus to the IRC server).
    pub fn send(&mut self, cmd: ClientCommand) {
        self.sent.push(ClientToDaemon::Command(cmd));
    }
}

/// A hook: sees every IRC event and may render UI or send messages back.
pub trait Hook: Send {
    /// Name for diagnostics.
    fn name(&self) -> &'static str;
    /// Called for every history event received from the daemon.
    fn on_event(&mut self, event: &HistoryItem, ctx: &mut HookCtx<'_>);
}

/// Build the hook registry. Add hooks here (source-level registration).
pub fn registry() -> Vec<Box<dyn Hook>> {
    vec![Box::new(UrlLogger)]
}

/// Example hook: surfaces URLs seen in messages as a status line.
struct UrlLogger;

impl Hook for UrlLogger {
    fn name(&self) -> &'static str {
        "url-logger"
    }

    fn on_event(&mut self, event: &HistoryItem, ctx: &mut HookCtx<'_>) {
        let (nick, text) = match &event.kind {
            HistoryKind::Privmsg { nick, text, .. } => (nick, text),
            _ => return,
        };
        if nick == ctx.nick() {
            return;
        }
        for word in text.split_whitespace() {
            if word.starts_with("http://") || word.starts_with("https://") {
                ctx.emit(HistoryKind::ClientStatus {
                    text: format!("url from {nick}: {word}"),
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_logger_emits_status_for_urls() {
        let mut hooks = registry();
        let mut ctx = HookCtx::new("sshine");
        let event = HistoryItem::now(HistoryKind::Privmsg {
            nick: "n_ko".into(),
            target: "#diku".into(),
            text: "see https://kampffrosch94.github.io/posts/hotreloading_rust/ for details".into(),
        });
        for hook in &mut hooks {
            hook.on_event(&event, &mut ctx);
        }
        assert_eq!(ctx.emitted.len(), 1);
        assert!(matches!(
            &ctx.emitted[0],
            HistoryKind::ClientStatus { text } if text.contains("url from n_ko")
        ));
    }

    #[test]
    fn url_logger_ignores_own_messages() {
        let mut hooks = registry();
        let mut ctx = HookCtx::new("sshine");
        let event = HistoryItem::now(HistoryKind::Privmsg {
            nick: "sshine".into(),
            target: "#diku".into(),
            text: "https://example.org".into(),
        });
        for hook in &mut hooks {
            hook.on_event(&event, &mut ctx);
        }
        assert!(ctx.emitted.is_empty());
    }
}
