//! Rendering of a single [`HistoryItem`] into styled, wrapped lines.
//!
//! This is where the "reactive history" lives: items are structured data and
//! every visible line is rendered on demand, so changing e.g. the timestamp
//! format re-renders without touching the history.

use chrono::{DateTime, Local};
use irciii_proto::history::{HistoryItem, HistoryKind};
use promptui_core::widgets::word_wrap::wrap_line_strs;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

/// Options that affect how history items are rendered.
#[derive(Debug, Clone)]
pub struct RenderOpts {
    /// strftime format for line timestamps (matches the status bar clock).
    pub ts_format: String,
    /// Our own nick, rendered bold to stand out.
    pub own_nick: String,
}

impl Default for RenderOpts {
    fn default() -> Self {
        Self {
            ts_format: "%H:%M:%S".into(),
            own_nick: String::new(),
        }
    }
}

impl RenderOpts {
    /// Options derived from the current UI state.
    pub fn from_state(state: &irciii_proto::ui_state::UiState) -> Self {
        Self {
            ts_format: state.ts_format.clone(),
            own_nick: state.snapshot.nick.clone(),
        }
    }
}

/// Continuation lines indent by two columns (under the first colon of the
/// timestamp) instead of a full hanging indent, to not waste width.
const CONT_INDENT: usize = 2;

/// Nick style: default color; only our own nick is bold.
fn nick_style(nick: &str, opts: &RenderOpts) -> Style {
    if nick == opts.own_nick && !nick.is_empty() {
        Style::default().add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    }
}

fn bold() -> Style {
    Style::default().add_modifier(Modifier::BOLD)
}

fn ts_span(item: &HistoryItem, opts: &RenderOpts) -> String {
    DateTime::from_timestamp_millis(item.ts_ms)
        .map(|utc| {
            utc.with_timezone(&Local)
                .format(&opts.ts_format)
                .to_string()
        })
        .unwrap_or_else(|| "??:??:??".to_string())
}

fn dim() -> Style {
    Style::default().add_modifier(Modifier::DIM)
}

/// Render one history item into wrapped lines. Continuation lines always
/// indent by [`CONT_INDENT`] columns.
pub fn render_item(item: &HistoryItem, width: u16, opts: &RenderOpts) -> Vec<Line<'static>> {
    let ts = ts_span(item, opts);
    // Block items render on their own, without the gutter machinery.
    match &item.kind {
        HistoryKind::Whois(info) => return render_whois(&ts, info, width),
        HistoryKind::Help => return render_help(&ts),
        _ => {}
    }
    let (gutter_spans, text, text_style) = match &item.kind {
        HistoryKind::Privmsg { nick, text, .. } => (
            vec![
                Span::styled(format!("{ts} "), dim()),
                Span::styled("<", bold()),
                Span::styled(nick.clone(), nick_style(nick, opts)),
                Span::styled(">", bold()),
                Span::raw(" "),
            ],
            text.clone(),
            Style::default(),
        ),
        HistoryKind::Action { nick, text, .. } => (
            vec![
                Span::styled(format!("{ts} "), dim()),
                Span::styled(" * ", bold()),
                Span::styled(format!("{nick} "), nick_style(nick, opts)),
            ],
            text.clone(),
            Style::default(),
        ),
        HistoryKind::Notice { from, text, .. } => (
            vec![
                Span::styled(format!("{ts} "), dim()),
                Span::styled(format!("-{from}- "), Style::default().fg(Color::LightRed)),
            ],
            text.clone(),
            Style::default(),
        ),
        HistoryKind::Join { nick, channel } => {
            status_line(&ts, format!("{nick} has joined {channel}"), Color::Green)
        }
        HistoryKind::Part {
            nick,
            channel,
            reason,
        } => status_line(
            &ts,
            match reason {
                Some(reason) => format!("{nick} has left {channel} ({reason})"),
                None => format!("{nick} has left {channel}"),
            },
            Color::Yellow,
        ),
        HistoryKind::QuitMsg { nick, reason } => status_line(
            &ts,
            match reason {
                Some(reason) => format!("{nick} has quit ({reason})"),
                None => format!("{nick} has quit"),
            },
            Color::Yellow,
        ),
        HistoryKind::NickChange { old, new } => {
            status_line(&ts, format!("{old} is now known as {new}"), Color::Cyan)
        }
        HistoryKind::Mode { by, target, modes } => {
            status_line(&ts, format!("mode/{target} [{modes}] by {by}"), Color::Cyan)
        }
        HistoryKind::Topic { channel, topic, by } => status_line(
            &ts,
            match by {
                Some(by) => format!("{by} changed the topic of {channel} to: {topic}"),
                None => format!("topic for {channel}: {topic}"),
            },
            Color::Cyan,
        ),
        HistoryKind::NamesReply { channel, names } => status_line(
            &ts,
            format!("users in {channel}: {}", names.join(" ")),
            Color::Cyan,
        ),
        HistoryKind::ServerLine { text } => (
            vec![Span::styled(format!("{ts} "), dim())],
            text.clone(),
            dim(),
        ),
        HistoryKind::ClientStatus { text } => status_line(&ts, text.clone(), Color::Cyan),
        HistoryKind::Error { text } => status_line(&ts, text.clone(), Color::Red),
        HistoryKind::Whois(_) | HistoryKind::Help => unreachable!("handled above"),
    };

    let gutter_width: usize = gutter_spans.iter().map(|s| s.content.width()).sum();
    let first_width = (width as usize).saturating_sub(gutter_width).max(8) as u16;
    let cont_width = (width as usize).saturating_sub(CONT_INDENT).max(8) as u16;
    let indent = " ".repeat(CONT_INDENT);

    let mut lines = Vec::new();
    for (i, part) in wrap_two(&text, first_width, cont_width)
        .into_iter()
        .enumerate()
    {
        if i == 0 {
            let mut spans = gutter_spans.clone();
            spans.push(Span::styled(part, text_style));
            lines.push(Line::from(spans));
        } else {
            lines.push(Line::from(vec![
                Span::raw(indent.clone()),
                Span::styled(part, text_style),
            ]));
        }
    }
    if lines.is_empty() {
        lines.push(Line::from(gutter_spans));
    }
    lines
}

