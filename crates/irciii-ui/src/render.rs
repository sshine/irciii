//! Rendering of a single [`HistoryItem`] into styled, wrapped lines.
//!
//! This is where the "reactive history" lives: items are structured data and
//! every visible line is rendered on demand, so changing e.g. the timestamp
//! format re-renders without touching the history.

use chrono::{DateTime, Local};
use irciii_proto::history::{HistoryItem, HistoryKind};
use promptui_core::widgets::word_wrap::wrap_line_strs;
use ratatui::style::{Color, Modifier, Style};

use crate::styles::{bold, dim};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

/// Options that affect how history items are rendered.
#[derive(Debug, Clone)]
pub struct RenderOpts {
    /// strftime format for line timestamps (matches the status bar clock).
    pub ts_format: String,
    /// Our own nick, rendered bold to stand out.
    pub own_nick: String,
    /// History viewport height in rows; the logo banner fills it (so its cell
    /// resolution scales with the terminal instead of a fixed coarse cap).
    pub viewport_rows: u16,
}

impl Default for RenderOpts {
    fn default() -> Self {
        Self {
            ts_format: "%H:%M:%S".into(),
            own_nick: String::new(),
            viewport_rows: 24,
        }
    }
}

impl RenderOpts {
    /// Options derived from the current UI state.
    pub fn from_state(state: &irciii_proto::ui_state::UiState) -> Self {
        Self {
            ts_format: state.ts_format.clone(),
            own_nick: state.snapshot.nick.clone(),
            viewport_rows: state.viewport_height,
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

fn ts_span(item: &HistoryItem, opts: &RenderOpts) -> String {
    DateTime::from_timestamp_millis(item.ts_ms)
        .map(|utc| {
            utc.with_timezone(&Local)
                .format(&opts.ts_format)
                .to_string()
        })
        .unwrap_or_else(|| "??:??:??".to_string())
}

/// Render one history item into wrapped lines. Continuation lines always
/// indent by [`CONT_INDENT`] columns.
pub fn render_item(item: &HistoryItem, width: u16, opts: &RenderOpts) -> Vec<Line<'static>> {
    let ts = ts_span(item, opts);
    // Block items render on their own, without the gutter machinery and
    // without timestamps, so the border stays aligned at column 0.
    match &item.kind {
        HistoryKind::Whois(info) => return render_whois(info, width),
        HistoryKind::Help => return render_help(),
        HistoryKind::NamesReply { channel, names, .. } => {
            return render_names(channel, names, width);
        }
        HistoryKind::Who { mask, entries } => return render_who(mask, entries),
        HistoryKind::Motd { server, lines } => return render_motd(server, lines, width),
        HistoryKind::Logo => return render_logo(width, opts.viewport_rows),
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
                Span::styled(format!("-{from}- "), crate::styles::accent()),
            ],
            text.clone(),
            Style::default(),
        ),
        HistoryKind::Join {
            nick,
            userhost,
            channel,
        } => (
            presence_gutter(&ts, nick, userhost),
            format!("has joined {channel}"),
            Style::default(),
        ),
        HistoryKind::Part {
            nick,
            userhost,
            channel,
            reason,
        } => (
            presence_gutter(&ts, nick, userhost),
            match reason {
                Some(reason) => format!("has left {channel} ({reason})"),
                None => format!("has left {channel}"),
            },
            Style::default(),
        ),
        HistoryKind::Kick {
            by,
            userhost,
            channel,
            kicked,
            reason,
        } => (
            presence_gutter(&ts, by, userhost),
            match reason {
                Some(reason) => format!("has kicked {kicked} from {channel} ({reason})"),
                None => format!("has kicked {kicked} from {channel}"),
            },
            Style::default(),
        ),
        HistoryKind::QuitMsg {
            nick,
            userhost,
            reason,
        } => (
            presence_gutter(&ts, nick, userhost),
            match reason {
                Some(reason) => format!("has quit ({reason})"),
                None => "has quit".to_string(),
            },
            Style::default(),
        ),
        HistoryKind::NickChange { old, new } => status_line(
            &ts,
            format!("{old} is now known as {new}"),
            crate::styles::ACCENT,
        ),
        HistoryKind::Mode { by, target, modes } => status_line(
            &ts,
            format!("mode/{target} [{modes}] by {by}"),
            crate::styles::ACCENT,
        ),
        HistoryKind::Topic { channel, topic, by } => status_line(
            &ts,
            match by {
                Some(by) => format!("{by} changed the topic of {channel} to: {topic}"),
                None => format!("topic for {channel}: {topic}"),
            },
            crate::styles::ACCENT,
        ),
        HistoryKind::ServerLine { text } => (
            vec![Span::styled(format!("{ts} "), dim())],
            text.clone(),
            dim(),
        ),
        HistoryKind::ClientStatus { text } => status_line(&ts, text.clone(), crate::styles::ACCENT),
        HistoryKind::Error { text } => status_line(&ts, text.clone(), Color::Red),
        HistoryKind::Whois(_)
        | HistoryKind::Who { .. }
        | HistoryKind::Help
        | HistoryKind::NamesReply { .. }
        | HistoryKind::Motd { .. }
        | HistoryKind::Logo => {
            unreachable!("handled above")
        }
    };

