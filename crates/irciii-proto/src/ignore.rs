//! Ignore rules, in the shape of irssi's `/ignore`.
//!
//! Rules are plain data: the daemon owns the list and replicates it to clients
//! inside the session snapshot, and the client decides per history item
//! whether to render it. Matching itself lives in the UI crate, which is where
//! the regex dependency belongs; this module holds the data and the parts that
//! are pure text handling (level names, event levels, time spans).

use serde::{Deserialize, Serialize};

use crate::history::HistoryKind;

/// An event class a rule can hide, following irssi's level names.
///
/// irssi's `NO_ACT`, `HIDDEN` and `NOHILIGHT` have no counterpart here: there
/// is no per-window activity tracking to suppress.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum IgnoreLevel {
    /// Channel joins.
    Joins,
    /// Channel parts.
    Parts,
    /// Network-wide quits.
    Quits,
    /// Channel kicks.
    Kicks,
    /// Nick changes.
    Nicks,
    /// Mode changes.
    Modes,
    /// Topic changes.
    Topics,
    /// Messages to a channel.
    Public,
    /// Messages to a query (irssi's MSGS).
    Msgs,
    /// Notices.
    Notices,
    /// CTCP actions (`/me`).
    Actions,
}

/// Every level, in the order [`IgnoreLevel::name`] lists them.
pub const ALL_LEVELS: &[IgnoreLevel] = &[
    IgnoreLevel::Joins,
    IgnoreLevel::Parts,
    IgnoreLevel::Quits,
    IgnoreLevel::Kicks,
    IgnoreLevel::Nicks,
    IgnoreLevel::Modes,
    IgnoreLevel::Topics,
    IgnoreLevel::Public,
    IgnoreLevel::Msgs,
    IgnoreLevel::Notices,
    IgnoreLevel::Actions,
];

impl IgnoreLevel {
    /// The level's canonical lowercase name, as accepted by `/ignore`.
    pub fn name(self) -> &'static str {
        match self {
            Self::Joins => "joins",
            Self::Parts => "parts",
            Self::Quits => "quits",
            Self::Kicks => "kicks",
            Self::Nicks => "nicks",
            Self::Modes => "modes",
            Self::Topics => "topics",
            Self::Public => "public",
            Self::Msgs => "msgs",
            Self::Notices => "notices",
            Self::Actions => "actions",
        }
    }

    /// Parse one level name, case-insensitively. `publics` is accepted as an
    /// alias for `public`, as irssi uses both spellings.
    pub fn parse(word: &str) -> Option<Self> {
        let word = word.to_ascii_lowercase();
        match word.as_str() {
            "publics" => Some(Self::Public),
            _ => ALL_LEVELS.iter().copied().find(|l| l.name() == word),
        }
    }
}

/// Parse a level list: names separated by commas or whitespace, case
/// insensitive, where `all` means every level and a `-` prefix subtracts.
///
/// So `all -msgs` is everything except queries, matching irssi. Returns the
/// offending word on an unknown name.
pub fn parse_levels(input: &str) -> Result<Vec<IgnoreLevel>, String> {
    let mut levels: Vec<IgnoreLevel> = Vec::new();
    for word in input.split([',', ' ', '\t']).filter(|w| !w.is_empty()) {
        let (negated, name) = match word.strip_prefix('-') {
            Some(name) => (true, name),
            None => (false, word),
        };
        let matched: Vec<IgnoreLevel> = if name.eq_ignore_ascii_case("all") {
            ALL_LEVELS.to_vec()
        } else {
            vec![IgnoreLevel::parse(name).ok_or_else(|| name.to_string())?]
        };
        for level in matched {
            if negated {
                levels.retain(|l| *l != level);
            } else if !levels.contains(&level) {
                levels.push(level);
            }
        }
    }
    Ok(levels)
}

