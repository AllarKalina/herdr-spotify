//! The one list every screen uses: labels, rows with a right-hand detail, a selection that
//! stays in view.

use super::{text, theme};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

/// From this width a row shows its secondary text in a column of its own.
const TWO_COLUMNS: usize = 64;
const GAP: usize = 2;
/// The playing mark and the space after it.
const MARK_WIDTH: usize = 2;

pub enum Row {
    /// A heading or a remark; never selected.
    Label(String),
    Item(Item),
    Blank,
}

#[derive(Default)]
pub struct Item {
    pub name: String,
    /// Artists of a track, owner of a playlist.
    pub secondary: String,
    /// Duration of a track, size of a playlist.
    pub detail: String,
    /// True for what is loaded in the player.
    pub current: bool,
}

/// Which item is selected, and how far the list is scrolled.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Cursor {
    pub selected: usize,
    offset: usize,
}

impl Cursor {
    pub fn move_by(&mut self, delta: isize, count: usize) {
        let last = count.saturating_sub(1);
        self.selected = self.selected.saturating_add_signed(delta).min(last);
    }

    pub fn clamp(&mut self, count: usize) {
        self.selected = self.selected.min(count.saturating_sub(1));
    }

    /// Scrolls so rows `top..=bottom` are visible: the selected item, and a label directly
    /// above or below it, so a heading or a "loading" remark is never scrolled out of sight.
    fn scroll_to(&mut self, top: usize, row: usize, bottom: usize, height: usize) {
        if height > 0 && bottom >= self.offset + height {
            self.offset = bottom + 1 - height;
        }
        // The item itself wins over the remark below it when both cannot fit.
        if height > 0 && row >= self.offset + height {
            self.offset = row + 1 - height;
        }
        if top < self.offset {
            self.offset = top;
        }
        if row < self.offset {
            self.offset = row;
        }
    }
}

/// Draws `rows` in `area`. `playing` colours the mark of the current item.
pub(super) fn draw(
    frame: &mut Frame,
    area: Rect,
    rows: &[Row],
    cursor: &mut Cursor,
    playing: bool,
) {
    let height = usize::from(area.height);
    let width = usize::from(area.width);
    if height == 0 || width == 0 {
        return;
    }
    // Find the display row of the selected item.
    let mut ordinal = 0;
    let mut selected_row = None;
    for (index, row) in rows.iter().enumerate() {
        if matches!(row, Row::Item(_)) {
            if ordinal == cursor.selected {
                selected_row = Some(index);
            }
            ordinal += 1;
        }
    }
    let is_label = |index: usize| matches!(rows.get(index), Some(Row::Label(_)));
    match selected_row {
        Some(row) => {
            let top = if row > 0 && is_label(row - 1) {
                row - 1
            } else {
                row
            };
            let bottom = if is_label(row + 1) { row + 1 } else { row };
            cursor.scroll_to(top, row, bottom, height);
        }
        None => cursor.offset = 0,
    }
    cursor.offset = cursor.offset.min(rows.len().saturating_sub(height));

    // Every row reserves the same right-hand column, so names and artists line up whether or
    // not a row carries the playing mark.
    let detail_width = rows
        .iter()
        .filter_map(|row| match row {
            Row::Item(item) => Some(item.detail.chars().count()),
            _ => None,
        })
        .max()
        .unwrap_or(0);

    let visible = rows.iter().enumerate().skip(cursor.offset).take(height);
    for (line, (index, row)) in (area.y..area.bottom()).zip(visible) {
        let rendered = match row {
            Row::Blank => continue,
            Row::Label(label) => Line::styled(text::fit(label, width), theme::muted()),
            Row::Item(item) => item_line(
                item,
                width,
                detail_width,
                selected_row == Some(index),
                playing,
            ),
        };
        frame.render_widget(
            Paragraph::new(rendered),
            Rect::new(area.x, line, area.width, 1),
        );
    }
}

fn item_line(
    item: &Item,
    width: usize,
    detail_width: usize,
    selected: bool,
    playing: bool,
) -> Line<'static> {
    let (base, quiet, mark_style) = if selected {
        (theme::selection(), theme::selection(), theme::selection())
    } else {
        let mark = if playing {
            theme::accent()
        } else {
            theme::muted()
        };
        (Style::default(), theme::muted(), mark)
    };
    let mark = if item.current { "▶ " } else { "  " };
    let detail = format!("{:>detail_width$}", item.detail);
    let right_width = MARK_WIDTH + detail_width;
    let room = width.saturating_sub(right_width + GAP);

    let two_columns = width >= TWO_COLUMNS && !item.secondary.is_empty();
    let name_width = if two_columns { room * 55 / 100 } else { room };
    let name = text::fit(&item.name, name_width);
    let mut spans = vec![Span::styled(text::pad(&name, name_width), base)];
    if two_columns {
        let secondary_width = room.saturating_sub(name_width + GAP);
        let secondary = text::fit(&item.secondary, secondary_width);
        spans.push(Span::styled(" ".repeat(GAP), base));
        spans.push(Span::styled(text::pad(&secondary, secondary_width), quiet));
    }
    spans.push(Span::styled(
        " ".repeat(width.saturating_sub(room + right_width)),
        base,
    ));
    spans.push(Span::styled(mark, mark_style));
    spans.push(Span::styled(detail, quiet));
    Line::from(spans)
}
