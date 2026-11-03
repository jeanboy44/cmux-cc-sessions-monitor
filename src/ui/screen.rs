//! Whole-frame drawing: HUD on top, tile grid (or empty / loading message)
//! in the middle, key help and messages in the footer.

use std::time::Instant;

use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use crate::app::{App, Mode};
use crate::model::Status;
use crate::ui::grid::{cell_rect, compute_cols, row_count, scroll_to_show, visible_rows};
use crate::ui::hud::{render_hud, StatusCounts};
use crate::ui::mascot::{sprite, sprite_oneshot_done, SpriteSize};
use crate::ui::settings::render_settings;
use crate::ui::style;
use crate::ui::tile::{render_tile, render_tile_with_sprite, tile_size};

pub const EMPTY_MESSAGE: &str = "cmux에서 실행 중인 Claude 세션이 없습니다";
pub const LOADING_MESSAGE: &str = "세션을 불러오는 중…";
pub const KEY_HELP: &str = " hjkl/←↓↑→ 이동 · Enter 포커스 · i 설정 · q 종료 ";

/// Draws the full frame. Updates `app.cols` and `app.scroll` from the layout.
pub fn draw(f: &mut Frame, app: &mut App, tick: u64, now: Instant) {
    let [hud, grid, footer] =
        Layout::vertical([Constraint::Length(1), Constraint::Fill(1), Constraint::Length(1)]).areas(f.area());
    let counts = StatusCounts::from_statuses(app.sessions.iter().map(|s| s.status));
    render_hud(f, hud, &counts);

    let (above, below) = if app.sessions.is_empty() {
        app.cols = 1;
        app.scroll = 0;
        draw_empty(f, grid, app.loaded, tick);
        (false, false)
    } else {
        draw_grid(f, grid, app, tick)
    };
    draw_footer(f, footer, app, above, below, now);

    if app.mode == Mode::Settings {
        render_settings(f, grid, &app.config, app.settings_cursor, tick, app.pr_tools_note.as_deref());
    }
}

/// Tile grid with vertical scrolling; returns (more above, more below).
/// Areas smaller than one tile fall back to one name-only row per session.
fn draw_grid(f: &mut Frame, area: Rect, app: &mut App, tick: u64) -> (bool, bool) {
    if area.is_empty() {
        return (false, false);
    }
    let view = app.view;
    let n = app.sessions.len();
    let (tile_w, tile_h) = tile_size(view);
    let compact = area.width < tile_w || area.height < tile_h;
    let (cols, visible) = if compact {
        (1, usize::from(area.height))
    } else {
        (compute_cols(area.width, view), visible_rows(area.height, view))
    };
    app.cols = cols;
    app.clamp_selection();
    let total = row_count(n, cols);
    app.scroll = scroll_to_show(app.scroll, app.selected / cols, visible, total);

    for row in 0..visible {
        for col in 0..cols {
            let idx = (app.scroll + row) * cols + col;
            let Some(s) = app.sessions.get(idx) else { break };
            let rect = if compact {
                Rect::new(area.x, area.y + row as u16, area.width, 1)
            } else {
                cell_rect(area, cols, col, row, view)
            };
            let selected = idx == app.selected;
            let fx = app.tile_fx(s, tick);
            match app.done_oneshot_tick(s, tick) {
                Some(t) => {
                    let frame = sprite_oneshot_done(view.sprite_size(), t);
                    render_tile_with_sprite(f, rect, s, &app.config, selected, tick, fx, view, frame);
                }
                None => render_tile(f, rect, s, &app.config, selected, tick, fx, view),
            }
        }
    }
    (app.scroll > 0, app.scroll + visible < total)
}

