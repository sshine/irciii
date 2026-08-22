//! Translation of incoming [`irc::proto::Message`]s into structured
//! [`HistoryKind`] items.

use irc::proto::{Command, Message, Response};
use irciii_proto::history::HistoryKind;

/// Nickname of a message's sender, falling back to the server prefix or "?".
fn source(message: &Message) -> String {
    message
        .source_nickname()
        .map(str::to_string)
        .or_else(|| message.prefix.as_ref().map(|p| p.to_string()))
        .unwrap_or_else(|| "?".to_string())
}

/// The `user@host` part of the sender prefix, if present.
fn userhost(message: &Message) -> Option<String> {
    message
        .prefix
        .as_ref()
        .map(|p| p.to_string())
        .and_then(|p| p.split_once('!').map(|(_, uh)| uh.to_string()))
}

/// Translate an incoming IRC message into a history item kind.
///
/// Returns `None` for protocol chatter that should not appear in the history
/// (PING/PONG, capability negotiation).
pub fn translate(message: &Message) -> Option<HistoryKind> {
    let kind = match &message.command {
        Command::PING(..) | Command::PONG(..) | Command::CAP(..) => return None,
        Command::PRIVMSG(target, text) => match text
            .strip_prefix("\u{1}ACTION ")
            .and_then(|rest| rest.strip_suffix('\u{1}'))
        {
            Some(action) => HistoryKind::Action {
                nick: source(message),
                userhost: userhost(message),
                target: target.clone(),
                text: action.to_string(),
            },
            None => HistoryKind::Privmsg {
                nick: source(message),
                userhost: userhost(message),
                target: target.clone(),
                text: text.clone(),
            },
        },
        Command::NOTICE(target, text) => HistoryKind::Notice {
            from: source(message),
            userhost: userhost(message),
            target: target.clone(),
            text: text.clone(),
        },
        Command::JOIN(channel, ..) => HistoryKind::Join {
            nick: source(message),
            userhost: userhost(message),
            channel: channel.clone(),
        },
        Command::PART(channel, reason) => HistoryKind::Part {
            nick: source(message),
            userhost: userhost(message),
            channel: channel.clone(),
            reason: reason.clone(),
        },
        Command::KICK(channel, kicked, reason) => HistoryKind::Kick {
            by: source(message),
            userhost: userhost(message),
            channel: channel.clone(),
            kicked: kicked.clone(),
            reason: reason.clone(),
        },
        // `channels` needs the membership the session tracks, which this
        // function does not see; the session fills it in.
        Command::QUIT(reason) => HistoryKind::QuitMsg {
            nick: source(message),
            userhost: userhost(message),
            channels: Vec::new(),
            reason: reason.clone(),
        },
        Command::NICK(new) => HistoryKind::NickChange {
            old: source(message),
            new: new.clone(),
            channels: Vec::new(),
        },
        Command::ChannelMODE(target, modes) => HistoryKind::Mode {
            by: source(message),
            target: target.clone(),
            modes: modes
                .iter()
                .map(|m| m.to_string())
                .collect::<Vec<_>>()
                .join(" "),
        },
        Command::UserMODE(target, modes) => HistoryKind::Mode {
            by: source(message),
            target: target.clone(),
            modes: modes
                .iter()
                .map(|m| m.to_string())
                .collect::<Vec<_>>()
                .join(" "),
        },
        Command::TOPIC(channel, topic) => HistoryKind::Topic {
            channel: channel.clone(),
            topic: topic.clone().unwrap_or_default(),
            by: Some(source(message)),
        },
        Command::Response(Response::RPL_TOPIC, args) if args.len() >= 3 => HistoryKind::Topic {
            channel: args[1].clone(),
            topic: args[2].clone(),
            by: None,
        },
        // The session aggregates NAMES bursts before translation; this arm
        // only fires for stray 353s outside a burst.
        Command::Response(Response::RPL_NAMREPLY, args) if args.len() >= 4 => {
            HistoryKind::NamesReply {
                channel: args[2].clone(),
                names: args[3].split_whitespace().map(str::to_string).collect(),
                requested: false,
            }
        }
        Command::Response(Response::RPL_ENDOFNAMES, _) => return None,
        Command::Response(_, args) => HistoryKind::ServerLine {
            // Drop our own nick (args[0]) from numeric replies; join the rest.
            text: args.iter().skip(1).cloned().collect::<Vec<_>>().join(" "),
        },
        Command::ERROR(text) => HistoryKind::Error { text: text.clone() },
        other => HistoryKind::ServerLine {
            text: Message::from(other.clone())
                .to_string()
                .trim_end()
                .to_string(),
        },
    };
    Some(kind)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn msg(line: &str) -> Message {
        Message::from_str(line).expect("parse irc line")
    }

    #[test]
    fn privmsg() {
        let kind = translate(&msg(
            ":zfnmxt!~z@host PRIVMSG #diku :I have dual citizenship.",
        ));
        assert_eq!(
            kind,
            Some(HistoryKind::Privmsg {
                nick: "zfnmxt".into(),
                userhost: Some("~z@host".into()),
                target: "#diku".into(),
                text: "I have dual citizenship.".into(),
            })
        );
    }

    #[test]
    fn action() {
        let kind = translate(&msg(
            ":sshine!~s@host PRIVMSG #diku :\u{1}ACTION imagines a conversation\u{1}",
        ));
        assert_eq!(
            kind,
            Some(HistoryKind::Action {
                nick: "sshine".into(),
                userhost: Some("~s@host".into()),
                target: "#diku".into(),
                text: "imagines a conversation".into(),
            })
        );
    }

    #[test]
    fn join_part_quit_carry_the_userhost() {
        assert_eq!(
            translate(&msg(":n_ko!~n@host JOIN #diku")),
            Some(HistoryKind::Join {
                nick: "n_ko".into(),
                userhost: Some("~n@host".into()),
                channel: "#diku".into()
            })
        );
        assert_eq!(
            translate(&msg(":n_ko!~n@host PART #diku :bye")),
            Some(HistoryKind::Part {
                nick: "n_ko".into(),
                userhost: Some("~n@host".into()),
                channel: "#diku".into(),
                reason: Some("bye".into())
            })
        );
        assert_eq!(
            translate(&msg(":n_ko!~n@host QUIT :leaving")),
            Some(HistoryKind::QuitMsg {
                nick: "n_ko".into(),
                userhost: Some("~n@host".into()),
                channels: Vec::new(),
                reason: Some("leaving".into())
            })
        );
    }

    #[test]
    fn kick_names_both_sides() {
        assert_eq!(
            translate(&msg(":op!~o@host KICK #diku n_ko :behave")),
            Some(HistoryKind::Kick {
                by: "op".into(),
                userhost: Some("~o@host".into()),
                channel: "#diku".into(),
                kicked: "n_ko".into(),
                reason: Some("behave".into())
            })
        );
    }

    #[test]
    fn ping_is_dropped() {
        assert_eq!(translate(&msg("PING :irc.libera.chat")), None);
    }

    #[test]
    fn names_reply() {
        let kind = translate(&msg(
            ":server 353 me = #diku :sshine zfnmxt n_ko @concieggs",
        ));
        assert_eq!(
            kind,
            Some(HistoryKind::NamesReply {
                channel: "#diku".into(),
                names: vec![
                    "sshine".into(),
                    "zfnmxt".into(),
                    "n_ko".into(),
                    "@concieggs".into()
                ],
                requested: false,
            })
        );
    }
}
