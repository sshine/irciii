//! Deciding whether an ignore rule hides a history item.
//!
//! The rules themselves are plain data owned by the daemon; this is the half
//! that needs a regex engine, so it lives here rather than in the proto crate.

use irciii_proto::history::{HistoryItem, HistoryKind};
use irciii_proto::ignore::{IgnoreRule, IgnoreSpec, is_channel, levels_of};

/// Whether `item` should be hidden, given the active rules.
///
/// Two passes, as in irssi: an `-except` rule that matches wins over any
/// number of ordinary rules that also match.
pub fn is_ignored(rules: &[IgnoreRule], item: &HistoryItem, own_nick: &str, now_ms: i64) -> bool {
    if rules.is_empty() {
        return false;
    }
    // Never hide what we said ourselves: `/ignore * public` is aimed at the
    // channel, and losing your own lines reads as the client having dropped
    // them.
    if sender(&item.kind).is_some_and(|nick| nick == own_nick) {
        return false;
    }
    let mut matched = false;
    for rule in rules.iter().filter(|r| !r.expired(now_ms)) {
        if !matches(&rule.spec, item, own_nick) {
            continue;
        }
        if rule.spec.except {
            return false;
        }
        matched = true;
    }
    matched
}

/// Whether one rule covers an item, ignoring its `-except` flag.
fn matches(spec: &IgnoreSpec, item: &HistoryItem, own_nick: &str) -> bool {
    let levels = levels_of(&item.kind);
    if levels.is_empty() {
        return false; // local lines and cards are not the network's to hide
    }
    if !spec.levels.is_empty() && !levels.iter().any(|l| spec.levels.contains(l)) {
        return false;
    }
    if !spec.channels.is_empty() && !channels_of(&item.kind).iter().any(|c| in_channels(spec, c)) {
        return false;
    }
    // irssi skips a patterned rule for events with no text, so a `-pattern`
    // rule never hides a join or a quit.
    if let Some(pattern) = &spec.pattern
        && !text_of(&item.kind).is_some_and(|text| pattern_matches(spec, pattern, text))
    {
        return false;
    }
    match &spec.mask {
        None => true,
        Some(mask) => {
            let sender = sender(&item.kind).unwrap_or_default();
            if mask_matches(mask, sender, userhost_of(&item.kind)) {
                return true;
            }
            spec.replies && addressed_to_masked(mask, item, own_nick)
        }
    }
}

/// Whether a rule's channel list covers `channel`, case-insensitively (IRC
/// channel names are).
fn in_channels(spec: &IgnoreSpec, channel: &str) -> bool {
    spec.channels
        .iter()
        .any(|c| c.eq_ignore_ascii_case(channel))
}

/// `-replies`: a channel message opening with `<ignored>:` or `<ignored>,`.
fn addressed_to_masked(mask: &str, item: &HistoryItem, own_nick: &str) -> bool {
    let (Some(text), channels) = (text_of(&item.kind), channels_of(&item.kind)) else {
        return false;
    };
    if channels.is_empty() {
        return false;
    }
    let addressee = text
        .split([':', ','])
        .next()
        .map(str::trim)
        .unwrap_or_default();
    !addressee.is_empty()
        && addressee.len() < text.len()
        && addressee != own_nick
        && mask_matches(mask, addressee, None)
}

/// Match a `nick!user@host` mask, with `*` and `?` wildcards.
///
/// A mask without `!` or `@` matches the nick alone, so `/ignore spammer` does
/// what it looks like. A fuller mask needs a userhost, and an event that
/// carries none cannot match one.
fn mask_matches(mask: &str, nick: &str, userhost: Option<&str>) -> bool {
    if !mask.contains(['!', '@']) {
        return glob(mask, nick);
    }
    match userhost {
        Some(userhost) => glob(mask, &format!("{nick}!{userhost}")),
        None => false,
    }
}

