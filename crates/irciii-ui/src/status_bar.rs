//! A promptui-styled status bar.
//!
//! Follows the idiom of promptui's `StatusBar`: theme background fill, a bold
//! accent segment on the left, ` · ` separators, and right-aligned muted
//! segments that degrade gracefully when the bar gets narrow.
//!
//! The clock stays leftmost and uses the same timestamp format as history
//! lines, so it column-aligns with the line timestamps above it.

use chrono::Local;
use irciii_proto::ui_state::UiState;
use promptui_core::theme::Theme;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

const SEP: &str = " · ";

/// Render the status bar into a one-row area.
pub fn render(state: &UiState, area: Rect, buf: &mut Buffer) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let theme = Theme::default();
    let bg = Style::default().bg(theme.bg);
    let clock_style = Style::default()
        .fg(theme.primary)
        .bg(theme.bg)
        .add_modifier(Modifier::BOLD);
    let text_style = Style::default().fg(theme.text).bg(theme.bg);
    let muted = Style::default().fg(theme.text_muted).bg(theme.bg);

    // Fill the row with the theme background.
    buf.set_style(Rect::new(area.x, area.y, area.width, 1), bg);
    for x in area.x..area.x + area.width {
        if let Some(cell) = buf.cell_mut((x, area.y)) {
            cell.set_char(' ');
        }
    }

    // --- Left segments: clock · N:channel(+modes) ---
    let clock = Local::now().format(&state.ts_format).to_string();
    let target = if state.active_target == 0 {
        "(status)".to_string()
    } else {
        match state.snapshot.targets.get(state.active_target - 1) {
            Some(t) if t.modes.is_empty() => format!("{}:{}", state.active_target, t.name),
            Some(t) => format!("{}:{}({})", state.active_target, t.name, t.modes),
            None => "(status)".to_string(),
        }
    };
    let left: Vec<(&str, Style)> = vec![(&clock, clock_style), (SEP, muted), (&target, text_style)];

    let mut x = area.x;
    let x_max = area.x + area.width;
    for (text, style) in &left {
        for ch in text.chars() {
            if x >= x_max {
                return;
            }
            if let Some(cell) = buf.cell_mut((x, area.y)) {
                cell.set_char(ch);
                cell.set_style(*style);
            }
            x += 1;
        }
    }

    // --- Right segments: nick(+modes) · "N below" while scrolled ---
    let nick = if state.snapshot.user_modes.is_empty() {
        state.snapshot.nick.clone()
    } else {
        format!("{}({})", state.snapshot.nick, state.snapshot.user_modes)
    };
    let below = (state.scroll_offset > 0).then(|| format!("{} below", state.scroll_offset));
    let mut parts: Vec<&str> = Vec::new();
    if !nick.is_empty() {
        parts.push(&nick);
    }
    if let Some(below) = &below {
        parts.push(below);
    }
    if parts.is_empty() {
        return;
    }

    let right_text = loop {
        let candidate = parts.join(SEP);
        if x_max.saturating_sub(x) > candidate.chars().count() as u16 {
            break candidate;
        }
        if parts.len() > 1 {
            parts.remove(0);
        } else {
            return; // nothing fits
        }
    };
    // The "N below" scroll indicator is highlighted; the rest stays muted.
    let highlight_from = below
        .as_ref()
        .filter(|b| right_text.ends_with(b.as_str()))
        .map(|b| right_text.chars().count() - b.chars().count())
        .unwrap_or(usize::MAX);
    let warn = Style::default()
        .fg(theme.warning)
        .bg(theme.bg)
        .add_modifier(Modifier::BOLD);
    let right_start = x_max.saturating_sub(right_text.chars().count() as u16);
    for (i, (rx, ch)) in (right_start..).zip(right_text.chars()).enumerate() {
        if let Some(cell) = buf.cell_mut((rx, area.y)) {
            cell.set_char(ch);
            cell.set_style(if i >= highlight_from { warn } else { muted });
        }
    }
}
