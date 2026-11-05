//! Grid layout math for the card view: columns and per-card rectangles.
//! Rows, window groups and scrolling live in [`crate::ui::groups`]. Pure
//! functions; no drawing.

use ratatui::layout::Rect;

use crate::config::CardLayout;
use crate::ui::card::card_size;
use crate::ui::groups::Row;

/// Horizontal gutter between cards. A terminal cell is about twice as tall
/// as wide, so [`GAP_X`] columns and [`GAP_Y`] rows look the same size.
pub const GAP_X: u16 = 2;
pub const GAP_Y: u16 = 1;

/// Height of each layout row, gutter included: [`GAP_Y`] after every card
/// row but the last. `card_h` 1 (compact lines) has no gutter.
pub fn row_heights(rows: &[Row], card_h: u16) -> Vec<u16> {
    let gap = if card_h > 1 { GAP_Y } else { 0 };
    let last = rows.len().saturating_sub(1);
    rows.iter()
        .enumerate()
        .map(|(i, r)| match r {
            Row::Divider { .. } => 1,
            Row::Tiles { .. } if i < last => card_h + gap,
            Row::Tiles { .. } => card_h,
        })
        .collect()
}

/// Height of each layout row without its gutter: what must fit on screen,
/// since the gutter of the last row shown may fall off the bottom.
pub fn solid_heights(rows: &[Row], card_h: u16) -> Vec<u16> {
    rows.iter().map(|r| if matches!(r, Row::Divider { .. }) { 1 } else { card_h }).collect()
}

/// Card columns that fit in `width`, [`GAP_X`] apart (each card at least as
/// wide as a card of `layout`, see [`card_size`]); never 0.
pub fn card_cols(width: u16, layout: CardLayout) -> usize {
    let (w, gap) = (usize::from(card_size(layout).0), usize::from(GAP_X));
    ((usize::from(width) + gap) / (w + gap)).max(1)
}