/// Case-insensitive glob with `*` (any run) and `?` (one character).
///
/// Iterative with a backtrack point rather than recursive, so a pathological
/// mask like `*a*a*a*` cannot blow the stack from user input.
fn glob(pattern: &str, text: &str) -> bool {
    let pat: Vec<char> = pattern.to_lowercase().chars().collect();
    let txt: Vec<char> = text.to_lowercase().chars().collect();
    let (mut p, mut t) = (0usize, 0usize);
    // Where to resume if the current `*` turns out to have matched too little.
    let (mut star, mut retry) = (None, 0usize);
    while t < txt.len() {
        match pat.get(p) {
            Some('*') => {
                star = Some(p);
                retry = t;
                p += 1;
            }
            Some('?') => {
                p += 1;
                t += 1;
            }
            Some(c) if *c == txt[t] => {
                p += 1;
                t += 1;
            }
            _ => match star {
                Some(s) => {
                    p = s + 1;
                    retry += 1;
                    t = retry;
                }
                None => return false,
            },
        }
    }
    pat[p..].iter().all(|c| *c == '*')
}

/// Whether `-pattern` matches the message text, honoring `-regexp` and
/// `-full`. A pattern that fails to compile makes the rule inert.
fn pattern_matches(spec: &IgnoreSpec, pattern: &str, text: &str) -> bool {
    if spec.regexp {
        return with_regex(pattern, spec.full, |re| re.is_match(text));
    }
    if spec.full {
        return text
            .split(|c: char| !c.is_alphanumeric())
            .any(|word| word.eq_ignore_ascii_case(pattern));
    }
    text.to_lowercase().contains(&pattern.to_lowercase())
}

/// Run `f` against a compiled pattern, or report no match if it will not
/// compile. Compiled patterns are cached: `render_item` runs per visible line,
/// and this lives inside the worker dylib so a reload discards it.
fn with_regex(pattern: &str, full: bool, f: impl Fn(&regex::Regex) -> bool) -> bool {
    thread_local! {
        static CACHE: std::cell::RefCell<std::collections::HashMap<String, Option<regex::Regex>>> =
            std::cell::RefCell::new(std::collections::HashMap::new());
    }
    let source = if full {
        format!(r"(?i)\b(?:{pattern})\b")
    } else {
        format!("(?i){pattern}")
    };
    CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        let compiled = cache
            .entry(source.clone())
            .or_insert_with(|| regex::Regex::new(&source).ok());
        compiled.as_ref().is_some_and(&f)
    })
}

/// Who an event came from, where that is a nick on the network.
fn sender(kind: &HistoryKind) -> Option<&str> {
    match kind {
        HistoryKind::Privmsg { nick, .. } | HistoryKind::Action { nick, .. } => Some(nick),
        HistoryKind::Notice { from, .. } => Some(from),
        HistoryKind::Join { nick, .. } | HistoryKind::Part { nick, .. } => Some(nick),
        HistoryKind::Kick { by, .. } => Some(by),
        HistoryKind::QuitMsg { nick, .. } => Some(nick),
        HistoryKind::NickChange { old, .. } => Some(old),
        HistoryKind::Mode { by, .. } => Some(by),
        HistoryKind::Topic { by, .. } => by.as_deref(),
        _ => None,
    }
}

fn userhost_of(kind: &HistoryKind) -> Option<&str> {
    match kind {
        HistoryKind::Privmsg { userhost, .. }
        | HistoryKind::Action { userhost, .. }
        | HistoryKind::Notice { userhost, .. }
        | HistoryKind::Join { userhost, .. }
        | HistoryKind::Part { userhost, .. }
        | HistoryKind::Kick { userhost, .. }
        | HistoryKind::QuitMsg { userhost, .. } => userhost.as_deref(),
        _ => None,
    }
}

/// The channels an event belongs to. Quits and nick changes carry a list
/// because they are network wide; everything else has at most one.
fn channels_of(kind: &HistoryKind) -> Vec<&str> {
    match kind {
        HistoryKind::Privmsg { target, .. }
        | HistoryKind::Action { target, .. }
        | HistoryKind::Notice { target, .. }
        | HistoryKind::Mode { target, .. } => {
            if is_channel(target) {
                vec![target]
            } else {
                Vec::new()
            }
        }
        HistoryKind::Join { channel, .. }
        | HistoryKind::Part { channel, .. }
        | HistoryKind::Kick { channel, .. }
        | HistoryKind::Topic { channel, .. } => vec![channel],
        HistoryKind::QuitMsg { channels, .. } | HistoryKind::NickChange { channels, .. } => {
            channels.iter().map(String::as_str).collect()
        }
        _ => Vec::new(),
    }
}

