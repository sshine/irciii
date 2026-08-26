//! Parsing of the input line into actions.

use irciii_proto::ignore::{IgnoreLevel, IgnoreSpec, is_channel, parse_levels, parse_time_ms};
use irciii_proto::wire::ClientCommand;

/// What an input line means.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputAction {
    /// Send a command to the daemon.
    Command(ClientCommand),
    /// Say text in the active target (input without a leading `/`).
    Say(String),
    /// Leave the active channel (`/part` without a channel); the app fills in
    /// the name, and says so when the active target is not a channel.
    PartActive {
        /// Optional part reason.
        reason: Option<String>,
    },
    /// Detach the client, keep the daemon.
    Detach,
    /// Exit client and daemon (`/exit`, `/quit`).
    Exit(Option<String>),
    /// Close the active query (`/query` without a nick); with no query active
    /// it shows what `/query` wants instead.
    CloseQuery,
    /// Show the command overview (`/help`).
    Help,
    /// Show the active ignore rules (bare `/ignore`).
    IgnoreList,
    /// Remove clearable blocks (whois, help, ...) from the log (`/clear`).
    Clear,
    /// Local error (bad syntax, unknown command).
    Error(String),
}

/// All slash commands with a one-line summary, for tab completion, the
/// `/help` block, and the command overlay shown while typing a `/command`.
pub const COMMANDS: &[(&str, &str)] = &[
    (
        "clear",
        "remove cards (whois/who/names/motd/help/ignores) from the log",
    ),
    (
        "connect",
        "[-notls] <server> [port] [nick] [#chan...] — connect (TLS by default)",
    ),
    (
        "detach",
        "close the client, keep the daemon (irciii --attach <id>)",
    ),
    ("disconnect", "leave the server, keep the session"),
    ("exit", "quit IRC and terminate daemon and client"),
    ("help", "show this command overview"),
    (
        "ignore",
        "[-flags] <mask|#channel> [levels] — hide events; bare lists the rules",
    ),
    ("join", "<channel> [key] — join a channel"),
    ("mode", "<target> [modes] — set or query modes"),
    ("msg", "<target> <text> — send a message"),
    ("names", "[channel] — list who is in a channel"),
    ("nick", "<nick> — change nick"),
    ("notice", "<target> <text> — send a notice"),
    (
        "part",
        "[channel] [reason] — leave a channel (default: the active one)",
    ),
    ("query", "<nick> — open a query buffer"),
    ("quit", "same as /exit"),
    ("quote", "<raw line> — send a raw IRC line"),
    ("topic", "<channel> [topic] — set or show the topic"),
    ("unignore", "<id|mask> — drop an ignore rule"),
    (
        "who",
        "[channel|mask] — list users (default: the active channel)",
    ),
    ("whois", "<nick> — look up a user"),
];

/// Commands matching a partial `/prefix` (the overlay's contents). `None`
/// when the input is not a bare `/command` prefix.
pub fn command_matches(input: &str) -> Option<Vec<(&'static str, &'static str)>> {
    let prefix = input.strip_prefix('/')?;
    if prefix.contains(char::is_whitespace) || input.starts_with("//") {
        return None;
    }
    Some(
        COMMANDS
            .iter()
            .filter(|(name, _)| name.starts_with(prefix))
            .copied()
            .collect(),
    )
}

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
        .map(|(name, _)| *name)
        .filter(|name| name.starts_with(prefix))
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

