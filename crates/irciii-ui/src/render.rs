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
}

impl Default for RenderOpts {
    fn default() -> Self {
        Self {
            ts_format: "%H:%M:%S".into(),
        }
    }
}

/// irssi-inspired palette for hashing nicks to colors.
const NICK_COLORS: &[Color] = &[
    Color::Cyan,
    Color::Magenta,
    Color::Green,
    Color::Yellow,
    Color::Blue,
    Color::LightCyan,
    Color::LightMagenta,
    Color::LightGreen,
];

fn nick_color(nick: &str) -> Color {
    let hash: usize = nick.bytes().map(usize::from).sum();
    NICK_COLORS[hash % NICK_COLORS.len()]
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

/// Render one history item into wrapped lines with an irssi-style hanging
/// indent: continuation lines are indented to the width of the gutter
/// (timestamp + sender), so message text stays aligned in its own column.
pub fn render_item(item: &HistoryItem, width: u16, opts: &RenderOpts) -> Vec<Line<'static>> {
    let ts = ts_span(item, opts);
    let (gutter_spans, text, text_style) = match &item.kind {
        HistoryKind::Privmsg { nick, text, .. } => (
            vec![
                Span::styled(format!("{ts} "), dim()),
                Span::raw("<"),
                Span::styled(nick.clone(), Style::default().fg(nick_color(nick))),
                Span::raw("> "),
            ],
            text.clone(),
            Style::default(),
        ),
        HistoryKind::Action { nick, text, .. } => (
            vec![
                Span::styled(format!("{ts} "), dim()),
                Span::raw(" * "),
                Span::styled(format!("{nick} "), Style::default().fg(nick_color(nick))),
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
    };

    let gutter_width: usize = gutter_spans.iter().map(|s| s.content.width()).sum();
    let body_width = (width as usize).saturating_sub(gutter_width).max(8) as u16;
    let indent = " ".repeat(gutter_width.min(width as usize / 2));

    let mut lines = Vec::new();
    for (i, part) in wrap_line_strs(&text, body_width).into_iter().enumerate() {
        if i == 0 {
            let mut spans = gutter_spans.clone();
            spans.push(Span::styled(part.to_string(), text_style));
            lines.push(Line::from(spans));
        } else {
            lines.push(Line::from(vec![
                Span::raw(indent.clone()),
                Span::styled(part.trim_start().to_string(), text_style),
            ]));
        }
    }
    if lines.is_empty() {
        lines.push(Line::from(gutter_spans));
    }
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
    fn privmsg_hanging_indent_aligns_continuations() {
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
        // Gutter: "HH:MM:SS <sshine> " = 8 + 1 + 8 + 1 = 18 columns.
        let gutter = 18;
        assert!(
            lines[0]
                .chars()
                .skip(9)
                .collect::<String>()
                .starts_with("<sshine> ")
        );
        for cont in &lines[1..] {
            assert!(
                cont.starts_with(&" ".repeat(gutter)),
                "continuation must be indented {gutter}: {cont:?}"
            );
            assert!(cont.len() > gutter, "continuation must have text");
        }
        // No line exceeds the width.
        for line in &lines {
            assert!(line.chars().count() <= 60, "line too long: {line:?}");
        }
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
