//! Parsing of the input line into actions.

use irciii_proto::wire::ClientCommand;

/// What an input line means.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputAction {
    /// Send a command to the daemon.
    Command(ClientCommand),
    /// Say text in the active target (input without a leading `/`).
    Say(String),
    /// Detach the client, keep the daemon.
    Detach,
    /// Exit client and daemon (`/exit`, `/quit`).
    Exit(Option<String>),
    /// Show the command overview (`/help`).
    Help,
    /// Local error (bad syntax, unknown command).
    Error(String),
}

/// All slash commands, for tab completion.
pub const COMMANDS: &[&str] = &[
    "connect",
    "join",
    "part",
    "nick",
    "msg",
    "notice",
    "mode",
    "query",
    "names",
    "topic",
    "quote",
    "disconnect",
    "detach",
    "exit",
    "quit",
    "help",
];

/// Complete a partial `/command` to the longest common prefix of all matches;
/// a unique match gets a trailing space. Returns `None` when nothing matches
/// or the input is not a bare command prefix.
pub fn complete_command(input: &str) -> Option<String> {
    let prefix = input.strip_prefix('/')?;
    if prefix.contains(char::is_whitespace) {
        return None;
    }
    let matches: Vec<&str> = COMMANDS
        .iter()
        .filter(|cmd| cmd.starts_with(prefix))
        .copied()
        .collect();
    match matches.as_slice() {
        [] => None,
        [only] => Some(format!("/{only} ")),
        [first, rest @ ..] => {
            let mut lcp = first.len();
            for cmd in rest {
                lcp = first
                    .bytes()
                    .zip(cmd.bytes())
                    .take(lcp)
                    .take_while(|(a, b)| a == b)
                    .count();
            }
            (lcp > prefix.len()).then(|| format!("/{}", &first[..lcp]))
        }
    }
}

/// One-line-per-command help text shown by `/help`.
pub const HELP: &[&str] = &[
    "/connect [-notls] <server> [port] [nick] — connect (TLS by default)",
    "/join <channel> [key]   /part <channel> [reason]   /names [channel]",
    "/msg <target> <text>   /notice <target> <text>   /query <nick>",
    "/nick <nick>   /mode <target> [modes]   /topic <channel> [topic]",
    "/quote <raw line>   /disconnect — leave the server, keep the session",
    "/detach — close the client, keep the daemon (reattach with --attach <id>)",
    "/exit, /quit — quit IRC and terminate daemon and client",
    "^X cycles the active target; PgUp/PgDn scroll the history",
];

fn args2(rest: &str) -> Option<(String, String)> {
    let rest = rest.trim();
    let (a, b) = rest.split_once(char::is_whitespace)?;
    Some((a.to_string(), b.trim().to_string()))
}