/// Parse an irssi `-time` span: a count with an optional unit suffix of
/// `ms`, `s`, `m`, `h` or `d`. A bare number is seconds.
pub fn parse_time_ms(input: &str) -> Option<i64> {
    let input = input.trim();
    let split = input
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(input.len());
    let (count, unit) = input.split_at(split);
    let count: i64 = count.parse().ok()?;
    let per_unit = match unit.trim().to_ascii_lowercase().as_str() {
        "ms" => 1,
        "" | "s" | "sec" | "secs" => 1_000,
        "m" | "min" | "mins" => 60 * 1_000,
        "h" | "hour" | "hours" => 60 * 60 * 1_000,
        "d" | "day" | "days" => 24 * 60 * 60 * 1_000,
        _ => return None,
    };
    count.checked_mul(per_unit)
}

/// The levels an event belongs to.
///
/// A level set, not a single level, because irssi treats a channel action as
/// both `ACTIONS` and `PUBLIC`: `/ignore nick public` hides their actions too.
/// Kinds absent from this mapping are never ignorable, including under `all` —
/// local status and error lines, server lines and the block-rendered cards
/// are ours, not the network's, and hiding them would strand the user.
pub fn levels_of(kind: &HistoryKind) -> Vec<IgnoreLevel> {
    match kind {
        HistoryKind::Privmsg { target, .. } => vec![target_level(target)],
        HistoryKind::Action { target, .. } => vec![IgnoreLevel::Actions, target_level(target)],
        HistoryKind::Notice { .. } => vec![IgnoreLevel::Notices],
        HistoryKind::Join { .. } => vec![IgnoreLevel::Joins],
        HistoryKind::Part { .. } => vec![IgnoreLevel::Parts],
        HistoryKind::Kick { .. } => vec![IgnoreLevel::Kicks],
        HistoryKind::QuitMsg { .. } => vec![IgnoreLevel::Quits],
        HistoryKind::NickChange { .. } => vec![IgnoreLevel::Nicks],
        HistoryKind::Mode { .. } => vec![IgnoreLevel::Modes],
        HistoryKind::Topic { .. } => vec![IgnoreLevel::Topics],
        _ => Vec::new(),
    }
}

/// Whether a target names a channel rather than a nick.
pub fn is_channel(target: &str) -> bool {
    target.starts_with(['#', '&', '+', '!'])
}

fn target_level(target: &str) -> IgnoreLevel {
    if is_channel(target) {
        IgnoreLevel::Public
    } else {
        IgnoreLevel::Msgs
    }
}

/// An ignore rule as the user stated it, before the daemon numbers it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IgnoreSpec {
    /// Nick mask: a bare nick or `nick!user@host`, with `*` and `?` wildcards.
    /// `None` matches anyone.
    pub mask: Option<String>,
    /// Channels the rule is limited to; empty means every channel.
    pub channels: Vec<String>,
    /// Levels the rule hides; empty means every level.
    pub levels: Vec<IgnoreLevel>,
    /// Text pattern the message body must contain (`-pattern`).
    pub pattern: Option<String>,
    /// Treat `pattern` as a regular expression (`-regexp`).
    pub regexp: bool,
    /// Require `pattern` to match a whole word (`-full`).
    pub full: bool,
    /// Invert: matching events are shown even if another rule hides them.
    pub except: bool,
    /// Also hide channel messages addressed to an ignored nick (`-replies`).
    pub replies: bool,
    /// When the rule lapses, in milliseconds since the Unix epoch (`-time`).
    pub expires_at_ms: Option<i64>,
}

/// A numbered ignore rule, as the daemon stores and replicates it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IgnoreRule {
    /// Identifier for `/unignore <id>`, assigned by the daemon.
    pub id: u32,
    /// What the user asked for.
    pub spec: IgnoreSpec,
}

impl IgnoreRule {
    /// Whether the rule has lapsed at `now_ms`.
    pub fn expired(&self, now_ms: i64) -> bool {
        self.spec.expires_at_ms.is_some_and(|at| now_ms >= at)
    }

