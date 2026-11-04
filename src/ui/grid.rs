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

/// Rectangle of the card itself at (`col`, `row`). Standard cards sit in
/// their [`cell_rect`] (drawn centred inside it). Large cards fill the row
/// instead: a 1-column gap between neighbours, none at the outer edges, the
/// last card takes the rounding remainder; when the gaps would make a card
/// narrower than [`card_size`] the cards touch instead. Same heights and
/// clipping as [`cell_rect`].
pub fn card_rect(area: Rect, cols: usize, col: usize, row: usize, layout: CardLayout) -> Rect {
    let cell = cell_rect(area, cols, col, row, layout);
    if layout != CardLayout::Large {
        return cell;
    }
    let card_w = u32::from(card_size(layout).0);
    let cols = cols.max(1) as u32;
    let col = (col as u32).min(cols - 1);
    let width = u32::from(area.width);
    let gaps = cols - 1;
    let gap = u32::from(gaps > 0 && width >= cols * card_w + gaps);
    let base = (width - gap * gaps) / cols;
    let x = col * (base + gap);
    let w = if col + 1 == cols { width - x } else { base };
    Rect { x: area.x + x as u16, width: w as u16, ..cell }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::card::{CARD_H, CARD_W};

    #[test]
    fn card_cols_scales_with_width_per_layout() {
        use CardLayout::{Large, Standard};
        assert_eq!((CARD_W, CARD_H), (20, 10));
        for (w, std, large) in [(60u16, 3usize, 1usize), (64, 3, 2), (96, 4, 3), (100, 5, 3), (111, 5, 3), (120, 6, 3), (150, 7, 4), (40, 2, 1), (39, 1, 1), (63, 3, 1)] {
            assert_eq!(card_cols(w, Standard), std, "standard w={w}");
            assert_eq!(card_cols(w, Large), large, "large w={w}");
        }
        for w in [0u16, 1, 10, 19, 31] {
            assert_eq!(card_cols(w, Standard), 1, "never 0: w={w}");
            assert_eq!(card_cols(w, Large), 1, "never 0: w={w}");
        }
        assert_eq!(card_cols(u16::MAX, Standard), u16::MAX as usize / 20);
        assert_eq!(card_cols(u16::MAX, Large), u16::MAX as usize / 32);
    }

    #[test]
    fn cell_rect_spreads_leftover_width() {
        let area = Rect::new(2, 1, 160, 60);
        for (layout, cols, h) in [(CardLayout::Standard, 8, 10u16), (CardLayout::Large, 5, 14)] {
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

    /// Large cards fill their cell: 1-column gaps between neighbours, none
    /// at the outer edges, the last card takes the rounding remainder.
    #[test]
    fn card_rect_fills_cells_with_one_column_gaps() {
        let large = CardLayout::Large;
        let area = Rect::new(0, 1, 111, 40);
        assert_eq!(card_cols(111, large), 3);
        let rects: Vec<Rect> = (0..3).map(|c| card_rect(area, 3, c, 0, large)).collect();
        assert_eq!(rects, vec![Rect::new(0, 1, 36, 14), Rect::new(37, 1, 36, 14), Rect::new(74, 1, 37, 14)]);
        assert_eq!(card_rect(area, 3, 2, 1, large), Rect::new(74, 15, 37, 14));
        // every width: full span, gaps of exactly 1 when they fit (else 0), never narrower than the card
        for w in 32..=400u16 {
            let area = Rect::new(3, 2, w, 50);
            let cols = card_cols(w, large);
            let rects: Vec<Rect> = (0..cols).map(|c| card_rect(area, cols, c, 0, large)).collect();
            assert_eq!(rects[0].x, 3, "w={w}");
            assert_eq!(rects[cols - 1].right(), area.right(), "w={w}: the cards span the full width");
            let gap = u16::from(w >= cols as u16 * 32 + cols as u16 - 1 && cols > 1);
            for pair in rects.windows(2) {
                assert_eq!(pair[1].x, pair[0].right() + gap, "w={w}: {rects:?}");
            }
            assert!(rects.iter().all(|r| r.width >= 32 && r.height == 14), "w={w}: {rects:?}");
            let widths: Vec<u16> = rects.iter().map(|r| r.width).collect();
            assert!(widths[..cols - 1].iter().all(|&x| x == widths[0]) && widths[cols - 1] >= widths[0], "w={w}: {widths:?}");
        }
        // tight widths: the cards touch rather than shrink
        let tight: Vec<Rect> = (0..3).map(|c| card_rect(Rect::new(0, 0, 96, 14), 3, c, 0, large)).collect();
        assert_eq!(tight.iter().map(|r| (r.x, r.width)).collect::<Vec<_>>(), vec![(0, 32), (32, 32), (64, 32)]);
        // the standard layout keeps its cells (cards centred inside)
        for c in 0..5 {
            assert_eq!(card_rect(Rect::new(2, 1, 111, 40), 5, c, 1, CardLayout::Standard), cell_rect(Rect::new(2, 1, 111, 40), 5, c, 1, CardLayout::Standard));
        }
        // tiny and degenerate areas never panic and stay inside the area
        for (w, h) in [(0u16, 0u16), (1, 1), (5, 2), (31, 13), (32, 14), (u16::MAX, 3)] {
            let area = Rect::new(0, 0, w, h);
            for cols in [0usize, 1, 3, 9] {
                for c in [0usize, 1, 8, 20] {
                    let r = card_rect(area, cols, c, 2, large);
                    assert!(r.x >= area.x && r.right() <= area.right() && r.bottom() <= area.bottom(), "{w}x{h} cols={cols} c={c}: {r:?}");
                }
            }
        }
    }
}
