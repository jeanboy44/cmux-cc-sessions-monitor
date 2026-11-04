//! Writes a spirit cell grid into a ratatui buffer.
//!
//! Only glyphs and foreground colours are written; the background of every
//! cell is left as it is (spirits never paint a background).

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;

use crate::spirit::SpiritGrid;

/// Draws `grid` with its top-left at `area`'s top-left, clipped to `area` and
/// to the buffer. Blank cells (`' '` without a colour) leave the buffer as is.
pub fn draw_grid(buf: &mut Buffer, area: Rect, grid: &SpiritGrid) {
    let area = area.intersection(buf.area);
    if area.is_empty() {
        return;
    }
    let cols = grid.cols.min(area.width);
    let rows = grid.rows.min(area.height);
    for row in 0..rows {
        for col in 0..cols {
            let Some(c) = grid.cells.get(usize::from(row) * usize::from(grid.cols) + usize::from(col)) else {
                return;
            };
            if c.ch == ' ' && c.fg.is_none() {
                continue;
            }
            let cell = &mut buf[(area.x + col, area.y + row)];
            cell.set_char(c.ch);
            if let Some((r, g, b)) = c.fg {
                cell.set_fg(Color::Rgb(r, g, b));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spirit::Cell;
    use ratatui::style::Style;

    fn grid(cols: u16, rows: u16) -> SpiritGrid {
        let cells = (0..cols * rows).map(|i| Cell { ch: if i % 2 == 0 { '\u{28ff}' } else { ' ' }, fg: (i % 2 == 0).then_some((10, 20, 30)) }).collect();
        SpiritGrid { cols, rows, cells }
    }

    #[test]
    fn draw_grid_clips_and_never_panics() {
        let g = grid(11, 5);
        let mut buf = Buffer::empty(Rect::new(0, 0, 6, 3));
        for area in [Rect::new(0, 0, 0, 0), Rect::new(0, 0, 1, 1), Rect::new(5, 2, 11, 5), Rect::new(50, 50, 11, 5), Rect::new(u16::MAX - 2, u16::MAX - 2, 11, 5)] {
            draw_grid(&mut buf, area, &g);
        }
        assert_eq!(buf[(0, 0)].symbol(), "\u{28ff}", "1x1 area drew one cell");
        assert_eq!(buf[(1, 0)].symbol(), " ", "clipped to the 1x1 area");
        assert_eq!(buf[(5, 2)].symbol(), "\u{28ff}", "clipped at the buffer edge");
        // empty and inconsistent grids
        draw_grid(&mut buf, Rect::new(0, 0, 6, 3), &SpiritGrid { cols: 0, rows: 0, cells: vec![] });
        draw_grid(&mut buf, Rect::new(0, 0, 6, 3), &SpiritGrid { cols: 4, rows: 4, cells: vec![] });
    }

    #[test]
    fn draw_grid_writes_fg_only_and_skips_blank_cells() {
        let bg = Color::Rgb(1, 2, 3);
        let mut buf = Buffer::empty(Rect::new(0, 0, 4, 2));
        buf.set_style(buf.area, Style::new().bg(bg).fg(Color::Red));
        buf[(1, 0)].set_char('x');
        draw_grid(&mut buf, Rect::new(0, 0, 4, 2), &grid(2, 1));
        assert_eq!((buf[(0, 0)].symbol(), buf[(0, 0)].fg, buf[(0, 0)].bg), ("\u{28ff}", Color::Rgb(10, 20, 30), bg));
        assert_eq!((buf[(1, 0)].symbol(), buf[(1, 0)].fg), ("x", Color::Red), "blank cell leaves the buffer untouched");
    }
}
