//! Grouping tiles by cmux window. Pure layout shared by the renderer and the
//! navigation so they can never disagree.
//!
//! Sessions are ordered by their window's position in `cmux tree` (unknown
//! windows last, as `창 ?`), keeping the existing order inside a group. Only
//! when the current sessions span two or more groups is each group headed by
//! one dim divider line (`── 창 2 · 세션 3 ─────`); a single window looks
//! exactly like the ungrouped grid. Every group has its own tile rows with
//! the same column count.

use ratatui::layout::Rect;

use crate::app::Dir;
use crate::model::Session;
use crate::ui::tile::truncate_to_width;

/// Label of the group of sessions whose window is unknown.
pub const UNKNOWN_WINDOW_LABEL: &str = "창 ?";

/// One layout row: a divider (1 terminal row) or a row of tiles
/// (`sessions[start..end]`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row {
    Divider { label: String, count: usize },
    Tiles { start: usize, end: usize },
}

/// Position of a session's window in `window_order`; unknown = last.
fn group_key(s: &Session, window_order: &[String]) -> usize {
    s.window_id
        .as_deref()
        .and_then(|w| window_order.iter().position(|o| o == w))
        .unwrap_or(usize::MAX)
}

/// Stable sort of `sessions` into window groups (tree order, unknown last).
pub fn sort_by_window(sessions: &mut [Session], window_order: &[String]) {
    sessions.sort_by_key(|s| group_key(s, window_order));
}

/// `창 N` (N = position in the tree, from 1), ` (현재)` for the monitor's
/// own window, or [`UNKNOWN_WINDOW_LABEL`].
pub fn window_label(window: Option<&str>, window_order: &[String], current: Option<&str>) -> String {
    let Some((i, w)) = window.and_then(|w| window_order.iter().position(|o| o == w).map(|i| (i, w))) else {
        return UNKNOWN_WINDOW_LABEL.to_string();
    };
    let mark = if current == Some(w) { " (현재)" } else { "" };
    format!("창 {}{mark}", i + 1)
}

/// Rows for `sessions` (already sorted with [`sort_by_window`]) in `cols`
/// columns: tile rows per group, each group headed by a divider when there
/// are at least two groups.
pub fn layout_groups(sessions: &[Session], window_order: &[String], current: Option<&str>, cols: usize) -> Vec<Row> {
    let cols = cols.max(1);
    // Consecutive runs of the same group key.
    let mut groups: Vec<(usize, usize, usize)> = Vec::new(); // (key, start, end)
    for (i, s) in sessions.iter().enumerate() {
        let key = group_key(s, window_order);
        match groups.last_mut() {
            Some((k, _, end)) if *k == key => *end = i + 1,
            _ => groups.push((key, i, i + 1)),
        }
    }
    let labelled = groups.len() >= 2;
    let mut rows = Vec::new();
    for (key, start, end) in groups {
        if labelled {
            let window = window_order.get(key).map(String::as_str);
            rows.push(Row::Divider { label: window_label(window, window_order, current), count: end - start });
        }
        rows.extend((start..end).step_by(cols).map(|s| Row::Tiles { start: s, end: (s + cols).min(end) }));
    }
    rows
}

/// `── <label> · 세션 <count> ───…` exactly `width` columns wide (cut safely).
pub fn divider_text(label: &str, count: usize, width: usize) -> String {
    let head = truncate_to_width(&format!("── {label} · 세션 {count} "), width);
    let used = unicode_width::UnicodeWidthStr::width(head.as_str());
    head + &"─".repeat(width.saturating_sub(used))
}

/// Layout row holding tile `idx`.
pub fn row_of(rows: &[Row], idx: usize) -> Option<usize> {
    rows.iter().position(|r| matches!(r, Row::Tiles { start, end } if (*start..*end).contains(&idx)))
}

/// Selection after a move: h/l stay in the row, j/k go to the next/previous
/// tile row (across dividers and groups) with the column clamped to that
/// row's length; edges clamp.
pub fn move_selection(rows: &[Row], selected: usize, dir: Dir) -> usize {
    let Some(r) = row_of(rows, selected) else { return selected };
    let Row::Tiles { start, end } = rows[r] else { return selected };
    let col = selected - start;
    let land = |row: Option<&Row>| match row {
        Some(Row::Tiles { start, end }) => start + col.min(end - start - 1),
        _ => selected,
    };
    let tiles = |r: &&Row| matches!(r, Row::Tiles { .. });
    match dir {
        Dir::Left if col > 0 => selected - 1,
        Dir::Right if selected + 1 < end => selected + 1,
        Dir::Down => land(rows[r + 1..].iter().find(tiles)),
        Dir::Up => land(rows[..r].iter().rev().find(tiles)),
        _ => selected,
    }
}