    // Highlight mentions of our own nick in messages from others.
    let highlight_nick = match &item.kind {
        HistoryKind::Privmsg { nick, .. } | HistoryKind::Action { nick, .. } => {
            nick != &opts.own_nick
        }
        HistoryKind::Notice { from, .. } => from != &opts.own_nick,
        _ => false,
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
        let body = if highlight_nick {
            mention_spans(&part, &opts.own_nick, text_style)
        } else {
            vec![Span::styled(part, text_style)]
        };
        if i == 0 {
            let mut spans = gutter_spans.clone();
            spans.extend(body);
            lines.push(Line::from(spans));
        } else {
            let mut spans = vec![Span::raw(indent.clone())];
            spans.extend(body);
            lines.push(Line::from(spans));
        }
    }
    if lines.is_empty() {
        lines.push(Line::from(gutter_spans));
    }
    lines
}

/// Style for a mention of our own nick in message text.
fn mention_style() -> Style {
    crate::styles::mention()
}

/// Whether the char is part of a nick (so matches respect word boundaries).
fn is_nick_char(c: char) -> bool {
    c.is_alphanumeric()
        || matches!(
            c,
            '_' | '-' | '[' | ']' | '\\' | '`' | '^' | '{' | '}' | '|'
        )
}

/// Split `text` into spans, highlighting whole-word, case-insensitive
/// occurrences of `nick`.
fn mention_spans(text: &str, nick: &str, base: Style) -> Vec<Span<'static>> {
    if nick.is_empty() {
        return vec![Span::styled(text.to_string(), base)];
    }
    let lower_text = text.to_lowercase();
    let lower_nick = nick.to_lowercase();
    // Only safe to slice by lowercase indices when the mapping is 1:1; nicks
    // and IRC text are near-always ASCII — bail out to unstyled otherwise.
    if lower_text.len() != text.len() {
        return vec![Span::styled(text.to_string(), base)];
    }
    let mut spans = Vec::new();
    let mut pos = 0;
    while let Some(found) = lower_text[pos..].find(&lower_nick) {
        let start = pos + found;
        let end = start + nick.len();
        let boundary_before =
            start == 0 || !text[..start].chars().next_back().is_some_and(is_nick_char);
        let boundary_after =
            end >= text.len() || !text[end..].chars().next().is_some_and(is_nick_char);
        if boundary_before && boundary_after {
            if start > pos {
                spans.push(Span::styled(text[pos..start].to_string(), base));
            }
            spans.push(Span::styled(text[start..end].to_string(), mention_style()));
            pos = end;
        } else {
            // Not a whole word: keep scanning past this occurrence (advance a
            // full char so we never split a multi-byte character).
            let step = text[start..].chars().next().map_or(1, char::len_utf8);
            spans.push(Span::styled(text[pos..start + step].to_string(), base));
            pos = start + step;
        }
    }
    if pos < text.len() {
        spans.push(Span::styled(text[pos..].to_string(), base));
    }
    if spans.is_empty() {
        spans.push(Span::styled(String::new(), base));
    }
    spans
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

