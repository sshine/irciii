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

    // --- Left segments: clock · nick(+modes) ---
    let clock = Local::now().format(&state.ts_format).to_string();
    let nick = if state.snapshot.user_modes.is_empty() {
        state.snapshot.nick.clone()
    } else {
        format!("{}({})", state.snapshot.nick, state.snapshot.user_modes)
    };
    let left: Vec<(&str, Style)> = vec![(&clock, clock_style), (SEP, muted), (&nick, text_style)];

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

    // --- Right segments: server · N:target(+modes), degrading left-first ---
    let target = if state.active_target == 0 {
        format!("{}:(status)", state.active_target)
    } else {
        match state.snapshot.targets.get(state.active_target - 1) {
            Some(t) if t.modes.is_empty() => format!("{}:{}", state.active_target, t.name),
            Some(t) => format!("{}:{}({})", state.active_target, t.name, t.modes),
            None => format!("{}:(status)", state.active_target),
        }
    };
    let mut parts: Vec<&str> = Vec::new();
    if let Some(server) = &state.snapshot.connected_to {
        parts.push(server);
    }
    parts.push(&target);

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
    let right_start = x_max.saturating_sub(right_text.chars().count() as u16);
    for (rx, ch) in (right_start..).zip(right_text.chars()) {
        if let Some(cell) = buf.cell_mut((rx, area.y)) {
            cell.set_char(ch);
            cell.set_style(muted);
        }
    }
}
