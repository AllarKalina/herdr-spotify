//! The list of things to play. For now that is Liked Songs; playlists and search join it
//! when the daemon can read the library.

use super::{text, theme};
use crate::protocol::State;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

pub struct Row {
    pub label: &'static str,
    /// The Spotify context this row plays.
    pub uri: String,
}

pub fn rows(state: &State) -> Vec<Row> {
    state
        .user
        .iter()
        .map(|user| Row {
            label: "♥ Liked Songs",
            uri: format!("spotify:user:{user}:collection"),
        })
        .collect()
}

/// Draws the label and the rows. The row being played says so at the right edge.
pub(super) fn draw(frame: &mut Frame, area: Rect, state: &State, selected: usize) {
    if area.height == 0 {
        return;
    }
    let label = Rect::new(area.x, area.y, area.width, 1);
    frame.render_widget(Paragraph::new("LIBRARY").style(theme::muted()), label);

    let width = usize::from(area.width);
    for (index, row) in rows(state).iter().enumerate() {
        let Ok(offset) = u16::try_from(index + 1) else {
            break;
        };
        if offset >= area.height {
            break;
        }
        let current = state.context_uri.as_deref() == Some(row.uri.as_str());
        let mark = match (current, state.playing) {
            (true, true) => "▶ playing",
            (true, false) => "paused",
            (false, _) => "",
        };
        let mark_width = mark.chars().count();
        let name = text::fit(row.label, width.saturating_sub(mark_width + 2));
        let gap = width.saturating_sub(name.chars().count() + mark_width);
        let (base, mark_style) = if index == selected {
            (theme::selection(), theme::selection())
        } else if current && state.playing {
            (Style::default(), theme::accent())
        } else {
            (Style::default(), theme::muted())
        };
        let line = Line::from(vec![
            Span::styled(name, base),
            Span::styled(" ".repeat(gap), base),
            Span::styled(mark, mark_style),
        ]);
        frame.render_widget(
            Paragraph::new(line),
            Rect::new(area.x, area.y + offset, area.width, 1),
        );
    }
}
