//! Preview panel: where it goes (picked from the space between HUD and
//! footer) and how it is drawn. Plain chrome: the tiles' thin rounded border
//! and a dim title, no scene.
//!
//! While the preview is on, the session list keeps full spirit cards and the
//! panel takes the rest (master/detail). Placement rule ([`preview_layout`],
//! sizes of the area between HUD and footer):
//! - width ≥ [`SIDE_MIN_WIDTH`] and height ≥ [`SIDE_MIN_HEIGHT`]: list on the
//!   LEFT, `clamp(width/5, 26, 34)` wide, ONE column of cards stretched to that
//!   width (user decision: the card grows to the right by the reserved column),
//!   preview on the RIGHT (≥ 44 columns);
//! - else height ≥ [`STACK_MIN_HEIGHT`]: list on TOP, `clamp(height*2/5, 9, 10)`
//!   tall = one row of the usual 20x9 card grid (+ a group divider line),
//!   preview at the BOTTOM (≥ 15 rows);
//! - else no panel: the normal grid, and the footer says the terminal is too small.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Clear, Paragraph};
use ratatui::Frame;

use crate::preview::preview_lines;
use crate::ui::card::{CARD_H, CARD_W};
use crate::ui::style;
use crate::ui::style::truncate_to_width;

/// From this width (and [`SIDE_MIN_HEIGHT`]) on, list left / preview right.
pub const SIDE_MIN_WIDTH: u16 = 70;
/// Minimum height for the side-by-side layout.
pub const SIDE_MIN_HEIGHT: u16 = 8;
/// Side-by-side list width bounds; the one-column cards stretch to this
/// width (always ≥ [`CARD_W`]; names are cut with `…` inside the card).
pub const LIST_MIN_WIDTH: u16 = 26;
pub const LIST_MAX_WIDTH: u16 = 34;
const _: () = assert!(LIST_MIN_WIDTH >= CARD_W, "a full card fits the side list");
/// Side-by-side list width = width / this, clamped to the bounds above.
pub const LIST_WIDTH_DIVISOR: u16 = 5;
/// Narrower than [`SIDE_MIN_WIDTH`]: from this height on, list top / preview bottom.
pub const STACK_MIN_HEIGHT: u16 = 24;
/// Stacked list height bounds: one row of full cards, plus one line for a
/// window group divider when there is room.
pub const LIST_MIN_HEIGHT: u16 = CARD_H;
pub const LIST_MAX_HEIGHT: u16 = CARD_H + 1;
/// The preview never asks cmux for more screen lines than this.
pub const PREVIEW_MAX_READ_LINES: u16 = 80;
/// Footer note while the preview is on but there is no room for it.
pub const PREVIEW_TOO_SMALL: &str = "터미널이 작아 미리보기를 쓸 수 없습니다";
pub const PREVIEW_LOADING: &str = "읽는 중…";
pub const PREVIEW_FAILED: &str = "미리보기를 읽지 못했습니다";
pub const PREVIEW_DONE: &str = "종료된 세션";
pub const PREVIEW_NO_SESSION: &str = "선택한 세션이 없습니다";

/// Session list area, (optional) preview panel area, and whether the list
/// is one column of cards stretched to its width.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PreviewLayout {
    pub list: Rect,
    pub panel: Option<Rect>,
    /// True exactly in the side-by-side layout: one stretched card per row.
    /// The stacked strip keeps the usual grid.
    pub one_column: bool,
}