/// Parse one submitted input line.
///
/// `/connect [-notls] <server> [port] [nick]` uses TLS by default (port 6697);
/// `-notls` switches the default port to 6667.
pub fn parse(line: &str) -> InputAction {
    let line = line.trim_end();
    if line.is_empty() {
        return InputAction::Error("empty input".into());
    }
    let Some(rest) = line.strip_prefix('/') else {
        return InputAction::Say(line.to_string());
    };
    // "//text" says "/text".
    if rest.starts_with('/') {
        return InputAction::Say(rest.to_string());
    }
    let (cmd, rest) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
    let rest = rest.trim();
    let arg = || rest.to_string();
    let opt = || (!rest.is_empty()).then(|| rest.to_string());

    match cmd.to_ascii_lowercase().as_str() {
        "connect" => {
            let mut words = rest.split_whitespace().peekable();
            let tls = if words.peek() == Some(&"-notls") {
                words.next();
                false
            } else {
                true
            };
            let Some(server) = words.next() else {
                return InputAction::Error(
                    "usage: /connect [-notls] <server> [port] [nick]".into(),
                );
            };
            let default_port = if tls { 6697 } else { 6667 };
            let port = match words.next() {
                Some(port) => match port.parse() {
                    Ok(port) => port,
                    Err(_) => return InputAction::Error(format!("bad port: {port}")),
                },
                None => default_port,
            };
            InputAction::Command(ClientCommand::Connect {
                server: server.to_string(),
                port,
                tls,
                nick: words.next().map(str::to_string),
            })
        }
        "join" => match rest.split_whitespace().collect::<Vec<_>>().as_slice() {
            [channel] => InputAction::Command(ClientCommand::Join {
                channel: channel.to_string(),
                key: None,
            }),
            [channel, key] => InputAction::Command(ClientCommand::Join {
                channel: channel.to_string(),
                key: Some(key.to_string()),
            }),
            _ => InputAction::Error("usage: /join <channel> [key]".into()),
        },
        "part" => match args2(rest) {
            Some((channel, reason)) => InputAction::Command(ClientCommand::Part {
                channel,
                reason: Some(reason),
            }),
            None if !rest.is_empty() => InputAction::Command(ClientCommand::Part {
                channel: arg(),
                reason: None,
            }),
            None => InputAction::Error("usage: /part <channel> [reason]".into()),
        },
        "nick" if !rest.is_empty() => InputAction::Command(ClientCommand::Nick { nick: arg() }),
        "msg" => match args2(rest) {
            Some((target, text)) => InputAction::Command(ClientCommand::Msg { target, text }),
            None => InputAction::Error("usage: /msg <target> <text>".into()),
        },
        "notice" => match args2(rest) {
            Some((target, text)) => InputAction::Command(ClientCommand::Notice { target, text }),
            None => InputAction::Error("usage: /notice <target> <text>".into()),
        },
        "mode" => match args2(rest) {
            Some((target, modes)) => InputAction::Command(ClientCommand::Mode { target, modes }),
            None if !rest.is_empty() => InputAction::Command(ClientCommand::Mode {
                target: arg(),
                modes: String::new(),
            }),
            None => InputAction::Error("usage: /mode <target> [modes]".into()),
        },
        "query" if !rest.is_empty() => InputAction::Command(ClientCommand::Query { target: arg() }),
        "names" => InputAction::Command(ClientCommand::Names { channel: opt() }),
        "topic" => match args2(rest) {
            Some((channel, topic)) => InputAction::Command(ClientCommand::Topic {
                channel,
                topic: Some(topic),
            }),
            None if !rest.is_empty() => InputAction::Command(ClientCommand::Topic {
                channel: arg(),
                topic: None,
            }),
            None => InputAction::Error("usage: /topic <channel> [topic]".into()),
        },
        "quote" if !rest.is_empty() => InputAction::Command(ClientCommand::Quote { raw: arg() }),
        "disconnect" => InputAction::Command(ClientCommand::Disconnect),
        "detach" => InputAction::Detach,
        "exit" | "quit" => InputAction::Exit(opt()),
        "help" => InputAction::Help,
        other => InputAction::Error(format!("unknown command: /{other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn say_and_slash_escape() {
        assert_eq!(parse("hello there"), InputAction::Say("hello there".into()));
        assert_eq!(parse("//joke"), InputAction::Say("/joke".into()));
    }

    #[test]
    fn connect_defaults() {
        assert_eq!(
            parse("/connect irc.libera.chat"),
            InputAction::Command(ClientCommand::Connect {
                server: "irc.libera.chat".into(),
                port: 6697,
                tls: true,
                nick: None,
            })
        );
        assert_eq!(
            parse("/connect -notls irc.example.org"),
            InputAction::Command(ClientCommand::Connect {
                server: "irc.example.org".into(),
                port: 6667,
                tls: false,
                nick: None,
            })
        );
        assert_eq!(
            parse("/connect irc.libera.chat 6697 sshine"),
            InputAction::Command(ClientCommand::Connect {
                server: "irc.libera.chat".into(),
                port: 6697,
                tls: true,
                nick: Some("sshine".into()),
            })
        );
    }

    #[test]
    fn quit_is_exit() {
        assert_eq!(parse("/quit"), InputAction::Exit(None));
        assert_eq!(parse("/exit"), InputAction::Exit(None));
        assert_eq!(
            parse("/quit gone fishing"),
            InputAction::Exit(Some("gone fishing".into()))
        );
    }

    #[test]
    fn msg_requires_target_and_text() {
        assert_eq!(
            parse("/msg n_ko hi there"),
            InputAction::Command(ClientCommand::Msg {
                target: "n_ko".into(),
                text: "hi there".into(),
            })
        );
        assert!(matches!(parse("/msg n_ko"), InputAction::Error(_)));
    }

    #[test]
    fn unknown_command() {
        assert!(matches!(parse("/frobnicate"), InputAction::Error(_)));
    }
}