/// Render a WHOIS reply block: a header plus indented detail lines. Block
/// items print no timestamp so the border stays aligned at column 0.
fn render_whois(info: &irciii_proto::history::WhoisInfo, width: u16) -> Vec<Line<'static>> {
    let label = crate::styles::accent();
    let mut lines = vec![Line::from(vec![
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
        // "│ " on the first line, "│   " on wrapped continuations.
        let first = (width as usize).saturating_sub(2).max(8) as u16;
        let cont = (width as usize).saturating_sub(4).max(8) as u16;
        let text = format!("{name}: {value}");
        for (i, part) in wrap_two(&text, first, cont).into_iter().enumerate() {
            lines.push(Line::from(vec![
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
    lines.push(Line::from(Span::styled("└".to_string(), label)));
    lines
}

/// The logo PNG, embedded at compile time.
static LOGO_PNG: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../irciii-logo.png"
));

/// Render the embedded irciii logo as ANSI block art, fit to the full history
/// viewport (`width` × `rows`) and centered. Filling the viewport gives the
/// block art enough cells to resolve the logo instead of a coarse thumbnail;
/// it then scrolls out of sight as lines arrive. The rendered lines are cached
/// per size: decoding and cell rendering only happen when the size changes.
fn render_logo(width: u16, rows: u16) -> Vec<Line<'static>> {
    thread_local! {
        #[expect(clippy::type_complexity, reason = "a simple size-keyed cache")]
        static CACHE: std::cell::RefCell<Option<((u16, u16), Vec<Line<'static>>)>> =
            const { std::cell::RefCell::new(None) };
    }
    CACHE.with(|cache| {
        if let Some((size, lines)) = &*cache.borrow()
            && *size == (width, rows)
        {
            return lines.clone();
        }
        let lines = build_logo_lines(width, rows);
        *cache.borrow_mut() = Some(((width, rows), lines.clone()));
        lines
    })
}

/// Decode the logo and turn its cell grid into styled, centered lines. Runs of
/// same-colored cells are merged into one span to keep the line span count low.
fn build_logo_lines(width: u16, rows: u16) -> Vec<Line<'static>> {
    let Ok(img) = graphix::image::load_from_memory(LOGO_PNG) else {
        return vec![Line::from("")];
    };
    let img = img.to_rgb8();
    let (w, h) = img.dimensions();
    let (cols, rows) = graphix::fit_grid(w, h, u32::from(width), u32::from(rows.max(1)));
    let grid = graphix::render_cells(&img, cols, rows);
    let pad = " ".repeat((usize::from(width).saturating_sub(cols as usize)) / 2);
    grid.iter()
        .map(|row| {
            let mut spans = vec![Span::raw(pad.clone())];
            let mut run = String::new();
            let mut run_style: Option<Style> = None;
            for cell in row {
                let style = Style::default()
                    .fg(Color::Rgb(cell.fg.r, cell.fg.g, cell.fg.b))
                    .bg(Color::Rgb(cell.bg.r, cell.bg.g, cell.bg.b));
                if run_style != Some(style) && !run.is_empty() {
                    spans.push(Span::styled(
                        std::mem::take(&mut run),
                        run_style.unwrap_or_default(),
                    ));
                }
                run_style = Some(style);
                run.push(cell.ch);
            }
            if !run.is_empty() {
                spans.push(Span::styled(run, run_style.unwrap_or_default()));
            }
            Line::from(spans)
        })
        .collect()
}

/// Render the `/help` overview block (cleared by `/clear`).
fn render_help() -> Vec<Line<'static>> {
    let label = crate::styles::accent();
    let mut lines = vec![Line::from(Span::styled("┌ help", label))];
    for (name, summary) in crate::input::COMMANDS {
        lines.push(Line::from(vec![
            Span::styled("│ ".to_string(), label),
            Span::styled(format!("/{name:<12}"), bold()),
            Span::raw((*summary).to_string()),
        ]));
    }
    lines.push(Line::from(vec![
        Span::styled("│ ".to_string(), label),
        Span::styled("^X".to_string(), bold()),
        Span::raw(" cycles targets; PgUp/PgDn scroll; Shift/Alt+Enter for a new line".to_string()),
    ]));
    lines.push(Line::from(Span::styled("└".to_string(), label)));
    lines
}

/// Render the server MOTD as a card (cleared by `/clear`).
fn render_motd(server: &str, motd: &[String], width: u16) -> Vec<Line<'static>> {
    let label = crate::styles::accent();
    let mut lines = vec![Line::from(vec![
        Span::styled("┌ motd ", label),
        Span::styled(server.to_string(), bold()),
        Span::styled(format!(" ({} lines)", motd.len()), dim()),
    ])];
    let first = (width as usize).saturating_sub(2).max(8) as u16;
    let cont = (width as usize).saturating_sub(4).max(8) as u16;
    for text in motd {
        for (i, part) in wrap_two(text, first, cont).into_iter().enumerate() {
            lines.push(Line::from(vec![
                Span::styled(if i == 0 { "│ " } else { "│   " }.to_string(), label),
                Span::styled(part, dim()),
            ]));
        }
    }
    lines.push(Line::from(Span::styled("└".to_string(), label)));
    lines
}

/// Render a WHO reply as an aligned table block (cleared by `/clear`).
fn render_who(mask: &str, entries: &[irciii_proto::history::WhoEntry]) -> Vec<Line<'static>> {
    let label = crate::styles::accent();
    let mut lines = vec![Line::from(vec![
        Span::styled("┌ who ", label),
        Span::styled(mask.to_string(), bold()),
        Span::styled(format!(" ({})", entries.len()), dim()),
    ])];
    let nick_w = entries.iter().map(|e| e.nick.width()).max().unwrap_or(1);
    let flags_w = entries.iter().map(|e| e.flags.width()).max().unwrap_or(1);
    let uh_w = entries
        .iter()
        .map(|e| e.userhost.width())
        .max()
        .unwrap_or(1);
    for entry in entries {
        lines.push(Line::from(vec![
            Span::styled("│ ".to_string(), label),
            Span::styled(format!("{:<nick_w$}  ", entry.nick), bold()),
            Span::styled(format!("{:<flags_w$}  ", entry.flags), dim()),
            Span::raw(format!("{:<uh_w$}  ", entry.userhost)),
            Span::styled(entry.realname.clone(), dim()),
        ]));
    }
    lines.push(Line::from(Span::styled("└".to_string(), label)));
    lines
}

