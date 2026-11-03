//! Preview panel: where it goes (picked from the space between HUD and
//! footer) and how it is drawn. Plain chrome: the tiles' thin rounded border
//! and a dim title, no mascot.
//!
//! While the preview is on, the session list turns into ONE column of tiles
//! and the panel takes the rest (master/detail). Placement rule
//! ([`preview_layout`], sizes of the area between HUD and footer):
//! - width ≥ [`SIDE_MIN_WIDTH`] and height ≥ [`SIDE_MIN_HEIGHT`]: list on the
//!   LEFT, `clamp(width/4, 28, 40)` wide, preview on the RIGHT (≥ 42 columns);
//! - else height ≥ [`STACK_MIN_HEIGHT`]: list on TOP, `clamp(height*2/5, 8, 14)`
//!   tall, preview at the BOTTOM (≥ 10 rows);
//! - else no panel: the normal grid, and the footer says the terminal is too small.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Clear, Paragraph};
use ratatui::Frame;

use crate::preview::preview_lines;
use crate::ui::style;
use crate::ui::tile::truncate_to_width;

/// From this width (and [`SIDE_MIN_HEIGHT`]) on, list left / preview right.
pub const SIDE_MIN_WIDTH: u16 = 70;
/// Minimum height for the side-by-side layout.
pub const SIDE_MIN_HEIGHT: u16 = 8;
/// Side-by-side list width bounds (the minimum is above both tile widths).
pub const LIST_MIN_WIDTH: u16 = 28;
pub const LIST_MAX_WIDTH: u16 = 40;
/// Narrower than [`SIDE_MIN_WIDTH`]: from this height on, list top / preview bottom.
pub const STACK_MIN_HEIGHT: u16 = 24;
/// Stacked list height bounds.
pub const LIST_MIN_HEIGHT: u16 = 8;
pub const LIST_MAX_HEIGHT: u16 = 14;
/// The preview never asks cmux for more screen lines than this.
pub const PREVIEW_MAX_READ_LINES: u16 = 80;
/// Footer note while the preview is on but there is no room for it.
pub const PREVIEW_TOO_SMALL: &str = "터미널이 작아 미리보기를 쓸 수 없습니다";
pub const PREVIEW_LOADING: &str = "읽는 중…";
pub const PREVIEW_FAILED: &str = "미리보기를 읽지 못했습니다";
pub const PREVIEW_DONE: &str = "종료된 세션";
pub const PREVIEW_NO_SESSION: &str = "선택한 세션이 없습니다";

/// Session list area, (optional) preview panel area, and whether the list
/// is forced to a single column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PreviewLayout {
    pub list: Rect,
    pub panel: Option<Rect>,
    /// True exactly when there is a panel: the list is one tile per row.
    pub one_column: bool,
}

/// Splits `area` (between HUD and footer) into session list and preview panel.
pub fn preview_layout(area: Rect, enabled: bool) -> PreviewLayout {
    let full = PreviewLayout { list: area, panel: None, one_column: false };
    if !enabled {
        return full;
    }
    if area.width >= SIDE_MIN_WIDTH && area.height >= SIDE_MIN_HEIGHT {
        let w = (area.width / 4).clamp(LIST_MIN_WIDTH, LIST_MAX_WIDTH);
        let list = Rect { width: w, ..area };
        let panel = Rect { x: area.x + w, width: area.width - w, ..area };
        PreviewLayout { list, panel: Some(panel), one_column: true }
    } else if area.height >= STACK_MIN_HEIGHT {
        let h = (u32::from(area.height) * 2 / 5) as u16;
        let h = h.clamp(LIST_MIN_HEIGHT, LIST_MAX_HEIGHT);
        let list = Rect { height: h, ..area };
        let panel = Rect { y: area.y + h, height: area.height - h, ..area };
        PreviewLayout { list, panel: Some(panel), one_column: true }
    } else {
        full
    }
}

