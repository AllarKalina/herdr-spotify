//! The player strip docked above the shortcut line: cover, track, progress, settings.

use super::{cover, text, theme};
use crate::protocol::{Repeat, State, Track};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

/// Rows the strip takes at full size, and when the popup is short or narrow.
pub(super) const FULL: u16 = cover::ROWS;
pub(super) const COMPACT: u16 = 3;
/// Below this width there is no room for a cover beside the text.
const COVER_MIN_WIDTH: u16 = 56;
const GAP: u16 = 2;

pub(super) fn draw(
    frame: &mut Frame,
    area: Rect,
    state: &State,
    art: Option<&cover::Art>,
    now_ms: u64,
) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let Some(track) = &state.track else {
        let width = usize::from(area.width);
        let rows = [
            Line::styled("Nothing playing", theme::muted()),
            Line::styled(
                text::fit("Press Enter to play the selection.", width),
                theme::faint(),
            ),
        ];
        let top = area.y + area.height.saturating_sub(COMPACT);
        for (row, line) in (top..area.bottom()).zip(rows) {
            frame.render_widget(Paragraph::new(line), Rect::new(area.x, row, area.width, 1));
        }
        return;
    };

    let full = area.height >= FULL;
    let with_cover = full && area.width >= COVER_MIN_WIDTH;
    let body = if with_cover {
        cover::draw(
            frame,
            Rect::new(area.x, area.y, cover::COLUMNS, cover::ROWS),
            art,
        );
        let shift = cover::COLUMNS + GAP;
        Rect::new(area.x + shift, area.y, area.width - shift, area.height)
    } else {
        area
    };
    let width = usize::from(body.width);
    let row = |offset: u16| Rect::new(body.x, body.y + offset, body.width, 1);

    frame.render_widget(Paragraph::new(heading(track, state.playing, width)), row(0));
    frame.render_widget(
        Paragraph::new(byline(track, width)).style(theme::muted()),
        row(1),
    );

    let position = state.position_at(now_ms);
    let times = format!(
        "{} / {}",
        text::clock(position),
        text::clock(track.duration_ms)
    );
    if full {
        frame.render_widget(
            Paragraph::new(bar(position, track.duration_ms, width)),
            row(3),
        );
        frame.render_widget(Paragraph::new(details(&times, state, width)), row(4));
    } else if body.height >= COMPACT {
        // One row carries both: the bar, then the times.
        let bar_width = width.saturating_sub(times.chars().count() + 2);
        let mut line = bar(position, track.duration_ms, bar_width);
        line.push_span(Span::raw("  "));
        line.push_span(Span::styled(times, theme::muted()));
        frame.render_widget(Paragraph::new(line), row(2));
    }
}

/// The track name, with the play state at the right edge.
fn heading(track: &Track, playing: bool, width: usize) -> Line<'static> {
    let (state, style) = if playing {
        ("▶ playing", theme::accent())
    } else {
        ("paused", theme::muted())
    };
    let room = width.saturating_sub(state.chars().count() + 2);
    let name = text::fit(&track.name, room);
    let gap = width.saturating_sub(name.chars().count() + state.chars().count());
    Line::from(vec![
        Span::styled(name, theme::title()),
        Span::raw(" ".repeat(gap)),
        Span::styled(state, style),
    ])
}

fn byline(track: &Track, width: usize) -> String {
    let artists = track.artists.join(", ");
    let text = match (artists.is_empty(), track.album.is_empty()) {
        (false, false) => format!("{artists} · {}", track.album),
        (false, true) => artists,
        (true, _) => track.album.clone(),
    };
    text::fit(&text, width)
}

/// A line the width of the strip: played in teal, the rest receding.
fn bar(position_ms: u32, duration_ms: u32, width: usize) -> Line<'static> {
    let played = if duration_ms == 0 {
        0
    } else {
        let cells = u64::from(position_ms) * width as u64 / u64::from(duration_ms);
        usize::try_from(cells).unwrap_or(width).min(width)
    };
    Line::from(vec![
        Span::styled("━".repeat(played), theme::progress()),
        Span::styled("─".repeat(width - played), theme::faint()),
    ])
}

/// Times at the left; volume, shuffle and repeat at the right, as many as fit.
fn details(times: &str, state: &State, width: usize) -> Line<'static> {
    let repeat = match state.repeat {
        Repeat::Off => "off",
        Repeat::Context => "all",
        Repeat::Track => "one",
    };
    let shuffle = if state.shuffle { "on" } else { "off" };
    let settings: [(&str, String, bool); 3] = [
        ("volume ", format!("{}%", state.volume), false),
        ("shuffle ", shuffle.to_owned(), state.shuffle),
        ("repeat ", repeat.to_owned(), state.repeat != Repeat::Off),
    ];
    let length = |count: usize| -> usize {
        let text: usize = settings[..count]
            .iter()
            .map(|(label, value, _)| label.len() + value.len())
            .sum();
        text + count.saturating_sub(1) * 3
    };
    let room = width.saturating_sub(times.chars().count() + 2);
    let shown = (0..=settings.len())
        .rev()
        .find(|count| length(*count) <= room)
        .unwrap_or(0);

    let mut spans = vec![Span::styled(times.to_owned(), theme::muted())];
    let gap = width.saturating_sub(times.chars().count() + length(shown));
    spans.push(Span::raw(" ".repeat(gap)));
    for (index, (label, value, on)) in settings.into_iter().take(shown).enumerate() {
        if index > 0 {
            spans.push(Span::styled(" · ", theme::faint()));
        }
        spans.push(Span::styled(label, theme::muted()));
        spans.push(Span::styled(
            value,
            if on {
                theme::accent()
            } else {
                Style::default()
            },
        ));
    }
    Line::from(spans)
}