/// First visible row so that `sel_row` is fully shown (or at least first),
/// including the divider right above it when it starts a group, moving as
/// little as possible and not scrolling past the end. `heights` are the
/// rows' heights in terminal rows.
pub fn scroll_to_show_rows(rows: &[Row], heights: &[u16], scroll: usize, sel_row: usize, area_h: u16) -> usize {
    let n = heights.len().min(rows.len());
    if n == 0 {
        return 0;
    }
    let sel = sel_row.min(n - 1);
    let sum = |a: usize, b: usize| heights[a..b].iter().map(|h| u32::from(*h)).sum::<u32>();
    let area = u32::from(area_h);
    let divider_above = |i: usize| i > 0 && matches!(rows[i - 1], Row::Divider { .. });
    let mut s = scroll.min(n - 1);
    if sel < s {
        s = if divider_above(sel) { sel - 1 } else { sel };
    }
    while s < sel && sum(s, sel + 1) > area {
        s += 1;
    }
    // Show the group's divider with its first row when it fits.
    if s == sel && divider_above(sel) && sum(sel - 1, sel + 1) <= area {
        s -= 1;
    }
    // Do not leave space unused at the end.
    while s > 0 && sum(s - 1, n) <= area {
        s -= 1;
    }
    s
}

/// Rects of the rows drawn from `scroll` on: rows that fit completely,
/// and at least the first one (cut at the bottom).
pub fn place_rows(heights: &[u16], scroll: usize, area: Rect) -> Vec<(usize, Rect)> {
    let mut out = Vec::new();
    if area.is_empty() {
        return out;
    }
    let bottom = u32::from(area.y) + u32::from(area.height);
    let mut y = u32::from(area.y);
    for (i, h) in heights.iter().enumerate().skip(scroll) {
        let h = u32::from(*h);
        if y + h > bottom {
            if out.is_empty() {
                out.push((i, Rect::new(area.x, y as u16, area.width, (bottom - y) as u16)));
            }
            break;
        }
        out.push((i, Rect::new(area.x, y as u16, area.width, h as u16)));
        y += h;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::fake_session;
    use crate::model::Status;
    use unicode_width::UnicodeWidthStr;

    fn order() -> Vec<String> {
        vec!["W1".to_string(), "W2".to_string()]
    }

    fn sess(id: &str, window: Option<&str>) -> Session {
        let mut s = fake_session(id, Status::Running);
        s.window_id = window.map(String::from);
        s
    }

    fn ids(list: &[Session]) -> Vec<&str> {
        list.iter().map(|s| s.id.as_str()).collect()
    }

    fn dividers(rows: &[Row]) -> Vec<(String, usize)> {
        rows.iter()
            .filter_map(|r| match r {
                Row::Divider { label, count } => Some((label.clone(), *count)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn groups_follow_window_tree_order_and_keep_order_inside() {
        let mut v = vec![sess("a", Some("W2")), sess("b", Some("W1")), sess("c", Some("W2")), sess("d", Some("W1"))];
        sort_by_window(&mut v, &order());
        assert_eq!(ids(&v), ["b", "d", "a", "c"]);
        let rows = layout_groups(&v, &order(), None, 3);
        assert_eq!(
            rows,
            vec![
                Row::Divider { label: "창 1".into(), count: 2 },
                Row::Tiles { start: 0, end: 2 },
                Row::Divider { label: "창 2".into(), count: 2 },
                Row::Tiles { start: 2, end: 4 },
            ]
        );
    }

    #[test]
    fn unknown_window_group_comes_last() {
        let mut v = vec![sess("x", None), sess("a", Some("W2")), sess("gone", Some("W9"))];
        sort_by_window(&mut v, &order());
        assert_eq!(ids(&v), ["a", "x", "gone"]);
        let rows = layout_groups(&v, &order(), None, 4);
        assert_eq!(dividers(&rows), vec![("창 2".into(), 1), (UNKNOWN_WINDOW_LABEL.into(), 2)]);
    }

    #[test]
    fn single_window_has_no_divider() {
        let v = vec![sess("a", Some("W2")), sess("b", Some("W2")), sess("c", Some("W2"))];
        let rows = layout_groups(&v, &order(), None, 2);
        assert_eq!(rows, vec![Row::Tiles { start: 0, end: 2 }, Row::Tiles { start: 2, end: 3 }]);
        // unknown everywhere (tree failed): one plain list too
        let v = vec![sess("a", None), sess("b", None)];
        assert!(dividers(&layout_groups(&v, &[], None, 2)).is_empty());
        assert!(layout_groups(&[], &order(), None, 2).is_empty());
    }

    #[test]
    fn current_window_is_marked() {
        assert_eq!(window_label(Some("W2"), &order(), Some("W2")), "창 2 (현재)");
        assert_eq!(window_label(Some("W1"), &order(), Some("W2")), "창 1");
        assert_eq!(window_label(None, &order(), Some("W2")), UNKNOWN_WINDOW_LABEL);
    }

    #[test]
    fn divider_text_exact_and_truncated() {
        assert_eq!(divider_text("창 2", 3, 24), "── 창 2 · 세션 3 ───────");
        assert_eq!(divider_text("창 2", 3, 24).width(), 24);
        for w in 0..40 {
            let t = divider_text("창 1 (현재)", 12, w);
            assert!(t.width() <= w, "w={w} {t:?}");
        }
        assert_eq!(divider_text("창 2", 3, 0), "");
        assert_eq!(divider_text("창 2", 3, 6), "── 창…");
    }

    /// Two groups: [0 1 2 | 3 4] then [5 6 7 | 8] in 3 columns.
    fn two_groups() -> Vec<Row> {
        let mut v: Vec<Session> = (0..5).map(|i| sess(&format!("a{i}"), Some("W1"))).collect();
        v.extend((0..4).map(|i| sess(&format!("b{i}"), Some("W2"))));
        layout_groups(&v, &order(), None, 3)
    }

    #[test]
    fn j_and_k_cross_group_boundaries_with_column_clamp() {
        let rows = two_groups();
        assert_eq!(move_selection(&rows, 1, Dir::Down), 4, "short last row of group 1");
        assert_eq!(move_selection(&rows, 2, Dir::Down), 4, "column clamped");
        assert_eq!(move_selection(&rows, 4, Dir::Down), 6, "into the next group's first row");
        assert_eq!(move_selection(&rows, 3, Dir::Down), 5);
        assert_eq!(move_selection(&rows, 7, Dir::Down), 8, "clamped to the short last row");
        assert_eq!(move_selection(&rows, 8, Dir::Down), 8, "very last row");
        assert_eq!(move_selection(&rows, 8, Dir::Up), 5);
        assert_eq!(move_selection(&rows, 7, Dir::Up), 4, "back into group 1, column clamped");
        assert_eq!(move_selection(&rows, 5, Dir::Up), 3);
        assert_eq!(move_selection(&rows, 3, Dir::Up), 0);
        assert_eq!(move_selection(&rows, 1, Dir::Up), 1, "very first row");
    }

    #[test]
    fn h_and_l_do_not_cross_rows_or_groups() {
        let rows = two_groups();
        assert_eq!(move_selection(&rows, 4, Dir::Right), 4, "end of group 1");
        assert_eq!(move_selection(&rows, 5, Dir::Left), 5, "start of group 2");
        assert_eq!(move_selection(&rows, 2, Dir::Right), 2);
        assert_eq!(move_selection(&rows, 3, Dir::Left), 3);
        assert_eq!(move_selection(&rows, 6, Dir::Right), 7);
        assert_eq!(move_selection(&rows, 6, Dir::Left), 5);
        // out of range selection never panics
        assert_eq!(move_selection(&rows, 99, Dir::Down), 99);
        assert_eq!(move_selection(&[], 0, Dir::Down), 0);
    }

    fn heights(rows: &[Row], tile_h: u16) -> Vec<u16> {
        rows.iter().map(|r| if matches!(r, Row::Divider { .. }) { 1 } else { tile_h }).collect()
    }

    #[test]
    fn scroll_keeps_selected_visible_with_dividers() {
        let rows = two_groups(); // D T T D T T
        let h = heights(&rows, 6);
        // room for 13 rows: a divider and two tile rows
        for sel in 0..9 {
            let r = row_of(&rows, sel).unwrap();
            for start in 0..rows.len() {
                let s = scroll_to_show_rows(&rows, &h, start, r, 13);
                let placed = place_rows(&h, s, Rect::new(0, 0, 80, 13));
                assert!(placed.iter().any(|(i, rect)| *i == r && rect.height == 6), "sel={sel} start={start} s={s}");
            }
        }
        // moving into group 2 scrolls as little as possible and shows its divider
        let s = scroll_to_show_rows(&rows, &h, 0, 4, 13);
        assert_eq!(s, 2);
        assert!(place_rows(&h, s, Rect::new(0, 0, 80, 13)).iter().any(|(i, _)| *i == 3));
        // a tight area still shows the divider with the group's first row
        assert_eq!(scroll_to_show_rows(&rows, &h, 0, 4, 7), 3);
        // a stale scroll past the end is pulled back to fill the area
        assert_eq!(scroll_to_show_rows(&rows, &h, 5, 5, 13), 3);
        // everything fits: no scroll
        assert_eq!(scroll_to_show_rows(&rows, &h, 2, 5, 100), 0);
    }

    #[test]
    fn placed_tile_rows_never_overlap_dividers() {
        let rows = two_groups();
        let h = heights(&rows, 6);
        let area = Rect::new(2, 1, 90, 40);
        let placed = place_rows(&h, 0, area);
        assert_eq!(placed.len(), rows.len());
        for (i, a) in &placed {
            for (j, b) in &placed {
                if i != j {
                    assert!(!a.intersects(*b), "{i} {a:?} vs {j} {b:?}");
                }
            }
            assert_eq!(a.height, h[*i]);
            assert!(a.y >= area.y && a.bottom() <= area.bottom());
        }
        assert_eq!(placed[0].1, Rect::new(2, 1, 90, 1));
        assert_eq!(placed[1].1, Rect::new(2, 2, 90, 6));
        // too small for even one row: the first is cut, nothing more
        let tiny = place_rows(&h, 1, Rect::new(0, 0, 10, 3));
        assert_eq!(tiny, vec![(1, Rect::new(0, 0, 10, 3))]);
        assert!(place_rows(&h, 0, Rect::new(0, 0, 10, 0)).is_empty());
        assert!(place_rows(&[], 0, area).is_empty());
    }
}
