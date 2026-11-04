//! Grid layout math for the card view: columns and per-card rectangles.
//! Rows, window groups and scrolling live in [`crate::ui::groups`]. Pure
//! functions; no drawing.

use ratatui::layout::Rect;

use crate::ui::card::{CARD_H, CARD_W};

/// Card columns that fit in `width` (each card at least [`CARD_W`] wide); never 0.
pub fn card_cols(width: u16) -> usize {
    (usize::from(width) / usize::from(CARD_W)).max(1)
}

/// Rectangle of the card at (`col`, `row` within the visible window) inside
/// `area`. Leftover width is spread across columns; heights are [`CARD_H`],
/// cut at the bottom of `area`.
pub fn cell_rect(area: Rect, cols: usize, col: usize, row: usize) -> Rect {
    let cols = cols.max(1) as u32;
    let col = (col as u32).min(cols - 1);
    let width = u32::from(area.width);
    let x0 = (col * width / cols) as u16;
    let x1 = ((col + 1) * width / cols) as u16;
    let y_off = (row as u64 * u64::from(CARD_H)).min(u64::from(area.height)) as u16;
    let height = CARD_H.min(area.height - y_off);
    Rect::new(area.x + x0, area.y + y_off, x1 - x0, height)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn card_cols_scales_with_width() {
        assert_eq!(CARD_W, 24);
        assert_eq!(card_cols(100), 4);
        assert_eq!(card_cols(60), 2);
        assert_eq!(card_cols(48), 2);
        assert_eq!(card_cols(47), 1);
        assert_eq!(card_cols(150), 6);
        for w in [0u16, 1, 10, 23] {
            assert_eq!(card_cols(w), 1, "never 0: w={w}");
        }
        assert_eq!(card_cols(u16::MAX), u16::MAX as usize / CARD_W as usize);
    }

    #[test]
    fn cell_rect_spreads_leftover_width() {
        let area = Rect::new(2, 1, 160, 60);
        let cols = card_cols(area.width); // 6
        let rects: Vec<Rect> = (0..cols).map(|c| cell_rect(area, cols, c, 0)).collect();
        assert_eq!(rects.iter().map(|r| r.width).sum::<u16>(), 160);
        assert!(rects.iter().all(|r| r.width >= CARD_W && r.height == CARD_H && r.y == 1));
        assert_eq!(rects[0].x, 2);
        assert_eq!(cols, 6);
        assert_eq!(rects[5].x + rects[5].width, 162);
        assert_eq!(cell_rect(area, cols, 1, 2).y, 1 + 2 * CARD_H);
    }

    #[test]
    fn cell_rect_clips_to_area() {
        let area = Rect::new(0, 0, 10, 3);
        assert_eq!(cell_rect(area, 1, 0, 0), Rect::new(0, 0, 10, 3));
        assert_eq!(cell_rect(area, 1, 0, 5).height, 0);
        assert_eq!(cell_rect(area, 0, 7, 0), Rect::new(0, 0, 10, 3), "cols 0 and a column past the end clamp");
    }
}
