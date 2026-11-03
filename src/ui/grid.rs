//! Grid layout math for the tile view: columns, visible rows, scrolling and
//! per-tile rectangles. Pure functions; no drawing.

use ratatui::layout::Rect;

use crate::config::ViewMode;
use crate::ui::tile::{tile_size, ViewKind};

/// `ViewMode::Auto` picks view A (compact) from this many sessions on, B below.
pub const AUTO_VIEW_A_FROM: usize = 7;

/// Concrete tile layout for the configured mode and current session count.
pub fn resolve_view(cfg_view: ViewMode, session_count: usize) -> ViewKind {
    match cfg_view {
        ViewMode::A => ViewKind::A,
        ViewMode::B => ViewKind::B,
        ViewMode::Auto if session_count >= AUTO_VIEW_A_FROM => ViewKind::A,
        ViewMode::Auto => ViewKind::B,
    }
}

/// Tile columns that fit in `width` (each tile at least its minimum width); never 0.
pub fn compute_cols(width: u16, view: ViewKind) -> usize {
    let (w, _) = tile_size(view);
    (usize::from(width) / usize::from(w.max(1))).max(1)
}

/// Full tile rows that fit in `height`; never 0 (a cut-off row is still drawn).
pub fn visible_rows(height: u16, view: ViewKind) -> usize {
    let (_, h) = tile_size(view);
    (usize::from(height) / usize::from(h.max(1))).max(1)
}

/// Rows needed for `n` tiles in `cols` columns.
pub fn row_count(n: usize, cols: usize) -> usize {
    n.div_ceil(cols.max(1))
}

/// New scroll offset (in rows) so that `selected_row` is inside the window of
/// `visible` rows, moving as little as possible and never past the end.
pub fn scroll_to_show(scroll: usize, selected_row: usize, visible: usize, total_rows: usize) -> usize {
    let visible = visible.max(1);
    let mut s = scroll.min(total_rows.saturating_sub(visible));
    if selected_row < s {
        s = selected_row;
    } else if selected_row >= s + visible {
        s = selected_row + 1 - visible;
    }
    s
}

/// Rectangle of the tile at (`col`, `row` within the visible window) inside
/// `area`. Leftover width is spread across columns; heights are the minimum
/// tile height, cut at the bottom of `area`.
pub fn cell_rect(area: Rect, cols: usize, col: usize, row: usize, view: ViewKind) -> Rect {
    let cols = cols.max(1) as u32;
    let col = (col as u32).min(cols - 1);
    let width = u32::from(area.width);
    let x0 = (col * width / cols) as u16;
    let x1 = ((col + 1) * width / cols) as u16;
    let (_, tile_h) = tile_size(view);
    let y_off = (row as u64 * u64::from(tile_h)).min(u64::from(area.height)) as u16;
    let height = tile_h.min(area.height - y_off);
    Rect::new(area.x + x0, area.y + y_off, x1 - x0, height)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_view_auto_thresholds() {
        assert_eq!(resolve_view(ViewMode::Auto, 0), ViewKind::B);
        assert_eq!(resolve_view(ViewMode::Auto, 6), ViewKind::B);
        assert_eq!(resolve_view(ViewMode::Auto, 7), ViewKind::A);
        assert_eq!(resolve_view(ViewMode::Auto, 30), ViewKind::A);
        assert_eq!(resolve_view(ViewMode::A, 1), ViewKind::A);
        assert_eq!(resolve_view(ViewMode::B, 30), ViewKind::B);
    }

    #[test]
    fn compute_cols_never_zero() {
        for view in [ViewKind::A, ViewKind::B] {
            for w in [0u16, 1, 5, 23, 27] {
                assert_eq!(compute_cols(w, view), 1, "{view:?} w={w}");
            }
        }
        assert_eq!(compute_cols(84, ViewKind::A), 3);
        assert_eq!(compute_cols(111, ViewKind::A), 3);
        assert_eq!(compute_cols(112, ViewKind::A), 4);
        assert_eq!(compute_cols(48, ViewKind::B), 2);
        assert_eq!(compute_cols(u16::MAX, ViewKind::B), u16::MAX as usize / 24);
    }

    #[test]
    fn visible_rows_never_zero() {
        assert_eq!(visible_rows(0, ViewKind::A), 1);
        assert_eq!(visible_rows(5, ViewKind::A), 1);
        assert_eq!(visible_rows(18, ViewKind::A), 3);
        assert_eq!(visible_rows(29, ViewKind::B), 2);
        assert_eq!(visible_rows(30, ViewKind::B), 3);
    }

    #[test]
    fn row_count_rounds_up() {
        assert_eq!(row_count(0, 3), 0);
        assert_eq!(row_count(1, 3), 1);
        assert_eq!(row_count(3, 3), 1);
        assert_eq!(row_count(4, 3), 2);
        assert_eq!(row_count(4, 0), 4);
    }

    #[test]
    fn scroll_keeps_selected_visible() {
        // window of 2 rows over 5 rows
        assert_eq!(scroll_to_show(0, 0, 2, 5), 0);
        assert_eq!(scroll_to_show(0, 1, 2, 5), 0);
        assert_eq!(scroll_to_show(0, 2, 2, 5), 1);
        assert_eq!(scroll_to_show(0, 4, 2, 5), 3);
        assert_eq!(scroll_to_show(3, 1, 2, 5), 1);
        assert_eq!(scroll_to_show(2, 3, 2, 5), 2);
        // stale scroll past the end is pulled back
        assert_eq!(scroll_to_show(9, 0, 2, 1), 0);
        assert_eq!(scroll_to_show(4, 4, 3, 5), 2);
        // everything fits
        assert_eq!(scroll_to_show(1, 0, 10, 3), 0);
        // degenerate visible = 0 behaves like 1
        assert_eq!(scroll_to_show(0, 2, 0, 5), 2);
        for scroll in 0..6 {
            for sel in 0..5 {
                let s = scroll_to_show(scroll, sel, 2, 5);
                assert!(s <= sel && sel < s + 2, "scroll={scroll} sel={sel} -> {s}");
            }
        }
    }

    #[test]
    fn cell_rect_spreads_leftover_width() {
        let area = Rect::new(2, 1, 100, 40);
        let cols = compute_cols(area.width, ViewKind::A); // 3
        let rects: Vec<Rect> = (0..cols).map(|c| cell_rect(area, cols, c, 0, ViewKind::A)).collect();
        assert_eq!(rects.iter().map(|r| r.width).sum::<u16>(), 100);
        assert!(rects.iter().all(|r| r.width >= 28 && r.height == 6 && r.y == 1));
        assert_eq!(rects[0].x, 2);
        assert_eq!(rects[2].x + rects[2].width, 102);
        assert_eq!(cell_rect(area, cols, 1, 2, ViewKind::A).y, 1 + 12);
    }

    #[test]
    fn cell_rect_clips_to_area() {
        let area = Rect::new(0, 0, 10, 3);
        let r = cell_rect(area, 1, 0, 0, ViewKind::B);
        assert_eq!(r, Rect::new(0, 0, 10, 3));
        let below = cell_rect(area, 1, 0, 5, ViewKind::B);
        assert_eq!(below.height, 0);
    }
}