/// Render a NAMES reply as a column-major table block (cleared by `/clear`).
fn render_names(channel: &str, names: &[String], width: u16) -> Vec<Line<'static>> {
    let label = crate::styles::accent();
    let mut lines = vec![Line::from(vec![
        Span::styled("┌ names ", label),
        Span::styled(channel.to_string(), bold()),
        Span::styled(format!(" ({})", names.len()), dim()),
    ])];

    // Sort by membership rank (@ ops, % halfops, + voiced, rest), then name.
    let rank = |name: &str| match name.chars().next() {
        Some('~') => 0,
        Some('&') => 1,
        Some('@') => 2,
        Some('%') => 3,
        Some('+') => 4,
        _ => 5,
    };
    let mut sorted: Vec<&String> = names.iter().collect();
    sorted.sort_by(|a, b| {
        rank(a)
            .cmp(&rank(b))
            .then_with(|| a.to_lowercase().cmp(&b.to_lowercase()))
    });

    let body = (width as usize).saturating_sub(2).max(8);
    let col_w = sorted.iter().map(|n| n.width()).max().unwrap_or(1) + 2;
    let cols = (body / col_w).max(1);
    let rows = sorted.len().div_ceil(cols);
    // Condense join-flood sized lists: cap the card and count the rest.
    // (/names above the screen size opens a modal with the full list.)
    let shown_rows = rows.min(NAMES_CARD_MAX_ROWS);
    for row in 0..shown_rows {
        let mut cells = String::new();
        for col in 0..cols {
            if let Some(name) = sorted.get(col * shown_rows + row) {
                cells.push_str(&format!("{name:<col_w$}"));
            }
        }
        lines.push(Line::from(vec![
            Span::styled("│ ".to_string(), label),
            Span::raw(cells.trim_end().to_string()),
        ]));
    }
    let shown = (shown_rows * cols).min(sorted.len());
    if shown < sorted.len() {
        lines.push(Line::from(vec![
            Span::styled("│ ".to_string(), label),
            Span::styled(format!("+ {} more", sorted.len() - shown), dim()),
        ]));
    }
    lines.push(Line::from(Span::styled("└".to_string(), label)));
    lines
}

