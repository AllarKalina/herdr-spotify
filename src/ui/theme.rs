//! Terminal colour roles, the same ones the Personal Inbox uses. Ratatui emits roles, never
//! hex values: the host terminal's palette decides the result. Cover art is the one exception.

use ratatui::style::{Color, Modifier, Style};

/// The selected row in a list.
pub(super) fn selection() -> Style {
    Style::default()
        .fg(Color::Black)
        .bg(Color::Cyan)
        .add_modifier(Modifier::BOLD)
}

/// The current breadcrumb segment, and whatever is live: the playing state, the elapsed bar.
pub(super) fn accent() -> Style {
    Style::default()
        .fg(Color::LightCyan)
        .add_modifier(Modifier::BOLD)
}

/// The part of the progress line already played.
pub(super) fn progress() -> Style {
    Style::default().fg(Color::Cyan)
}

/// The track title.
pub(super) fn title() -> Style {
    Style::default().add_modifier(Modifier::BOLD)
}

/// Secondary labels, headings, and empty states.
pub(super) fn muted() -> Style {
    Style::default().fg(Color::Gray)
}

/// Structure that should recede: the unplayed progress line, the empty cover.
pub(super) fn faint() -> Style {
    Style::default().fg(Color::DarkGray)
}

pub(super) fn error() -> Style {
    Style::default().fg(Color::Red)
}
