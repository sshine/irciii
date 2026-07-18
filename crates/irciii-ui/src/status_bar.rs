//! A promptui-styled status bar, fully left-aligned:
//! `HH:MM:SS · N:channel(+modes) · nick(+modes) [· N below]`.
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
    let warn = Style::default()
        .fg(theme.warning)
        .bg(theme.bg)
        .add_modifier(Modifier::BOLD);

    // Fill the row with the theme background.
    buf.set_style(Rect::new(area.x, area.y, area.width, 1), bg);
    for x in area.x..area.x + area.width {
        if let Some(cell) = buf.cell_mut((x, area.y)) {
            cell.set_char(' ');
        }
    }

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
    let nick = if state.snapshot.user_modes.is_empty() {
        state.snapshot.nick.clone()
    } else {
        format!("{}({})", state.snapshot.nick, state.snapshot.user_modes)
    };
    let below = (state.scroll_offset > 0).then(|| format!("{} below", state.scroll_offset));

    let mut segments: Vec<(String, Style)> = vec![(clock, clock_style), (target, text_style)];
    if !nick.is_empty() {
        segments.push((nick, muted));
    }
    if let Some(below) = below {
        segments.push((below, warn));
    }

    let mut x = area.x;
    let x_max = area.x + area.width;
    for (i, (text, style)) in segments.iter().enumerate() {
        if i > 0 {
            for ch in SEP.chars() {
                if x >= x_max {
                    return;
                }
                if let Some(cell) = buf.cell_mut((x, area.y)) {
                    cell.set_char(ch);
                    cell.set_style(muted);
                }
                x += 1;
            }
        }
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
}