/// Maximum rows a names card shows inline before condensing to "+ N more".
pub const NAMES_CARD_MAX_ROWS: usize = 8;

/// Gutter for join/part/quit lines: the full `nick!user@host` with the
/// nickname highlighted and the userhost dimmed.
fn presence_gutter(ts: &str, nick: &str, userhost: &Option<String>) -> Vec<Span<'static>> {
    let mut spans = vec![
        Span::styled(format!("{ts} "), dim()),
        Span::styled("-!- ".to_string(), crate::styles::accent()),
        Span::styled(nick.to_string(), bold()),
    ];
    if let Some(uh) = userhost {
        spans.push(Span::styled(format!("!{uh}"), dim()));
    }
    spans.push(Span::raw(" "));
    spans
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
    fn mentions_of_own_nick_are_highlighted() {
        let opts = RenderOpts {
            own_nick: "sshine".into(),
            ..RenderOpts::default()
        };
        let it = item(HistoryKind::Privmsg {
            nick: "n_ko".into(),
            target: "#diku".into(),
            text: "sshine, are you also into quantum computing? Sshine?".into(),
        });
        let lines = render_item(&it, 120, &opts);
        let highlighted: Vec<&str> = lines[0]
            .spans
            .iter()
            .filter(|s| s.style == mention_style())
            .map(|s| s.content.as_ref())
            .collect();
        // Case-insensitive, word-boundary matches only.
        assert_eq!(highlighted, vec!["sshine", "Sshine"]);

        // Substrings are not mentions.
        let it = item(HistoryKind::Privmsg {
            nick: "n_ko".into(),
            target: "#diku".into(),
            text: "sshines stuff is not a mention".into(),
        });
        let lines = render_item(&it, 120, &opts);
        assert!(lines[0].spans.iter().all(|s| s.style != mention_style()));

        // Our own messages are not highlighted.
        let it = item(HistoryKind::Privmsg {
            nick: "sshine".into(),
            target: "#diku".into(),
            text: "talking about sshine myself".into(),
        });
        let lines = render_item(&it, 120, &opts);
        assert!(lines[0].spans.iter().all(|s| s.style != mention_style()));
    }

    #[test]
    fn names_renders_as_sorted_table_block() {
        let it = item(HistoryKind::NamesReply {
            channel: "#diku".into(),
            requested: false,
            names: vec![
                "sshine".into(),
                "@concieggs".into(),
                "+n_ko".into(),
                "zfnmxt".into(),
            ],
        });
        assert!(it.kind.is_card());
        let lines = plain(&render_item(&it, 40, &RenderOpts::default()));
        assert!(lines[0].starts_with("┌ names #diku (4)"), "{lines:?}");
        // Column-major with ops before voiced before the rest: the first
        // column reads @concieggs then +n_ko.
        assert!(lines[1].starts_with("│ @concieggs"), "{lines:?}");
        assert!(lines[2].starts_with("│ +n_ko"), "{lines:?}");
        let body = lines[1..lines.len() - 1].join("\n");
        for name in ["sshine", "zfnmxt"] {
            assert!(body.contains(name), "{name} missing: {body}");
        }
        assert!(lines.last().expect("lines").starts_with("└"));
        // No timestamp gutter on block items.
        assert!(lines[0].starts_with("┌"));
    }

    #[test]
    fn huge_names_card_condenses_with_more_line() {
        let names: Vec<String> = (0..100).map(|i| format!("nick{i:02}")).collect();
        let it = item(HistoryKind::NamesReply {
            channel: "#big".into(),
            names,
            requested: false,
        });
        let lines = plain(&render_item(&it, 40, &RenderOpts::default()));
        // header + capped rows + "+ N more" + footer
        assert!(lines.len() <= NAMES_CARD_MAX_ROWS + 3, "{}", lines.len());
        assert!(
            lines[lines.len() - 2].contains("more"),
            "{:?}",
            lines[lines.len() - 2]
        );
    }

    #[test]
    fn whois_renders_as_block_and_is_card() {
        let it = item(HistoryKind::Whois(irciii_proto::history::WhoisInfo {
            nick: "zfnmxt".into(),
            userhost: Some("~z@example.org".into()),
            realname: Some("zfnmxt".into()),
            channels: vec!["#diku".into(), "##futhark".into()],
            idle_secs: Some(63),
            ..Default::default()
        }));
        assert!(it.kind.is_card());
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
            userhost: Some("~n@host".into()),
            channel: "#diku".into(),
        });
        let lines = render_item(&it, 80, &RenderOpts::default());
        let text = plain(&lines);
        assert_eq!(text.len(), 1);
        assert!(
            text[0].contains("-!- n_ko!~n@host has joined #diku"),
            "{text:?}"
        );
        // The nickname is highlighted; the userhost is dimmed.
        let nick_span = &lines[0].spans[2];
        assert_eq!(nick_span.content, "n_ko");
        assert!(nick_span.style.add_modifier.contains(Modifier::BOLD));
        let uh_span = &lines[0].spans[3];
        assert_eq!(uh_span.content, "!~n@host");
        assert!(uh_span.style.add_modifier.contains(Modifier::DIM));
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
    fn logo_fills_the_viewport_and_stays_within_bounds() {
        // A wide viewport so the row budget (not the width) binds the fit.
        let opts = RenderOpts {
            viewport_rows: 20,
            ..RenderOpts::default()
        };
        let lines = render_item(&item(HistoryKind::Logo), 200, &opts);
        // A non-empty banner bounded by the viewport, no line overflowing width.
        assert!(!lines.is_empty());
        assert!(lines.len() <= opts.viewport_rows as usize);
        for line in &lines {
            let w: usize = line.spans.iter().map(|s| s.content.width()).sum();
            assert!(w <= 200, "logo line exceeds width: {w}");
        }
        // A taller viewport yields a higher-resolution (taller) render.
        let taller = render_item(
            &item(HistoryKind::Logo),
            200,
            &RenderOpts {
                viewport_rows: 40,
                ..RenderOpts::default()
            },
        );
        assert!(
            taller.len() > lines.len(),
            "more rows should mean more detail"
        );
        // The logo is not a card, so `/clear` leaves it in the log.
        assert!(!HistoryKind::Logo.is_card());
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
