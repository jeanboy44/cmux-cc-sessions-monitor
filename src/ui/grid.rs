//! Grid layout math for the card view: columns and per-card rectangles.
//! Rows, window groups and scrolling live in [`crate::ui::groups`]. Pure
//! functions; no drawing.

use ratatui::layout::Rect;

use crate::config::CardLayout;
use crate::ui::card::card_size;

/// Card columns that fit in `width` (each card at least as wide as a card of
/// `layout`, see [`card_size`]); never 0.
pub fn card_cols(width: u16, layout: CardLayout) -> usize {
    (usize::from(width) / usize::from(card_size(layout).0)).max(1)
}

/// Rectangle of the card at (`col`, `row` within the visible window) inside
/// `area`. Leftover width is spread across columns; heights are the card
/// height of `layout`, cut at the bottom of `area`.
pub fn cell_rect(area: Rect, cols: usize, col: usize, row: usize, layout: CardLayout) -> Rect {
    let card_h = card_size(layout).1;
    let cols = cols.max(1) as u32;
    let col = (col as u32).min(cols - 1);
    let width = u32::from(area.width);
    let x0 = (col * width / cols) as u16;
    let x1 = ((col + 1) * width / cols) as u16;
    let y_off = (row as u64 * u64::from(card_h)).min(u64::from(area.height)) as u16;
    let height = card_h.min(area.height - y_off);
    Rect::new(area.x + x0, area.y + y_off, x1 - x0, height)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::card::{CARD_H, CARD_W};

    #[test]
    fn card_cols_scales_with_width_per_layout() {
        use CardLayout::{Large, Standard};
        assert_eq!((CARD_W, CARD_H), (20, 10));
        for (w, std, large) in [(60u16, 3usize, 2usize), (100, 5, 3), (150, 7, 5), (120, 6, 4), (40, 2, 1), (39, 1, 1), (56, 2, 2), (55, 2, 1)] {
            assert_eq!(card_cols(w, Standard), std, "standard w={w}");
            assert_eq!(card_cols(w, Large), large, "large w={w}");
        }
        for w in [0u16, 1, 10, 19, 27] {
            assert_eq!(card_cols(w, Standard), 1, "never 0: w={w}");
            assert_eq!(card_cols(w, Large), 1, "never 0: w={w}");
        }
        assert_eq!(card_cols(u16::MAX, Standard), u16::MAX as usize / 20);
        assert_eq!(card_cols(u16::MAX, Large), u16::MAX as usize / 28);
    }

    #[test]
    fn cell_rect_spreads_leftover_width() {
        let area = Rect::new(2, 1, 160, 60);
        for (layout, cols, h) in [(CardLayout::Standard, 8, 10u16), (CardLayout::Large, 5, 13)] {
            assert_eq!(card_cols(area.width, layout), cols);
            let (w, _) = card_size(layout);
            let rects: Vec<Rect> = (0..cols).map(|c| cell_rect(area, cols, c, 0, layout)).collect();
            assert_eq!(rects.iter().map(|r| r.width).sum::<u16>(), 160);
            assert!(rects.iter().all(|r| r.width >= w && r.height == h && r.y == 1), "{layout:?}: {rects:?}");
            assert_eq!(rects[0].x, 2);
            assert_eq!(rects[cols - 1].x + rects[cols - 1].width, 162);
            assert_eq!(cell_rect(area, cols, 1, 2, layout).y, 1 + 2 * h);
        }
    }

    #[test]
    fn cell_rect_clips_to_area() {
        let area = Rect::new(0, 0, 10, 3);
        for layout in [CardLayout::Standard, CardLayout::Large] {
            assert_eq!(cell_rect(area, 1, 0, 0, layout), Rect::new(0, 0, 10, 3));
            assert_eq!(cell_rect(area, 1, 0, 5, layout).height, 0);
            assert_eq!(cell_rect(area, 0, 7, 0, layout), Rect::new(0, 0, 10, 3), "cols 0 and a column past the end clamp");
        }
    }
}