/// Rectangle of the card at (`col`, `row` within the visible window) inside
/// `area`, [`GAP_X`] between neighbours and none at the outer edges. Standard
/// cards keep their size (a golden rectangle) and sit at the left; large
/// cards fill the row, leftover width spread evenly (widths differ by at most
/// one). When the gaps would make a card narrower than [`card_size`] the
/// cards touch instead. Heights are the card height of `layout`, cut at the
/// bottom of `area`.
pub fn card_rect(area: Rect, cols: usize, col: usize, row: usize, layout: CardLayout) -> Rect {
    let (card_w, card_h) = card_size(layout);
    let cols = cols.max(1) as u32;
    let col = (col as u32).min(cols - 1);
    let width = u32::from(area.width);
    let need = cols * u32::from(card_w) + (cols - 1) * u32::from(GAP_X);
    let gap = if cols > 1 && width >= need { u32::from(GAP_X) } else { 0 };
    let (x0, x1) = if layout == CardLayout::Standard && width >= need {
        let x0 = col * (u32::from(card_w) + gap);
        (x0, x0 + u32::from(card_w))
    } else {
        (col * (width + gap) / cols, (col + 1) * (width + gap) / cols - gap)
    };
    let y_off = (row as u64 * u64::from(card_h)).min(u64::from(area.height)) as u16;
    let height = card_h.min(area.height - y_off);
    Rect::new(area.x + x0 as u16, area.y + y_off, (x1 - x0) as u16, height)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::card::{CARD_H, CARD_W, LARGE_H, LARGE_W};

    #[test]
    fn standard_cards_keep_their_size_and_sit_left() {
        let area = Rect::new(4, 2, 100, CARD_H);
        let (a, b) = (card_rect(area, 2, 0, 0, CardLayout::Standard), card_rect(area, 2, 1, 0, CardLayout::Standard));
        assert_eq!((a.x, a.width), (4, CARD_W));
        assert_eq!((b.x, b.width), (4 + CARD_W + GAP_X, CARD_W));
    }

    #[test]
    fn cards_get_equal_two_column_gaps_and_large_cards_fill_the_row() {
        let area = Rect::new(4, 2, 100, 6);
        assert_eq!(card_cols(2 * CARD_W + GAP_X - 1, CardLayout::Standard), 1);
        assert_eq!(card_cols(2 * CARD_W + GAP_X, CardLayout::Standard), 2);
        assert_eq!(card_cols(3 * CARD_W + 2 * GAP_X, CardLayout::Standard), 3);
        assert_eq!(card_cols(94, CardLayout::Large), 2);
        for layout in [CardLayout::Standard, CardLayout::Large] {
            let rects: Vec<Rect> = (0..2).map(|c| card_rect(area, 2, c, 0, layout)).collect();
            assert_eq!(rects[0].x, area.x);
            assert_eq!(rects[0].right() + GAP_X, rects[1].x);
            assert!(rects[0].width.abs_diff(rects[1].width) <= 1);
        }
        assert_eq!(card_rect(area, 2, 1, 0, CardLayout::Large).right(), area.right());
        let three: Vec<Rect> = (0..3).map(|c| card_rect(Rect::new(0, 0, 143, 10), 3, c, 0, CardLayout::Large)).collect();
        assert!(three.windows(2).all(|w| w[0].right() + GAP_X == w[1].x), "{three:?}");
        assert_eq!(three[2].right(), 143);
    }

    #[test]
    fn card_rows_are_a_gap_apart_and_the_last_row_has_no_trailing_gap() {
        let t = || Row::Tiles { start: 0, end: 1 };
        let d = || Row::Divider { label: "w".into(), count: 1 };
        assert_eq!(row_heights(&[t(), t(), t()], 6), vec![7, 7, 6]);
        assert_eq!(row_heights(&[d(), t(), d(), t()], 6), vec![1, 7, 1, 6], "a group heading sits on its cards");
        assert_eq!(row_heights(&[t()], 1), vec![1], "compact lines have no gap");
        assert_eq!(row_heights(&[t(), t()], 1), vec![1, 1]);
    }

    #[test]
    fn card_cols_scales_with_width_per_layout() {
        use CardLayout::{Large, Standard};
        assert_eq!((CARD_W, CARD_H), (29, 9));
        assert_eq!((LARGE_W, LARGE_H), (46, 10));
        for (w, std, large) in [(60u16, 2usize, 1usize), (74, 2, 1), (94, 3, 2), (112, 3, 2), (120, 3, 2), (142, 4, 3), (150, 4, 3)] {
            assert_eq!(card_cols(w, Standard), std, "standard w={w}");
            assert_eq!(card_cols(w, Large), large, "large w={w}");
        }
        for w in [0u16, 1, 10, CARD_W - 1, LARGE_W - 1] {
            assert_eq!(card_cols(w, Standard), 1, "never 0: w={w}");
            assert_eq!(card_cols(w, Large), 1, "never 0: w={w}");
        }
        assert_eq!(card_cols(u16::MAX, Standard), (u16::MAX as usize + 2) / usize::from(CARD_W + GAP_X));
        assert_eq!(card_cols(u16::MAX, Large), (u16::MAX as usize + 2) / usize::from(LARGE_W + GAP_X));
    }

    #[test]
    fn card_rect_lays_out_the_row_and_stacks_rows() {
        let area = Rect::new(2, 1, 160, 60);
        for (layout, cols, h) in [(CardLayout::Standard, 5, CARD_H), (CardLayout::Large, 3, LARGE_H)] {
            assert_eq!(card_cols(area.width, layout), cols);
            let (w, _) = card_size(layout);
            let rects: Vec<Rect> = (0..cols).map(|c| card_rect(area, cols, c, 0, layout)).collect();
            assert!(rects.iter().all(|r| r.width >= w && r.height == h && r.y == 1), "{layout:?}: {rects:?}");
            assert_eq!(rects[0].x, 2);
            let span = if layout == CardLayout::Large { area.width } else { cols as u16 * w + (cols as u16 - 1) * GAP_X };
            assert_eq!(rects[cols - 1].right(), 2 + span);
            assert_eq!(card_rect(area, cols, 1, 2, layout).y, 1 + 2 * h);
        }
    }

    #[test]
    fn card_rect_clips_to_area() {
        let area = Rect::new(0, 0, 10, 3);
        for layout in [CardLayout::Standard, CardLayout::Large] {
            assert_eq!(card_rect(area, 1, 0, 0, layout), Rect::new(0, 0, 10, 3));
            assert_eq!(card_rect(area, 1, 0, 5, layout).height, 0);
            assert_eq!(card_rect(area, 0, 7, 0, layout), Rect::new(0, 0, 10, 3), "cols 0 and a column past the end clamp");
        }
    }

    #[test]
    fn cards_sit_at_every_width_with_gaps_when_they_fit() {
        let large = CardLayout::Large;
        for layout in [CardLayout::Standard, large] {
            let (card_w, card_h) = card_size(layout);
            for w in card_w..=400u16 {
                let area = Rect::new(3, 2, w, 50);
                let cols = card_cols(w, layout);
                let rects: Vec<Rect> = (0..cols).map(|c| card_rect(area, cols, c, 0, layout)).collect();
                assert_eq!(rects[0].x, 3, "w={w}");
                let span = if layout == large { w } else { cols as u16 * card_w + (cols as u16 - 1) * GAP_X };
                assert_eq!(rects[cols - 1].right(), area.x + span, "w={w}: large cards span the full width, standard ones keep their size");
                for pair in rects.windows(2) {
                    assert_eq!(pair[1].x, pair[0].right() + GAP_X, "w={w}: {rects:?}");
                }
                assert!(rects.iter().all(|r| r.width >= card_w && r.height == card_h), "w={w}: {rects:?}");
                assert!(layout == large || rects.iter().all(|r| r.width == card_w), "w={w}: {rects:?}");
            }
        }
        // tight widths: the cards touch rather than shrink
        let tight: Vec<Rect> = (0..3).map(|c| card_rect(Rect::new(0, 0, 3 * LARGE_W, LARGE_H), 3, c, 0, large)).collect();
        assert_eq!(tight.iter().map(|r| (r.x, r.width)).collect::<Vec<_>>(), vec![(0, LARGE_W), (LARGE_W, LARGE_W), (2 * LARGE_W, LARGE_W)]);
        // tiny and degenerate areas never panic and stay inside the area
        for (w, h) in [(0u16, 0u16), (1, 1), (5, 2), (LARGE_W - 1, LARGE_H - 1), (LARGE_W, LARGE_H), (u16::MAX, 3)] {
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
