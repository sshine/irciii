//! Shared UI styles, so command/block styling stays consistent everywhere.

use ratatui::style::{Color, Modifier, Style};

/// The accent color for commands and block borders (whois, names, who, help,
/// the command overlay, and the input border while typing a command).
pub const ACCENT: Color = Color::Cyan;

/// Style for block borders and command labels.
pub fn accent() -> Style {
    Style::default().fg(ACCENT)
}

/// Bold, default color.
pub fn bold() -> Style {
    Style::default().add_modifier(Modifier::BOLD)
}

/// Dim, default color (timestamps, secondary detail).
pub fn dim() -> Style {
    Style::default().add_modifier(Modifier::DIM)
}

/// Style for a mention of our own nick in message text.
pub fn mention() -> Style {
    Style::default()
        .fg(Color::White)
        .add_modifier(Modifier::BOLD)
}

/// A darker tint of a color (used for the input border while browsing the
/// input history).
pub fn darken(color: Color) -> Color {
    match color {
        Color::Rgb(r, g, b) => Color::Rgb(r / 2, g / 2, b / 2),
        _ => Color::DarkGray,
    }
}