fn text_of(kind: &HistoryKind) -> Option<&str> {
    match kind {
        HistoryKind::Privmsg { text, .. }
        | HistoryKind::Action { text, .. }
        | HistoryKind::Notice { text, .. } => Some(text),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use irciii_proto::ignore::{IgnoreLevel, parse_levels};

    fn rule(id: u32, spec: IgnoreSpec) -> IgnoreRule {
        IgnoreRule { id, spec }
    }

    fn item(kind: HistoryKind) -> HistoryItem {
        HistoryItem {
            ts_ms: 1_700_000_000_000,
            kind,
        }
    }

    fn join(nick: &str, channel: &str) -> HistoryItem {
        item(HistoryKind::Join {
            nick: nick.into(),
            userhost: Some("~u@host".into()),
            channel: channel.into(),
        })
    }

    fn msg(nick: &str, target: &str, text: &str) -> HistoryItem {
        item(HistoryKind::Privmsg {
            nick: nick.into(),
            userhost: Some("~u@spam.example".into()),
            target: target.into(),
            text: text.into(),
        })
    }

    fn hidden(rules: &[IgnoreRule], it: &HistoryItem) -> bool {
        is_ignored(rules, it, "sshine", 1_700_000_000_000)
    }

    #[test]
    fn the_target_invocation_hides_channel_presence_noise() {
        // /ignore #rust joins,parts,quits
        let rules = vec![rule(
            1,
            IgnoreSpec {
                channels: vec!["#rust".into()],
                levels: parse_levels("joins,parts,quits").expect("levels parse"),
                ..Default::default()
            },
        )];
        assert!(hidden(&rules, &join("n_ko", "#rust")));
        assert!(!hidden(&rules, &join("n_ko", "#diku")), "other channel");
        assert!(
            !hidden(&rules, &msg("n_ko", "#rust", "hi")),
            "conversation survives"
        );
    }

    #[test]
    fn quits_are_scoped_by_the_channels_we_shared() {
        let rules = vec![rule(
            1,
            IgnoreSpec {
                channels: vec!["#rust".into()],
                levels: vec![IgnoreLevel::Quits],
                ..Default::default()
            },
        )];
        let quit = |channels: Vec<String>| {
            item(HistoryKind::QuitMsg {
                nick: "n_ko".into(),
                userhost: Some("~n@host".into()),
                channels,
                reason: None,
            })
        };
        assert!(hidden(&rules, &quit(vec!["#rust".into()])));
        assert!(hidden(&rules, &quit(vec!["#diku".into(), "#rust".into()])));
        assert!(!hidden(&rules, &quit(vec!["#diku".into()])));
        assert!(!hidden(&rules, &quit(Vec::new())), "no shared channel");
    }

    #[test]
    fn a_bare_mask_matches_the_nick_and_a_full_mask_needs_the_host() {
        let nick_rule = vec![rule(
            1,
            IgnoreSpec {
                mask: Some("n_*".into()),
                ..Default::default()
            },
        )];
        assert!(hidden(&nick_rule, &msg("n_ko", "#rust", "hi")));
        assert!(!hidden(&nick_rule, &msg("zfnmxt", "#rust", "hi")));

        let host_rule = vec![rule(
            1,
            IgnoreSpec {
                mask: Some("*!*@spam.example".into()),
                ..Default::default()
            },
        )];
        assert!(hidden(&host_rule, &msg("n_ko", "#rust", "hi")));
        // A nick change carries no userhost, so a host mask cannot judge it.
        assert!(!hidden(
            &host_rule,
            &item(HistoryKind::NickChange {
                old: "n_ko".into(),
                new: "n_ko_".into(),
                channels: vec!["#rust".into()],
            })
        ));
    }

    #[test]
    fn empty_levels_and_empty_channels_mean_everything() {
        let rules = vec![rule(
            1,
            IgnoreSpec {
                mask: Some("n_ko".into()),
                ..Default::default()
            },
        )];
        assert!(hidden(&rules, &join("n_ko", "#rust")));
        assert!(hidden(&rules, &msg("n_ko", "#diku", "hi")));
    }

    #[test]
    fn except_overrides_a_matching_ignore() {
        let rules = vec![
            rule(
                1,
                IgnoreSpec {
                    channels: vec!["#rust".into()],
                    levels: vec![IgnoreLevel::Joins],
                    ..Default::default()
                },
            ),
            rule(
                2,
                IgnoreSpec {
                    mask: Some("n_ko".into()),
                    except: true,
                    ..Default::default()
                },
            ),
        ];
        assert!(!hidden(&rules, &join("n_ko", "#rust")), "exception wins");
        assert!(hidden(&rules, &join("zfnmxt", "#rust")));
    }

    #[test]
    fn pattern_never_hides_an_event_without_text() {
        let rules = vec![rule(
            1,
            IgnoreSpec {
                pattern: Some("spam".into()),
                ..Default::default()
            },
        )];
        assert!(hidden(&rules, &msg("n_ko", "#rust", "buy my spam now")));
        assert!(!hidden(&rules, &msg("n_ko", "#rust", "hello there")));
        assert!(
            !hidden(&rules, &join("n_ko", "#rust")),
            "a join has no text"
        );
    }

    #[test]
    fn full_requires_a_whole_word() {
        let spec = |full| {
            vec![rule(
                1,
                IgnoreSpec {
                    pattern: Some("spam".into()),
                    full,
                    ..Default::default()
                },
            )]
        };
        assert!(hidden(&spec(false), &msg("n_ko", "#rust", "spamming")));
        assert!(!hidden(&spec(true), &msg("n_ko", "#rust", "spamming")));
        assert!(hidden(&spec(true), &msg("n_ko", "#rust", "pure spam here")));
    }

    #[test]
    fn regexp_patterns_apply_and_bad_ones_are_inert() {
        let good = vec![rule(
            1,
            IgnoreSpec {
                pattern: Some(r"^\d+ free".into()),
                regexp: true,
                ..Default::default()
            },
        )];
        assert!(hidden(&good, &msg("n_ko", "#rust", "500 free coins")));
        assert!(!hidden(&good, &msg("n_ko", "#rust", "no free coins")));

        let broken = vec![rule(
            1,
            IgnoreSpec {
                pattern: Some("([unclosed".into()),
                regexp: true,
                ..Default::default()
            },
        )];
        assert!(!hidden(&broken, &msg("n_ko", "#rust", "([unclosed")));
    }

    #[test]
    fn replies_hides_messages_addressed_to_an_ignored_nick() {
        let rules = vec![rule(
            1,
            IgnoreSpec {
                mask: Some("spammer".into()),
                replies: true,
                ..Default::default()
            },
        )];
        assert!(hidden(&rules, &msg("spammer", "#rust", "buy coins")));
        assert!(hidden(&rules, &msg("n_ko", "#rust", "spammer: go away")));
        assert!(hidden(&rules, &msg("n_ko", "#rust", "spammer, go away")));
        assert!(!hidden(&rules, &msg("n_ko", "#rust", "unrelated chatter")));
    }

    #[test]
    fn our_own_lines_are_never_hidden() {
        let rules = vec![rule(1, IgnoreSpec::default())];
        assert!(hidden(&rules, &msg("n_ko", "#rust", "hi")));
        assert!(
            !hidden(&rules, &msg("sshine", "#rust", "hi")),
            "/ignore * all must not silence us"
        );
    }

    #[test]
    fn local_lines_and_cards_survive_ignore_everything() {
        let rules = vec![rule(1, IgnoreSpec::default())];
        for kind in [
            HistoryKind::ClientStatus {
                text: "connecting".into(),
            },
            HistoryKind::Error {
                text: "connect failed".into(),
            },
            HistoryKind::Help,
            HistoryKind::Logo,
        ] {
            assert!(!hidden(&rules, &item(kind)));
        }
    }

    #[test]
    fn expired_rules_stop_hiding() {
        let rules = vec![rule(
            1,
            IgnoreSpec {
                mask: Some("n_ko".into()),
                expires_at_ms: Some(1_700_000_000_000),
                ..Default::default()
            },
        )];
        let it = msg("n_ko", "#rust", "hi");
        assert!(is_ignored(&rules, &it, "sshine", 1_699_999_999_999));
        assert!(!is_ignored(&rules, &it, "sshine", 1_700_000_000_000));
    }

    #[test]
    fn glob_handles_wildcards_and_backtracking() {
        assert!(glob("*", "anything"));
        assert!(glob("n_?o", "n_ko"));
        assert!(glob("*@spam.example", "n_ko!~u@spam.example"));
        assert!(glob("*a*a*a*", "banana"));
        assert!(!glob("*a*a*a*a*a*", "banana"));
        assert!(glob("N_KO", "n_ko"), "masks are case insensitive");
        assert!(!glob("n_ko", "n_kos"));
    }
}
