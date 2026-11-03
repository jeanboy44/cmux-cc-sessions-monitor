//! Grid layout math for the tile view: columns and per-tile rectangles.
//! Rows, window groups and scrolling live in [`crate::ui::groups`]. Pure
//! functions; no drawing.

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
            for w in [0u16, 1, 5, 23, 25] {
                assert_eq!(compute_cols(w, view), 1, "{view:?} w={w}");
            }
        }
        assert_eq!(compute_cols(77, ViewKind::A), 2);
        assert_eq!(compute_cols(78, ViewKind::A), 3);
        assert_eq!(compute_cols(103, ViewKind::A), 3);
        assert_eq!(compute_cols(104, ViewKind::A), 4);
        assert_eq!(compute_cols(48, ViewKind::B), 2);
        assert_eq!(compute_cols(u16::MAX, ViewKind::B), u16::MAX as usize / 24);
    }

    #[test]
    fn cell_rect_spreads_leftover_width() {
        let area = Rect::new(2, 1, 100, 40);
        let cols = compute_cols(area.width, ViewKind::A); // 3
        let rects: Vec<Rect> = (0..cols).map(|c| cell_rect(area, cols, c, 0, ViewKind::A)).collect();
        assert_eq!(rects.iter().map(|r| r.width).sum::<u16>(), 100);
        assert!(rects.iter().all(|r| r.width >= 26 && r.height == 6 && r.y == 1));
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