/// Splits `area` (between HUD and footer) into session list and preview panel.
pub fn preview_layout(area: Rect, enabled: bool) -> PreviewLayout {
    let full = PreviewLayout { list: area, panel: None, one_column: false };
    if !enabled {
        return full;
    }
    if area.width >= SIDE_MIN_WIDTH && area.height >= SIDE_MIN_HEIGHT {
        let w = (area.width / LIST_WIDTH_DIVISOR).clamp(LIST_MIN_WIDTH, LIST_MAX_WIDTH);
        let list = Rect { width: w, ..area };
        let panel = Rect { x: area.x + w, width: area.width - w, ..area };
        PreviewLayout { list, panel: Some(panel), one_column: true }
    } else if area.height >= STACK_MIN_HEIGHT {
        let h = (u32::from(area.height) * 2 / 5) as u16;
        let h = h.clamp(LIST_MIN_HEIGHT, LIST_MAX_HEIGHT);
        let list = Rect { height: h, ..area };
        let panel = Rect { y: area.y + h, height: area.height - h, ..area };
        PreviewLayout { list, panel: Some(panel), one_column: false }
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
        assert_eq!(l.one_column, p.x > area.x, "one column of stretched cards exactly in the side layout: {l:?}");
    }

    #[test]
    fn list_bounds_hold_full_cards() {
        use crate::ui::card::{CARD_H, CARD_W};
        assert!(LIST_MIN_WIDTH >= CARD_W && LIST_MAX_WIDTH >= LIST_MIN_WIDTH);
        assert_eq!((LIST_MIN_WIDTH, LIST_MAX_WIDTH), (26, 34), "the column the stretched card fills");
        assert_eq!((LIST_MIN_HEIGHT, LIST_MAX_HEIGHT), (CARD_H, CARD_H + 1), "one row of full cards (+ a divider)");
        assert!(STACK_MIN_HEIGHT - LIST_MIN_HEIGHT >= 10, "the stacked preview keeps at least 10 rows");
    }

    #[test]
    fn preview_layout_left_list_right_preview_when_wide_enough() {
        let l = preview_layout(r(160, 30), true);
        assert_eq!(l.list, Rect::new(0, 1, 32, 30));
        assert_eq!(l.panel, Some(Rect::new(32, 1, 128, 30)));
        assert_split(l, r(160, 30));
        // list width = clamp(width/5, 26, 34)
        assert_eq!(preview_layout(r(70, 8), true).list.width, 26);
        assert_eq!(preview_layout(r(134, 8), true).list.width, 26);
        assert_eq!(preview_layout(r(135, 8), true).list.width, 27);
        assert_eq!(preview_layout(r(170, 8), true).list.width, 34);
        assert_eq!(preview_layout(r(400, 8), true).list.width, 34);
        // never below the tile minimum width, the preview keeps at least 44 columns
        for w in 70..=400 {
            let l = preview_layout(r(w, 10), true);
            assert!(l.list.width >= 26 && l.list.width <= 34, "w={w}");
            assert!(l.panel.unwrap().width >= 44, "w={w}");
            assert_split(l, r(w, 10));
        }
    }

    #[test]
    fn preview_layout_top_list_bottom_preview_when_narrow_but_tall() {
        let l = preview_layout(r(60, 30), true);
        assert_eq!(l.list, Rect::new(0, 1, 60, 10));
        assert_eq!(l.panel, Some(Rect::new(0, 11, 60, 20)));
        assert!(!l.one_column, "the strip is the usual grid");
        assert_split(l, r(60, 30));
        // list height = clamp(height*2/5, 9, 10): one row of full cards (+ a divider line)
        assert_eq!(preview_layout(r(40, 24), true).list.height, 9);
        assert_eq!(preview_layout(r(40, 25), true).list.height, 10);
        assert_eq!(preview_layout(r(40, 100), true).list.height, 10);
        assert_eq!(preview_layout(r(0, 24), true).list.height, 9, "zero width does not panic");
        let huge = preview_layout(r(69, u16::MAX - 1), true);
        assert_eq!(huge.list.height, 10, "no overflow");
        for h in 24..=200 {
            let l = preview_layout(r(60, h), true);
            assert!(l.panel.unwrap().height >= 15, "h={h}");
        }
    }

    #[test]
    fn preview_layout_boundaries() {
        // width 69/70 (tall enough for both)
        assert_eq!(preview_layout(r(70, 24), true).list.height, 24, "70 wide: side by side");
        assert_eq!(preview_layout(r(69, 24), true).list.width, 69, "69 wide: stacked");
        assert_eq!(preview_layout(r(69, 24), true).panel.map(|p| p.y), Some(1 + 9));
        // height 7/8 when wide
        assert!(preview_layout(r(70, 8), true).panel.is_some_and(|p| p.x == 26 && p.width == 44));
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
    fn render_preview_wraps_a_long_line_fully_and_keeps_the_tail_visible() {
        let long = format!("읽은 화면은 이렇게 보입니다 {} 끝", "x".repeat(40));
        let body = PreviewBody::Screen(format!("older\n{long}\n❯ newest prompt\n"));
        // inner 20x6: the long line (28 + 40 + 3 = 71 cols) wraps into 4 rows
        let screen = rows(&draw(22, 8, |f| render_preview(f, f.area(), Some("p"), &body)));
        let inner: Vec<String> = screen[1..7].iter().map(|r| r.trim_start_matches('│').trim_end_matches('│').to_string()).collect();
        assert!(inner.iter().all(|r| !r.contains('…')), "no truncation: {inner:?}");
        assert_eq!(inner[5].trim_end(), "❯ newest prompt", "{inner:?}");
        assert!(inner[4].trim_end().ends_with("끝"), "end of the long line: {inner:?}");
        let joined: String = inner[0..5].iter().map(|r| r.trim_end()).collect();
        assert!(joined.contains("xxxxxxxx"), "{inner:?}");
        // tall enough: every character of the long line is shown
        let screen = rows(&draw(22, 12, |f| render_preview(f, f.area(), Some("p"), &body)));
        let all: String = screen[1..11].iter().map(|r| r.trim_start_matches('│').trim_end_matches('│').trim_end()).collect();
        assert!(all.replace(' ', "").contains(&long.replace(' ', "")), "every char shown: {screen:?}");
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