/// Complete the last word of `input` against channel members. At the start
/// of a line a unique match gets `", "` appended, elsewhere a space.
pub fn complete_nick(input: &str, members: &[String]) -> Option<String> {
    let last_line = input.rsplit('\n').next().unwrap_or(input);
    let word_start = last_line
        .rfind(char::is_whitespace)
        .map(|i| i + 1)
        .unwrap_or(0);
    let word = &last_line[word_start..];
    if word.is_empty() || word.starts_with('/') {
        return None;
    }
    let matches: Vec<&str> = members
        .iter()
        .map(String::as_str)
        .filter(|m| m.to_lowercase().starts_with(&word.to_lowercase()))
        .collect();
    let head = &input[..input.len() - word.len()];
    match matches.as_slice() {
        [] => None,
        [only] => {
            let suffix = if word_start == 0 { ", " } else { " " };
            Some(format!("{head}{only}{suffix}"))
        }
        [first, rest @ ..] => {
            let mut lcp = first.len();
            for m in rest {
                lcp = first
                    .bytes()
                    .zip(m.bytes())
                    .take(lcp)
                    .take_while(|(a, b)| a.eq_ignore_ascii_case(b))
                    .count();
            }
            (lcp > word.len()).then(|| format!("{head}{}", &first[..lcp]))
        }
    }
}

/// Whether a word names a level, so `-msgs` reads as a subtraction rather
/// than an unknown flag.
fn is_level_word(word: &str) -> bool {
    word.eq_ignore_ascii_case("all") || IgnoreLevel::parse(word).is_some()
}

/// Parse `/ignore`'s arguments into a rule.
///
/// ```text
/// /ignore [-regexp] [-full] [-except] [-replies] [-pattern <text>]
///         [-channels <list>] [-time <span>] <mask|#channel> [<levels>]
/// ```
///
/// A first argument that names a channel fills the channel list rather than
/// the mask, which is what makes `/ignore #rust joins,parts,quits` mean what
/// it looks like. Omitted levels mean every level, as in irssi.
fn parse_ignore(rest: &str) -> Result<IgnoreSpec, String> {
    let mut spec = IgnoreSpec::default();
    let mut words = rest.split_whitespace();
    let mut positional: Vec<&str> = Vec::new();
    while let Some(word) = words.next() {
        // A value-taking flag consumes the next word.
        let mut value = |flag: &str| {
            words
                .next()
                .map(str::to_string)
                .ok_or_else(|| format!("{flag} needs a value"))
        };
        match word.to_ascii_lowercase().as_str() {
            "-regexp" => spec.regexp = true,
            "-full" => spec.full = true,
            "-except" => spec.except = true,
            "-replies" => spec.replies = true,
            "-pattern" => spec.pattern = Some(value("-pattern")?),
            "-channels" => {
                spec.channels = value("-channels")?
                    .split(',')
                    .filter(|c| !c.is_empty())
                    .map(str::to_string)
                    .collect();
            }
            "-time" => {
                let span = value("-time")?;
                let ms = parse_time_ms(&span)
                    .ok_or_else(|| format!("bad time span: {span} (try 30s, 5m, 2h, 1d)"))?;
                spec.expires_at_ms = Some(chrono::Utc::now().timestamp_millis() + ms);
            }
            // One network per session, so scoping to one is always a no-op.
            "-network" => return Err("-network: a session holds one network".into()),
            // `all -msgs` subtracts a level, so a leading `-` alone does not
            // make a word a flag.
            other if other.starts_with('-') && !is_level_word(&other[1..]) => {
                return Err(format!("unknown flag: {word}"));
            }
            _ => positional.push(word),
        }
    }
    // Levels can be given as several words, so everything after the mask is
    // one level list.
    let (target, levels) = match positional.split_first() {
        Some((target, levels)) => (*target, levels.join(" ")),
        None => return Err("usage: /ignore [-flags] <mask|#channel> [levels]".into()),
    };
    if is_channel(target) {
        spec.channels.push(target.to_string());
    } else if target != "*" {
        spec.mask = Some(target.to_string());
    }
    if !levels.is_empty() {
        spec.levels = parse_levels(&levels).map_err(|bad| format!("unknown level: {bad}"))?;
    }
    if spec.regexp
        && let Some(pattern) = &spec.pattern
        && let Err(err) = regex::Regex::new(pattern)
    {
        return Err(format!("bad regexp: {err}"));
    }
    Ok(spec)
}

fn args2(rest: &str) -> Option<(String, String)> {
    let rest = rest.trim();
    let (a, b) = rest.split_once(char::is_whitespace)?;
    Some((a.to_string(), b.trim().to_string()))
}

