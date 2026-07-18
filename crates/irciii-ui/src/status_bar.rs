//! The irssi-style status bar: `HH:MM:SS nick(+modes) N:#channel(+chanmodes)`.
//!
//! The clock uses the same timestamp format as history lines, so the bar's
//! clock column-aligns with the line timestamps above it.

use chrono::Local;
use irciii_proto::ui_state::UiState;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

/// Render the status bar into a one-row area.
pub fn render(state: &UiState, area: Rect, buf: &mut Buffer) {
    if area.height == 0 {
        return;
    }
    let bar = Style::default().bg(Color::Blue).fg(Color::White);
    let clock = Local::now().format(&state.ts_format).to_string();

    let nick = if state.snapshot.user_modes.is_empty() {
        state.snapshot.nick.clone()
    } else {
        format!("{}({})", state.snapshot.nick, state.snapshot.user_modes)
    };

    let target = if state.active_target == 0 {
        format!("{}:(status)", state.active_target)
    } else {
        match state.snapshot.targets.get(state.active_target - 1) {
            Some(t) if t.modes.is_empty() => format!("{}:{}", state.active_target, t.name),
            Some(t) => format!("{}:{}({})", state.active_target, t.name, t.modes),
            None => format!("{}:(status)", state.active_target),
        }
    };

    let line = Line::from(vec![
        Span::styled(clock, bar),
        Span::styled(" ", bar),
        Span::styled(nick, bar),
        Span::styled(" ", bar),
        Span::styled(target, bar),
    ]);
    // Paint the full row with the bar background first.
    buf.set_style(Rect::new(area.x, area.y, area.width, 1), bar);
    line.render(Rect::new(area.x, area.y, area.width, 1), buf);
}