    /// The rule as an `/ignore` argument list, for the listing card.
    pub fn describe(&self) -> String {
        let spec = &self.spec;
        let mut parts: Vec<String> = Vec::new();
        if spec.regexp {
            parts.push("-regexp".into());
        }
        if spec.full {
            parts.push("-full".into());
        }
        if spec.except {
            parts.push("-except".into());
        }
        if spec.replies {
            parts.push("-replies".into());
        }
        if let Some(pattern) = &spec.pattern {
            parts.push(format!("-pattern {pattern}"));
        }
        if !spec.channels.is_empty() {
            parts.push(format!("-channels {}", spec.channels.join(",")));
        }
        parts.push(spec.mask.clone().unwrap_or_else(|| "*".into()));
        parts.push(if spec.levels.is_empty() {
            "all".into()
        } else {
            spec.levels
                .iter()
                .map(|l| l.name())
                .collect::<Vec<_>>()
                .join(",")
        });
        parts.join(" ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_parse_commas_and_case() {
        assert_eq!(
            parse_levels("joins,parts,quits"),
            Ok(vec![
                IgnoreLevel::Joins,
                IgnoreLevel::Parts,
                IgnoreLevel::Quits
            ])
        );
        assert_eq!(
            parse_levels("JOINS PARTS"),
            Ok(vec![IgnoreLevel::Joins, IgnoreLevel::Parts])
        );
        assert_eq!(parse_levels("nosuchlevel"), Err("nosuchlevel".to_string()));
    }

    #[test]
    fn all_expands_and_minus_subtracts() {
        let all = parse_levels("all").expect("all is a level");
        assert_eq!(all.len(), ALL_LEVELS.len());
        let except_msgs = parse_levels("all -msgs").expect("all -msgs parses");
        assert_eq!(except_msgs.len(), ALL_LEVELS.len() - 1);
        assert!(!except_msgs.contains(&IgnoreLevel::Msgs));
        assert!(except_msgs.contains(&IgnoreLevel::Public));
    }

    #[test]
    fn duplicate_levels_collapse() {
        assert_eq!(parse_levels("joins,joins"), Ok(vec![IgnoreLevel::Joins]));
    }

    #[test]
    fn time_spans_take_unit_suffixes() {
        assert_eq!(parse_time_ms("30"), Some(30_000));
        assert_eq!(parse_time_ms("30s"), Some(30_000));
        assert_eq!(parse_time_ms("500ms"), Some(500));
        assert_eq!(parse_time_ms("5m"), Some(300_000));
        assert_eq!(parse_time_ms("2h"), Some(7_200_000));
        assert_eq!(parse_time_ms("1d"), Some(86_400_000));
        assert_eq!(parse_time_ms("nope"), None);
        assert_eq!(parse_time_ms("10weeks"), None);
    }

    #[test]
    fn channel_messages_are_public_and_queries_are_msgs() {
        let public = HistoryKind::Privmsg {
            nick: "n_ko".into(),
            userhost: None,
            target: "#diku".into(),
            text: "hi".into(),
        };
        let query = HistoryKind::Privmsg {
            nick: "n_ko".into(),
            userhost: None,
            target: "sshine".into(),
            text: "hi".into(),
        };
        assert_eq!(levels_of(&public), vec![IgnoreLevel::Public]);
        assert_eq!(levels_of(&query), vec![IgnoreLevel::Msgs]);
    }

    #[test]
    fn a_channel_action_is_both_actions_and_public() {
        let action = HistoryKind::Action {
            nick: "n_ko".into(),
            userhost: None,
            target: "#diku".into(),
            text: "waves".into(),
        };
        assert_eq!(
            levels_of(&action),
            vec![IgnoreLevel::Actions, IgnoreLevel::Public]
        );
    }

    #[test]
    fn local_lines_and_cards_have_no_level() {
        // `/ignore * all` must never hide these: they are ours, not the
        // network's, and there would be no way to see what went wrong.
        for kind in [
            HistoryKind::ClientStatus { text: "x".into() },
            HistoryKind::Error { text: "x".into() },
            HistoryKind::ServerLine { text: "x".into() },
            HistoryKind::Help,
            HistoryKind::Logo,
        ] {
            assert!(levels_of(&kind).is_empty(), "{kind:?} should have no level");
        }
    }

    #[test]
    fn describe_round_trips_the_target_invocation() {
        let rule = IgnoreRule {
            id: 1,
            spec: IgnoreSpec {
                channels: vec!["#rust".into()],
                levels: parse_levels("joins,parts,quits").expect("levels parse"),
                ..IgnoreSpec::default()
            },
        };
        assert_eq!(rule.describe(), "-channels #rust * joins,parts,quits");
    }
}