fn draw_empty(f: &mut Frame, area: Rect, loaded: bool, tick: u64) {
    if area.is_empty() {
        return;
    }
    let text = if loaded { EMPTY_MESSAGE } else { LOADING_MESSAGE };
    let (sw, sh) = SpriteSize::Large.cells();
    let show_sprite = loaded && area.width >= sw && area.height >= sh + 2;
    let block_h = if show_sprite { sh + 2 } else { 1 };
    let top = area.y + area.height.saturating_sub(block_h) / 2;
    if show_sprite {
        let sprite_area = Rect::new(area.x + (area.width - sw) / 2, top, sw, sh);
        f.render_widget(Paragraph::new(sprite(Status::Idle, SpriteSize::Large, tick)), sprite_area);
    }
    let line_area = Rect::new(area.x, top + block_h - 1, area.width, 1);
    let para = Paragraph::new(Line::styled(text, Style::new().fg(style::TEXT).add_modifier(Modifier::BOLD)))
        .alignment(Alignment::Center);
    f.render_widget(para, line_area);
}

fn draw_footer(f: &mut Frame, area: Rect, app: &App, above: bool, below: bool, now: Instant) {
    if area.is_empty() {
        return;
    }
    let dim = Style::new().fg(style::TEXT_DIM);
    let mut spans = Vec::new();
    // The transient message goes first so a narrow footer cuts the key help instead.
    if let Some(msg) = app.footer_message(now) {
        spans.push(Span::styled(
            format!(" {msg} "),
            Style::new().fg(style::BADGE_FG).bg(style::FLASH_ALERT).add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::raw(" "));
    }
    let mut tail = Vec::new();
    if above {
        tail.push(Span::styled("▲ more ", Style::new().fg(style::SELECTED)));
    }
    if below {
        tail.push(Span::styled("▼ more ", Style::new().fg(style::SELECTED)));
    }
    tail.push(Span::styled(KEY_HELP, dim));
    // The persistent PR note is the first thing dropped when space is short.
    if let Some(note) = &app.pr_note {
        let note = format!("{note} · ");
        let width = |v: &[Span]| v.iter().map(Span::width).sum::<usize>();
        if width(&spans) + note.width() + width(&tail) <= usize::from(area.width) {
            spans.push(Span::styled(note, dim));
        }
    }
    spans.extend(tail);
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::fake_session;
    use crate::config::{Config, ViewMode};
    use crate::refresh::Snapshot;
    use crate::ui::style;
    use crate::ui::test_util::{draw as draw_buf, rows};

    fn render(app: &mut App, w: u16, h: u16, tick: u64) -> Vec<String> {
        rows(&draw_buf(w, h, |f| draw(f, app, tick, Instant::now())))
    }

    fn ten_sessions() -> Vec<crate::model::Session> {
        let statuses = [Status::Running, Status::NeedsInput, Status::Idle, Status::Done, Status::Unknown];
        (0..10).map(|i| fake_session(&format!("t{i}"), statuses[i % 5])).collect()
    }

    #[test]
    fn draw_with_zero_sessions_shows_message() {
        let mut app = App::new(Config::default());
        let screen = render(&mut app, 80, 24, 0).join("\n");
        assert!(screen.contains(LOADING_MESSAGE));
        app.apply_snapshot(Snapshot::default(), 0);
        let screen = render(&mut app, 80, 24, 0).join("\n");
        assert!(screen.contains(EMPTY_MESSAGE), "{screen}");
        assert!(screen.contains("SESSIONS 0"));
        assert!(screen.contains("q 종료"));
        // the idle mascot is drawn above the message (half-block pixels)
        assert!(screen.contains('▀') || screen.contains('▄') || screen.contains('█'));
    }

    #[test]
    fn draw_small_terminal_does_not_panic() {
        for view in [ViewMode::A, ViewMode::B, ViewMode::Auto] {
            let mut cfg = Config::default();
            cfg.view = view;
            let mut app = App::new(cfg);
            app.set_sessions(ten_sessions(), 0);
            app.selected = 9;
            for (w, h) in [(0, 0), (1, 1), (5, 2), (10, 3), (20, 6), (29, 7), (40, 11), (200, 3)] {
                let screen = render(&mut app, w, h, 3);
                assert_eq!(screen.len(), h as usize);
            }
            // name-only rows on a terminal smaller than one tile, selection visible
            let screen = render(&mut app, 20, 6, 0).join("\n");
            assert!(screen.contains("proj-t9"), "{screen}");
        }
        let mut app = App::new(Config::default());
        app.apply_snapshot(Snapshot::default(), 0);
        for (w, h) in [(0, 0), (1, 1), (3, 3), (10, 8)] {
            render(&mut app, w, h, 0);
        }
    }

    #[test]
    fn draw_smoke_both_views() {
        for view in [ViewMode::A, ViewMode::B] {
            let mut cfg = Config::default();
            cfg.view = view;
            let mut app = App::new(cfg);
            app.apply_snapshot(Snapshot { sessions: ten_sessions(), ..Default::default() }, 0);
            let screen = render(&mut app, 120, 40, 0);
            let text = screen.join("\n");
            assert!(screen[0].contains("SESSIONS 10"), "{}", screen[0]);
            assert!(screen[0].contains("RUN 2") && screen[0].contains("INPUT 2") && screen[0].contains("DONE 2"));
            assert!(text.contains("proj-t0"), "{text}");
            assert!(screen.last().unwrap().contains("Enter 포커스"));
            assert!(app.cols >= 2, "{view:?} cols={}", app.cols);
        }
    }

    #[test]
    fn draw_scrolls_to_selected_and_hints_more() {
        let mut cfg = Config::default();
        cfg.view = ViewMode::B;
        let mut app = App::new(cfg);
        app.set_sessions(ten_sessions(), 0);
        // 48x26 -> 2 cols, 2 visible rows of 5
        let screen = render(&mut app, 48, 26, 0);
        assert!(screen.last().unwrap().contains("▼ more"));
        app.selected = 9;
        let screen = render(&mut app, 48, 26, 0);
        assert_eq!(app.scroll, 3);
        assert!(screen.join("\n").contains("proj-t9"));
        assert!(screen.last().unwrap().contains("▲ more"));
        assert!(!screen.last().unwrap().contains("▼ more"));
    }

    #[test]
    fn settings_mode_draws_window() {
        let mut app = App::new(Config::default());
        app.set_sessions(ten_sessions(), 0);
        app.mode = Mode::Settings;
        let screen = render(&mut app, 80, 24, 0).join("\n");
        assert!(screen.contains("SETTINGS") && screen.contains("Esc 닫기"), "{screen}");
    }

    #[test]
    fn footer_message_survives_narrow_terminal() {
        let mut app = App::new(Config::default());
        app.set_sessions(ten_sessions(), 0);
        app.selected = 9;
        app.notify("no such surface", Instant::now());
        let screen = render(&mut app, 60, 30, 0);
        let footer = screen.last().unwrap();
        assert!(footer.contains("no such surface"), "{footer}");
    }

    #[test]
    fn footer_shows_pr_note_dim_and_drops_it_first_when_narrow() {
        let mut app = App::new(Config::default());
        let snap = Snapshot { sessions: ten_sessions(), pr_note: Some("PR: az 로그인 필요".into()), ..Default::default() };
        app.apply_snapshot(snap, 0);
        let buf = draw_buf(120, 30, |f| draw(f, &mut app, 0, Instant::now()));
        let footer = rows(&buf).last().unwrap().clone();
        assert!(footer.contains("PR: az 로그인 필요") && footer.contains("q 종료"), "{footer}");
        let x = footer.find("PR:").map(|b| footer[..b].chars().count()).unwrap() as u16;
        assert_eq!(buf[(x, 29)].fg, style::TEXT_DIM);
        // stays after a transient message, which comes first
        app.notify("boom", Instant::now());
        let footer = render(&mut app, 120, 30, 0).last().unwrap().clone();
        assert!(footer.find("boom").unwrap() < footer.find("PR:").unwrap(), "{footer}");
        // narrow: the note goes before the key help is cut
        let footer = render(&mut app, 60, 30, 0).last().unwrap().clone();
        assert!(!footer.contains("PR:") && footer.contains("boom"), "{footer}");
    }

    #[test]
    fn footer_shows_flash_message() {
        let mut app = App::new(Config::default());
        app.set_sessions(ten_sessions(), 0);
        app.notify("이동할 수 없는 세션", Instant::now());
        let screen = render(&mut app, 120, 30, 0);
        assert!(screen.last().unwrap().contains("이동할 수 없는 세션"));
    }
}
