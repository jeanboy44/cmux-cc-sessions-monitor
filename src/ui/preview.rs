//! Preview panel: where it goes (picked from the space between HUD and
//! footer) and how it is drawn. Plain chrome: the tiles' thin rounded border
//! and a dim title, no mascot.
//!
//! Placement rule ([`preview_layout`]):
//! - width ≥ [`RIGHT_MIN_WIDTH`]: panel on the RIGHT, `clamp(width/3, 40, 60)` wide, full height;
//! - else height ≥ [`BOTTOM_MIN_HEIGHT`]: panel at the BOTTOM, `clamp(height/3, 6, 12)` tall;
//! - else no panel (the footer says the terminal is too small).

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Clear, Paragraph};
use ratatui::Frame;

use crate::preview::preview_lines;
use crate::ui::style;
use crate::ui::tile::truncate_to_width;

/// From this width on the panel goes to the right.
pub const RIGHT_MIN_WIDTH: u16 = 120;
/// Below [`RIGHT_MIN_WIDTH`], from this height on the panel goes to the bottom.
pub const BOTTOM_MIN_HEIGHT: u16 = 24;
/// Footer note while the preview is on but there is no room for it.
pub const PREVIEW_TOO_SMALL: &str = "터미널이 작아 미리보기를 쓸 수 없습니다";
pub const PREVIEW_LOADING: &str = "읽는 중…";
pub const PREVIEW_FAILED: &str = "미리보기를 읽지 못했습니다";
pub const PREVIEW_DONE: &str = "종료된 세션";
pub const PREVIEW_NO_SESSION: &str = "선택한 세션이 없습니다";

/// Grid area and (optional) preview panel area.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PreviewLayout {
    pub grid: Rect,
    pub panel: Option<Rect>,
}

/// Splits `area` (between HUD and footer) into grid and preview panel.
pub fn preview_layout(area: Rect, enabled: bool) -> PreviewLayout {
    let full = PreviewLayout { grid: area, panel: None };
    if !enabled {
        return full;
    }
    if area.width >= RIGHT_MIN_WIDTH {
        let w = (area.width / 3).clamp(40, 60);
        let grid = Rect { width: area.width - w, ..area };
        let panel = Rect { x: area.x + grid.width, width: w, ..area };
        PreviewLayout { grid, panel: Some(panel) }
    } else if area.height >= BOTTOM_MIN_HEIGHT {
        let h = (area.height / 3).clamp(6, 12);
        let grid = Rect { height: area.height - h, ..area };
        let panel = Rect { y: area.y + grid.height, height: h, ..area };
        PreviewLayout { grid, panel: Some(panel) }
    } else {
        full
    }
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

    #[test]
    fn preview_layout_right_when_wide() {
        let l = preview_layout(r(150, 30), true);
        assert_eq!(l.panel, Some(Rect::new(100, 1, 50, 30)));
        assert_eq!(l.grid, Rect::new(0, 1, 100, 30));
        // width/3 clamped to 40..=60
        assert_eq!(preview_layout(r(120, 10), true).panel.unwrap().width, 40);
        assert_eq!(preview_layout(r(240, 10), true).panel.unwrap().width, 60);
        assert_eq!(preview_layout(r(300, 10), true).grid.width, 240);
    }

    #[test]
    fn preview_layout_bottom_when_tall_enough() {
        let l = preview_layout(r(100, 30), true);
        assert_eq!(l.panel, Some(Rect::new(0, 21, 100, 10)));
        assert_eq!(l.grid, Rect::new(0, 1, 100, 20));
        // height/3 clamped to 6..=12
        assert_eq!(preview_layout(r(80, 24), true).panel.unwrap().height, 8);
        assert_eq!(preview_layout(r(80, 60), true).panel.unwrap().height, 12);
    }

    #[test]
    fn preview_layout_none_when_small() {
        for (w, h) in [(119, 23), (80, 10), (0, 0), (1, 1), (10, 3)] {
            let l = preview_layout(r(w, h), true);
            assert_eq!(l.panel, None, "{w}x{h}");
            assert_eq!(l.grid, r(w, h));
        }
    }

    #[test]
    fn preview_layout_disabled_gives_full_area() {
        for (w, h) in [(200, 50), (100, 30), (10, 3)] {
            assert_eq!(preview_layout(r(w, h), false), PreviewLayout { grid: r(w, h), panel: None });
        }
    }

    #[test]
    fn preview_layout_boundaries() {
        assert!(preview_layout(r(120, 5), true).panel.is_some_and(|p| p.x == 80), "120 wide: right");
        let l = preview_layout(r(119, 5), true);
        assert_eq!(l.panel, None, "119 wide and short: none");
        let l = preview_layout(r(119, 24), true);
        assert_eq!(l.panel.map(|p| (p.y, p.height)), Some((1 + 16, 8)), "119 wide, 24 tall: bottom");
        assert_eq!(preview_layout(r(119, 23), true).panel, None);
        // the two parts never overlap and fill the area
        for (w, h) in [(120, 24), (200, 60), (119, 24), (130, 3)] {
            let l = preview_layout(r(w, h), true);
            let p = l.panel.unwrap();
            assert!(!l.grid.intersects(p), "{w}x{h}");
            assert_eq!(u32::from(l.grid.area()) + u32::from(p.area()), u32::from(r(w, h).area()));
        }
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