/// Wrap `text` with different widths for the first line (next to the gutter)
/// and continuation lines (indented by [`CONT_INDENT`]).
fn wrap_two(text: &str, first_width: u16, cont_width: u16) -> Vec<String> {
    let ranges = promptui_core::widgets::word_wrap::wrap_lines(text, first_width);
    let Some(&(start, end)) = ranges.first() else {
        return vec![String::new()];
    };
    let mut out = vec![text[start..end].trim_end().to_string()];
    if end < text.len() {
        let remainder = text[end..].trim_start();
        out.extend(
            wrap_line_strs(remainder, cont_width)
                .into_iter()
                .map(|part| part.trim_end().to_string()),
        );
    }
    out
}

/// Render a WHOIS reply block: a header plus indented detail lines.
fn render_whois(
    ts: &str,
    info: &irciii_proto::history::WhoisInfo,
    width: u16,
) -> Vec<Line<'static>> {
    let label = Style::default().fg(Color::Cyan);
    let mut lines = vec![Line::from(vec![
        Span::styled(format!("{ts} "), dim()),
        Span::styled("┌ whois ", label),
        Span::styled(info.nick.clone(), bold()),
        Span::styled(
            info.userhost
                .as_ref()
                .map(|uh| format!(" ({uh})"))
                .unwrap_or_default(),
            dim(),
        ),
    ])];
    let mut detail = |name: &str, value: String| {
        let text = format!("{name}: {value}");
        let cont = (width as usize).saturating_sub(CONT_INDENT + 2).max(8) as u16;
        for (i, part) in wrap_two(&text, cont, cont).into_iter().enumerate() {
            lines.push(Line::from(vec![
                Span::raw(" ".repeat(CONT_INDENT)),
                Span::styled(if i == 0 { "│ " } else { "│   " }.to_string(), label),
                Span::raw(part),
            ]));
        }
    };
    if let Some(error) = &info.error {
        detail("error", error.clone());
    }
    if let Some(realname) = &info.realname {
        detail("realname", realname.clone());
    }
    if let Some(server) = &info.server {
        detail("server", server.clone());
    }
    if let Some(account) = &info.account {
        detail("account", account.clone());
    }
    if !info.channels.is_empty() {
        detail("channels", info.channels.join(" "));
    }
    if let Some(idle) = info.idle_secs {
        let signon = info
            .signon_ts
            .and_then(|ts| DateTime::from_timestamp(ts, 0))
            .map(|dt| {
                dt.with_timezone(&Local)
                    .format("%Y-%m-%d %H:%M:%S")
                    .to_string()
            });
        let idle_text = format!("{}m {}s", idle / 60, idle % 60);
        match signon {
            Some(signon) => detail("idle", format!("{idle_text}, signon: {signon}")),
            None => detail("idle", idle_text),
        }
    }
    if let Some(away) = &info.away {
        detail("away", away.clone());
    }
    for line in &info.lines {
        detail("info", line.clone());
    }
    lines.push(Line::from(vec![
        Span::raw(" ".repeat(CONT_INDENT)),
        Span::styled("└".to_string(), label),
    ]));
    lines
}

/// Render the `/help` overview block (cleared by `/clear`).
fn render_help(ts: &str) -> Vec<Line<'static>> {
    let label = Style::default().fg(Color::Cyan);
    let mut lines = vec![Line::from(vec![
        Span::styled(format!("{ts} "), dim()),
        Span::styled("┌ help", label),
    ])];
    for (name, summary) in crate::input::COMMANDS {
        lines.push(Line::from(vec![
            Span::raw(" ".repeat(CONT_INDENT)),
            Span::styled("│ ".to_string(), label),
            Span::styled(format!("/{name:<12}"), bold()),
            Span::raw((*summary).to_string()),
        ]));
    }
    lines.push(Line::from(vec![
        Span::raw(" ".repeat(CONT_INDENT)),
        Span::styled("│ ".to_string(), label),
        Span::styled("^X".to_string(), bold()),
        Span::raw(" cycles targets; PgUp/PgDn scroll; Shift/Alt+Enter for a new line".to_string()),
    ]));
    lines.push(Line::from(vec![
        Span::raw(" ".repeat(CONT_INDENT)),
        Span::styled("└".to_string(), label),
    ]));
    lines
}