/// Screen lines to ask cmux for when the panel is `panel`: its inner height
/// (cmux returns the last non-blank lines), at least 1, at most
/// [`PREVIEW_MAX_READ_LINES`].
pub fn preview_read_lines(panel: Rect) -> u16 {
    panel.height.saturating_sub(2).clamp(1, PREVIEW_MAX_READ_LINES)
}

/// What the panel shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreviewBody {
    /// Raw screen text (escapes are stripped when drawn).
    Screen(String),
    /// One dim line instead of content.
    Note(&'static str),
}

/// Draws the panel in `area` with title ` 미리보기 · <name> `.
pub fn render_preview(f: &mut Frame, area: Rect, name: Option<&str>, body: &PreviewBody) {
    let area = area.intersection(f.area());
    if area.is_empty() {
        return;
    }
    let dim = Style::new().fg(style::TEXT_DIM);
    let title = match name {
        Some(n) => format!(" 미리보기 · {n} "),
        None => " 미리보기 ".to_string(),
    };
    // Corners stay visible: the title gets at most width - 2 columns.
    let title = truncate_to_width(&title, usize::from(area.width.saturating_sub(2)));
    let block = Block::bordered()
        .border_set(style::BORDER)
        .border_style(dim.add_modifier(Modifier::DIM))
        .title(Line::styled(title, dim));
    let inner = block.inner(area);
    f.render_widget(Clear, area);
    f.render_widget(block, area);
    if inner.is_empty() {
        return;
    }
    let (h, w) = (usize::from(inner.height), usize::from(inner.width));
    let lines: Vec<Line> = match body {
        PreviewBody::Screen(text) => {
            preview_lines(text, h, w).into_iter().map(|l| Line::styled(l, Style::new().fg(style::TEXT))).collect()
        }
        PreviewBody::Note(note) => vec![Line::styled(truncate_to_width(note, w), dim)],
    };
    f.render_widget(Paragraph::new(lines), inner);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::test_util::{draw, rows};

    fn r(w: u16, h: u16) -> Rect {
        Rect::new(0, 1, w, h)
    }

    /// The list and the panel never overlap and together cover the area.
    fn assert_split(l: PreviewLayout, area: Rect) {
        let p = l.panel.expect("panel");
        assert!(!l.list.intersects(p), "{l:?}");
        assert_eq!(u32::from(l.list.area()) + u32::from(p.area()), u32::from(area.area()), "{l:?}");
        assert_eq!(l.list.union(p), area, "{l:?}");
        assert!(l.one_column);
    }

    #[test]
    fn preview_layout_left_list_right_preview_when_wide_enough() {
        let l = preview_layout(r(160, 30), true);
        assert_eq!(l.list, Rect::new(0, 1, 40, 30));
        assert_eq!(l.panel, Some(Rect::new(40, 1, 120, 30)));
        assert_split(l, r(160, 30));
        // list width = clamp(width/4, 28, 40)
        assert_eq!(preview_layout(r(70, 8), true).list.width, 28);
        assert_eq!(preview_layout(r(120, 8), true).list.width, 30);
        assert_eq!(preview_layout(r(400, 8), true).list.width, 40);
        // the preview always keeps at least 40 columns
        for w in 70..=300 {
            let l = preview_layout(r(w, 10), true);
            assert!(l.panel.unwrap().width >= 40 && l.list.width >= 28, "w={w}");
            assert_split(l, r(w, 10));
        }
    }

    #[test]
    fn preview_layout_top_list_bottom_preview_when_narrow_but_tall() {
        let l = preview_layout(r(60, 30), true);
        assert_eq!(l.list, Rect::new(0, 1, 60, 12));
        assert_eq!(l.panel, Some(Rect::new(0, 13, 60, 18)));
        assert_split(l, r(60, 30));
        // list height = clamp(height*2/5, 8, 14)
        assert_eq!(preview_layout(r(40, 24), true).list.height, 9);
        assert_eq!(preview_layout(r(40, 100), true).list.height, 14);
        assert_eq!(preview_layout(r(0, 24), true).list.height, 9, "zero width does not panic");
        let huge = preview_layout(r(69, u16::MAX - 1), true);
        assert_eq!(huge.list.height, 14, "no overflow");
    }

    #[test]
    fn preview_layout_boundaries() {
        // width 69/70 (tall enough for both)
        assert_eq!(preview_layout(r(70, 24), true).list.height, 24, "70 wide: side by side");
        assert_eq!(preview_layout(r(69, 24), true).list.width, 69, "69 wide: stacked");
        assert_eq!(preview_layout(r(69, 24), true).panel.map(|p| p.y), Some(1 + 9));
        // height 7/8 when wide
        assert!(preview_layout(r(70, 8), true).panel.is_some_and(|p| p.x == 28));
        assert_eq!(preview_layout(r(70, 7), true).panel, None);
        assert_eq!(preview_layout(r(200, 7), true).panel, None);
        // height 23/24 when narrow
        assert!(preview_layout(r(69, 24), true).panel.is_some());
        assert_eq!(preview_layout(r(69, 23), true).panel, None);
        for (w, h) in [(70, 8), (70, 24), (69, 24), (200, 60), (0, 24), (30, 40)] {
            assert_split(preview_layout(r(w, h), true), r(w, h));
        }
    }

    #[test]
    fn preview_layout_none_when_small() {
        for (w, h) in [(69, 23), (70, 7), (40, 10), (0, 0), (1, 1), (10, 3)] {
            let l = preview_layout(r(w, h), true);
            assert_eq!(l, PreviewLayout { list: r(w, h), panel: None, one_column: false }, "{w}x{h}");
        }
    }

    #[test]
    fn preview_layout_disabled_gives_full_area() {
        for (w, h) in [(200, 50), (60, 30), (10, 3)] {
            assert_eq!(preview_layout(r(w, h), false), PreviewLayout { list: r(w, h), panel: None, one_column: false });
        }
    }

    #[test]
    fn preview_read_lines_follow_panel_height_capped() {
        assert_eq!(preview_read_lines(Rect::new(0, 0, 50, 30)), 28);
        assert_eq!(preview_read_lines(Rect::new(0, 0, 50, 82)), 80);
        assert_eq!(preview_read_lines(Rect::new(0, 0, 50, 200)), 80);
        assert_eq!(preview_read_lines(Rect::new(0, 0, 50, 3)), 1);
        assert_eq!(preview_read_lines(Rect::new(0, 0, 50, 0)), 1);
    }

    #[test]
    fn render_preview_shows_title_and_last_lines() {
        let body = PreviewBody::Screen("old\nline two\n\x1b[1mlast line\x1b[0m\n\n".into());
        let buf = draw(30, 5, |f| render_preview(f, f.area(), Some("proj-a"), &body));
        let screen = rows(&buf);
        assert!(screen[0].starts_with("╭") && screen[0].contains("미리보기 · proj-a"), "{screen:?}");
        assert!(screen[1].contains("old") && screen[3].contains("last line"), "{screen:?}");
        assert!(screen[4].starts_with("╰"));
        // 2 inner rows: only the last two lines
        let screen = rows(&draw(30, 4, |f| render_preview(f, f.area(), Some("proj-a"), &body)));
        assert!(screen[1].contains("line two") && screen[2].contains("last line"), "{screen:?}");
        // notes and long titles
        let note = PreviewBody::Note(PREVIEW_LOADING);
        let screen = rows(&draw(16, 4, |f| render_preview(f, f.area(), Some("아주아주긴세션이름입니다"), &note)));
        assert!(screen[1].contains(PREVIEW_LOADING), "{screen:?}");
        assert!(screen[0].ends_with("╮"), "title stays inside the border: {screen:?}");
    }

    #[test]
    fn render_preview_tiny_areas_do_not_panic() {
        let body = PreviewBody::Screen("프로젝트\nabc".into());
        for (w, h) in [(0, 0), (1, 1), (2, 2), (10, 3), (3, 10)] {
            draw(w.max(1), h.max(1), |f| {
                render_preview(f, Rect::new(0, 0, w, h), Some("x"), &body);
                render_preview(f, Rect::new(0, 0, w, h), None, &PreviewBody::Note(PREVIEW_FAILED));
            });
        }
    }
}
