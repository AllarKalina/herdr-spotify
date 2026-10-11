//! What every screen shares, at the same cells as the Personal Inbox: the anchored header,
//! the content area and the shortcut line.

use super::{text, theme};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

/// The area below the header, inset two columns on each side and one row at the bottom.
pub(super) fn content(area: Rect) -> Rect {
    Rect::new(
        area.x.saturating_add(2),
        area.y.saturating_add(3),
        area.width.saturating_sub(4),
        area.height.saturating_sub(4),
    )
}

/// `Spotify`, then the current location when there is one. Only the last segment is accented.
pub(super) fn draw_header(frame: &mut Frame, crumb: Option<&str>) {
    let area = frame.area();
    let header = Rect::new(
        area.x.saturating_add(2),
        area.y.saturating_add(1),
        area.width.saturating_sub(4),
        1.min(area.height.saturating_sub(1)),
    );
    let (accent, muted) = (theme::accent(), theme::muted());
    let mut spans = vec![Span::styled(
        "Spotify",
        if crumb.is_some() { muted } else { accent },
    )];
    if let Some(crumb) = crumb {
        let room = usize::from(header.width).saturating_sub(10);
        spans.push(Span::styled(" / ", muted));
        spans.push(Span::styled(text::fit(crumb, room), accent));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), header);
}

/// Draws the longest set of shortcut hints that fits on the last content row, in the
/// unstyled terminal foreground. `choices` run from most to least complete.
pub(super) fn draw_footer(frame: &mut Frame, choices: &[&str]) {
    let content = content(frame.area());
    if content.height == 0 {
        return;
    }
    let row = Rect::new(content.x, content.bottom() - 1, content.width, 1);
    let hints = choices
        .iter()
        .find(|hints| text::width(hints) <= row.width)
        .or(choices.last())
        .copied()
        .unwrap_or_default();
    frame.render_widget(
        Paragraph::new(text::fit(hints, usize::from(row.width))),
        row,
    );
}