/// Gutter and style for irssi's `-!-` status lines.
fn status_line(ts: &str, text: String, color: Color) -> (Vec<Span<'static>>, String, Style) {
    (
        vec![
            Span::styled(format!("{ts} "), dim()),
            Span::styled("-!- ", Style::default().fg(color)),
        ],
        text,
        Style::default(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(kind: HistoryKind) -> HistoryItem {
        HistoryItem {
            // 2023-11-14 22:13:20 UTC; rendered in local time, so tests that
            // snapshot text force a fixed format via plain rendering checks.
            ts_ms: 1_700_000_000_000,
            kind,
        }
    }

    fn plain(lines: &[Line<'_>]) -> Vec<String> {
        lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect()
    }

    #[test]
    fn privmsg_continuations_indent_two_columns() {
        let it = item(HistoryKind::Privmsg {
            nick: "sshine".into(),
            target: "#diku".into(),
            text: "the question is, can you degrade your familiarity without degrading \
                   the quality of what you make?"
                .into(),
        });
        let opts = RenderOpts::default();
        let lines = plain(&render_item(&it, 60, &opts));
        assert!(lines.len() > 1, "must wrap at width 60: {lines:?}");
        assert!(
            lines[0]
                .chars()
                .skip(9)
                .collect::<String>()
                .starts_with("<sshine> ")
        );
        // Continuations line up under the first colon of the timestamp: two
        // spaces, then text — not a full hanging indent.
        for cont in &lines[1..] {
            assert!(
                cont.starts_with("  ") && !cont.starts_with("   "),
                "continuation must be indented exactly 2: {cont:?}"
            );
        }
        // No line exceeds the width.
        for line in &lines {
            assert!(line.chars().count() <= 60, "line too long: {line:?}");
        }
    }

    #[test]
    fn own_nick_is_bold_others_plain() {
        let it = item(HistoryKind::Privmsg {
            nick: "sshine".into(),
            target: "#diku".into(),
            text: "hi".into(),
        });
        let mine = RenderOpts {
            own_nick: "sshine".into(),
            ..RenderOpts::default()
        };
        let lines = render_item(&it, 80, &mine);
        let nick_span = &lines[0].spans[2];
        assert_eq!(nick_span.content, "sshine");
        assert!(nick_span.style.add_modifier.contains(Modifier::BOLD));

        let theirs = RenderOpts::default();
        let lines = render_item(&it, 80, &theirs);
        let nick_span = &lines[0].spans[2];
        assert!(!nick_span.style.add_modifier.contains(Modifier::BOLD));
        assert_eq!(nick_span.style.fg, None, "nick has default color");
        // The angle brackets are bold.
        assert!(
            lines[0].spans[1]
                .style
                .add_modifier
                .contains(Modifier::BOLD)
        );
    }

    #[test]
    fn whois_renders_as_block_and_is_clearable() {
        let it = item(HistoryKind::Whois(irciii_proto::history::WhoisInfo {
            nick: "zfnmxt".into(),
            userhost: Some("~z@example.org".into()),
            realname: Some("zfnmxt".into()),
            channels: vec!["#diku".into(), "##futhark".into()],
            idle_secs: Some(63),
            ..Default::default()
        }));
        assert!(it.kind.is_clearable());
        let lines = plain(&render_item(&it, 80, &RenderOpts::default()));
        assert!(
            lines[0].contains("whois zfnmxt (~z@example.org)"),
            "{lines:?}"
        );
        assert!(
            lines
                .iter()
                .any(|l| l.contains("channels: #diku ##futhark"))
        );
        assert!(lines.iter().any(|l| l.contains("idle: 1m 3s")));
        assert!(lines.last().expect("lines").contains("└"));
    }

    #[test]
    fn join_renders_as_status() {
        let it = item(HistoryKind::Join {
            nick: "n_ko".into(),
            channel: "#diku".into(),
        });
        let lines = plain(&render_item(&it, 80, &RenderOpts::default()));
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("-!- n_ko has joined #diku"), "{lines:?}");
    }

    #[test]
    fn timestamp_format_is_reactive() {
        let it = item(HistoryKind::ClientStatus { text: "hi".into() });
        let hms = plain(&render_item(&it, 80, &RenderOpts::default()));
        let hm = plain(&render_item(
            &it,
            80,
            &RenderOpts {
                ts_format: "%H:%M".into(),
                ..RenderOpts::default()
            },
        ));
        // Same item renders differently under a different format: the gutter
        // shrinks by the ":SS" part.
        assert_eq!(hms[0].len(), hm[0].len() + 3);
    }

    #[test]
    fn empty_text_still_renders_one_line() {
        let it = item(HistoryKind::ServerLine {
            text: String::new(),
        });
        let lines = render_item(&it, 80, &RenderOpts::default());
        assert_eq!(lines.len(), 1);
    }
}
