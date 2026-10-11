//! Cover art as coloured half blocks: each cell shows two pixels, one above the other.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Color;
use std::path::Path;

/// The cover's size in cells. A cell is about twice as tall as wide, so this is square.
pub(super) const COLUMNS: u16 = 10;
pub(super) const ROWS: u16 = 5;

type Pixel = [u8; 3];

/// A decoded cover, `COLUMNS` pixels wide and two pixels per row tall.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Art {
    pixels: Vec<Pixel>,
}

impl Art {
    pub fn load(path: &Path) -> Option<Self> {
        let image = image::open(path).ok()?;
        let (width, height) = (u32::from(COLUMNS), u32::from(ROWS) * 2);
        let small = image.resize_exact(width, height, image::imageops::FilterType::Triangle);
        Some(Self {
            pixels: small.to_rgb8().pixels().map(|pixel| pixel.0).collect(),
        })
    }

    /// A cover from pixel rows, top to bottom; for tests.
    pub fn from_pixels(pixels: Vec<Pixel>) -> Self {
        Self { pixels }
    }

    fn pixel(&self, column: u16, row: u16) -> Color {
        let index = usize::from(row) * usize::from(COLUMNS) + usize::from(column);
        let [red, green, blue] = self.pixels.get(index).copied().unwrap_or_default();
        Color::Rgb(red, green, blue)
    }
}

/// Remembers the decoded cover so a redraw does not decode the file again.
#[derive(Default)]
pub struct Cache {
    path: Option<String>,
    art: Option<Art>,
}

impl Cache {
    pub fn get(&mut self, path: Option<&str>) -> Option<&Art> {
        if self.path.as_deref() != path {
            self.path = path.map(str::to_owned);
            self.art = path.and_then(|path| Art::load(Path::new(path)));
        }
        self.art.as_ref()
    }

    /// Fixes the cover for tests, which have no image file.
    pub fn pin(&mut self, path: &str, art: Art) {
        self.path = Some(path.to_owned());
        self.art = Some(art);
    }
}

/// Draws the cover, or a faint placeholder of the same size so the layout never shifts.
pub(super) fn draw(frame: &mut Frame, area: Rect, art: Option<&Art>) {
    let buffer = frame.buffer_mut();
    let area = area.intersection(*buffer.area());
    for row in 0..area.height.min(ROWS) {
        for column in 0..area.width.min(COLUMNS) {
            let Some(cell) = buffer.cell_mut((area.x + column, area.y + row)) else {
                continue;
            };
            match art {
                Some(art) => {
                    cell.set_symbol("▀");
                    cell.set_fg(art.pixel(column, row * 2));
                    cell.set_bg(art.pixel(column, row * 2 + 1));
                }
                None => {
                    cell.set_symbol("░");
                    cell.set_style(super::theme::faint());
                }
            }
        }
    }
}