/// Parse one submitted input line.
///
/// `/connect [-notls] <server> [port] [nick] [#channel...]` uses TLS by
/// default (port 6697); `-notls` switches the default port to 6667.
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
                    "usage: /connect [-notls] <server> [port] [nick] [#channel...]".into(),
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
            // Nicks cannot start with '#', so channels and the nick override
            // are distinguishable wherever they appear.
            let mut nick = None;
            let mut channels = Vec::new();
            for word in words {
                if word.starts_with('#') {
                    channels.push(word.to_string());
                } else if nick.is_none() {
                    nick = Some(word.to_string());
                }
            }
            InputAction::Command(ClientCommand::Connect {
                server: server.to_string(),
                port,
                tls,
                nick,
                channels,
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
        // A first word that names a channel is the channel; anything else is
        // a reason for leaving the active one, as in irssi.
        "part" => {
            let (first, reason) = match args2(rest) {
                Some((first, reason)) => (first, Some(reason)),
                None => (arg(), None),
            };
            if is_channel(&first) {
                InputAction::Command(ClientCommand::Part {
                    channel: first,
                    reason,
                })
            } else {
                InputAction::PartActive { reason: opt() }
            }
        }
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
        "query" => InputAction::CloseQuery,
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
        "whois" if !rest.is_empty() => InputAction::Command(ClientCommand::Whois { nick: arg() }),
        // Bare /who: the app substitutes the active channel when there is one.
        "who" => InputAction::Command(ClientCommand::Who { mask: opt() }),
        "ignore" if rest.is_empty() => InputAction::IgnoreList,
        "ignore" => match parse_ignore(rest) {
            Ok(spec) => InputAction::Command(ClientCommand::Ignore(spec)),
            Err(err) => InputAction::Error(err),
        },
        "unignore" if !rest.is_empty() => {
            InputAction::Command(ClientCommand::Unignore { target: arg() })
        }
        "unignore" => InputAction::Error("usage: /unignore <id|mask>".into()),
        "detach" => InputAction::Detach,
        "exit" | "quit" => InputAction::Exit(opt()),
        "help" => InputAction::Help,
        "clear" => InputAction::Clear,
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
                channels: Vec::new(),
            })
        );
        assert_eq!(
            parse("/connect -notls irc.example.org"),
            InputAction::Command(ClientCommand::Connect {
                server: "irc.example.org".into(),
                port: 6667,
                tls: false,
                nick: None,
                channels: Vec::new(),
            })
        );
        assert_eq!(
            parse("/connect irc.libera.chat 6697 sshine"),
            InputAction::Command(ClientCommand::Connect {
                server: "irc.libera.chat".into(),
                port: 6697,
                tls: true,
                nick: Some("sshine".into()),
                channels: Vec::new(),
            })
        );
    }

    #[test]
    fn connect_autojoins_channels_in_any_position() {
        assert_eq!(
            parse("/connect irc.libera.chat 6697 #rust #nixos"),
            InputAction::Command(ClientCommand::Connect {
                server: "irc.libera.chat".into(),
                port: 6697,
                tls: true,
                nick: None,
                channels: vec!["#rust".into(), "#nixos".into()],
            })
        );
        assert_eq!(
            parse("/connect irc.libera.chat 6697 #rust sshine"),
            InputAction::Command(ClientCommand::Connect {
                server: "irc.libera.chat".into(),
                port: 6697,
                tls: true,
                nick: Some("sshine".into()),
                channels: vec!["#rust".into()],
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
    fn part_names_a_channel_or_leaves_the_active_one() {
        assert_eq!(
            parse("/part #diku bye"),
            InputAction::Command(ClientCommand::Part {
                channel: "#diku".into(),
                reason: Some("bye".into()),
            })
        );
        assert_eq!(
            parse("/part #diku"),
            InputAction::Command(ClientCommand::Part {
                channel: "#diku".into(),
                reason: None,
            })
        );
        assert_eq!(parse("/part"), InputAction::PartActive { reason: None });
        // Without a channel the whole rest is the reason, not a channel name.
        assert_eq!(
            parse("/part gone fishing"),
            InputAction::PartActive {
                reason: Some("gone fishing".into()),
            }
        );
    }

    #[test]
    fn query_opens_with_a_nick_and_closes_without_one() {
        assert_eq!(
            parse("/query n_ko"),
            InputAction::Command(ClientCommand::Query {
                target: "n_ko".into(),
            })
        );
        assert_eq!(parse("/query"), InputAction::CloseQuery);
    }

    #[test]
    fn unknown_command() {
        assert!(matches!(parse("/frobnicate"), InputAction::Error(_)));
    }

    fn spec_of(line: &str) -> IgnoreSpec {
        match parse(line) {
            InputAction::Command(ClientCommand::Ignore(spec)) => spec,
            other => panic!("expected an ignore rule, got {other:?}"),
        }
    }

    #[test]
    fn ignore_channel_and_levels() {
        assert_eq!(
            spec_of("/ignore #rust joins,parts,quits"),
            IgnoreSpec {
                channels: vec!["#rust".into()],
                levels: parse_levels("joins,parts,quits").expect("levels parse"),
                ..Default::default()
            }
        );
    }

    #[test]
    fn a_channel_first_argument_is_not_a_mask() {
        assert_eq!(spec_of("/ignore #rust joins").mask, None);
        assert_eq!(
            spec_of("/ignore spammer").mask,
            Some("spammer".to_string()),
            "a nick is a mask"
        );
        assert_eq!(spec_of("/ignore * public").mask, None, "* means anyone");
    }

    #[test]
    fn omitted_levels_mean_every_level() {
        assert!(
            spec_of("/ignore spammer").levels.is_empty(),
            "an empty level list is read as all"
        );
    }

    #[test]
    fn levels_may_be_spaced_and_subtracted() {
        let spec = spec_of("/ignore spammer all -msgs");
        assert!(!spec.levels.contains(&IgnoreLevel::Msgs));
        assert!(spec.levels.contains(&IgnoreLevel::Public));
    }

    #[test]
    fn flags_parse_and_take_their_values() {
        let spec = spec_of("/ignore -except -replies -full -pattern spam -channels #a,#b n_ko");
        assert!(spec.except && spec.replies && spec.full);
        assert_eq!(spec.pattern.as_deref(), Some("spam"));
        assert_eq!(spec.channels, vec!["#a".to_string(), "#b".to_string()]);
        assert_eq!(spec.mask.as_deref(), Some("n_ko"));
    }

    #[test]
    fn time_sets_an_expiry_in_the_future() {
        let now = chrono::Utc::now().timestamp_millis();
        let at = spec_of("/ignore -time 5m spammer")
            .expires_at_ms
            .expect("a -time rule expires");
        assert!(at > now, "expiry is ahead of now");
        assert!(at <= now + 300_000, "5m is the upper bound");
        assert!(matches!(
            parse("/ignore -time 5x spammer"),
            InputAction::Error(_)
        ));
    }

    #[test]
    fn bad_input_is_reported_rather_than_silently_inert() {
        // A regexp that cannot compile would otherwise become a rule that
        // never matches, with nothing to say why.
        assert!(matches!(
            parse("/ignore -regexp -pattern ([unclosed spammer"),
            InputAction::Error(_)
        ));
        assert!(matches!(
            parse("/ignore -nosuchflag x"),
            InputAction::Error(_)
        ));
        assert!(matches!(
            parse("/ignore spammer nosuchlevel"),
            InputAction::Error(_)
        ));
        assert!(matches!(parse("/ignore -pattern"), InputAction::Error(_)));
        // One network per session, so the flag can only mislead.
        assert!(matches!(
            parse("/ignore -network libera x"),
            InputAction::Error(_)
        ));
    }

    #[test]
    fn bare_ignore_lists_and_unignore_takes_a_target() {
        assert_eq!(parse("/ignore"), InputAction::IgnoreList);
        assert_eq!(
            parse("/unignore 3"),
            InputAction::Command(ClientCommand::Unignore { target: "3".into() })
        );
        assert!(matches!(parse("/unignore"), InputAction::Error(_)));
    }
}
