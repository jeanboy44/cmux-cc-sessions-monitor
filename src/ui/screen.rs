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
use crate::ui::grid::{cell_rect, grid_cols};
use crate::ui::groups::{divider_text, place_rows, row_of, scroll_to_show_rows, Row};
use crate::ui::hud::{render_hud, StatusCounts};
use crate::ui::mascot::{sprite, sprite_oneshot_done, SpriteSize};
use crate::ui::preview::{preview_layout, preview_read_lines, render_preview, PREVIEW_TOO_SMALL};
use crate::ui::settings::render_settings;
use crate::ui::style;
use crate::ui::tile::{render_tile, render_tile_with_sprite, tile_size};

pub const EMPTY_MESSAGE: &str = "cmux에서 실행 중인 Claude 세션이 없습니다";
pub const LOADING_MESSAGE: &str = "세션을 불러오는 중…";
pub const KEY_HELP: &str = " hjkl/←↓↑→ 이동 · Enter 포커스 · p 미리보기 · i 설정 · q 종료 ";

/// Draws the full frame. Updates `app.cols`, `app.scroll`, `app.grid_rect`,
/// `app.preview_too_small` and the preview's read line count from the layout.
/// With the preview panel shown the session list is one column.
pub fn draw(f: &mut Frame, app: &mut App, tick: u64, now: Instant) {
    let [hud, middle, footer] =
        Layout::vertical([Constraint::Length(1), Constraint::Fill(1), Constraint::Length(1)]).areas(f.area());
    let counts = StatusCounts::from_statuses(app.sessions.iter().map(|s| s.status));
    render_hud(f, hud, &counts);

    let layout = preview_layout(middle, app.preview);
    let grid = layout.list;
    app.grid_rect = grid;
    app.preview_too_small = app.preview && layout.panel.is_none();
    if let Some(panel) = layout.panel {
        app.set_preview_read_lines(preview_read_lines(panel));
        let name = app.selected_session().map(|s| s.name.clone());
        render_preview(f, panel, name.as_deref(), &app.preview_body());
    }

    let (above, below) = if app.sessions.is_empty() {
        app.cols = 1;
        app.scroll = 0;
        draw_empty(f, grid, app.loaded, tick);
        (false, false)
    } else {
        draw_grid(f, grid, app, tick, layout.one_column)
    };
    draw_footer(f, footer, app, above, below, now);

    if app.mode == Mode::Settings {
        render_settings(f, middle, &app.config, app.settings_cursor, tick, app.pr_tools_note.as_deref());
    }
}

/// Tile grid (grouped by cmux window, see [`crate::ui::groups`]) with
/// vertical scrolling in layout rows; returns (more above, more below).
/// Areas smaller than one tile fall back to one name-only row per session.
/// `one_column` (preview shown) forces a single column, see [`grid_cols`].
fn draw_grid(f: &mut Frame, area: Rect, app: &mut App, tick: u64, one_column: bool) -> (bool, bool) {
    if area.is_empty() {
        return (false, false);
    }
    let view = app.view;
    let (tile_w, tile_h) = tile_size(view);
    let compact = area.width < tile_w || area.height < tile_h;
    let cols = if compact { 1 } else { grid_cols(area.width, view, one_column) };
    app.cols = cols;
    app.clamp_selection();
    let rows = app.grid_rows();
    let tile_row_h = if compact { 1 } else { tile_h };
    let heights: Vec<u16> =
        rows.iter().map(|r| if matches!(r, Row::Divider { .. }) { 1 } else { tile_row_h }).collect();
    let sel_row = row_of(&rows, app.selected).unwrap_or(0);
    app.scroll = scroll_to_show_rows(&rows, &heights, app.scroll, sel_row, area.height);
    let placed = place_rows(&heights, app.scroll, area);
    let dim = Style::new().fg(style::TEXT_DIM);

    for (ri, row_rect) in &placed {
        let (start, end) = match &rows[*ri] {
            Row::Divider { label, count } => {
                let line = divider_text(label, *count, usize::from(row_rect.width));
                f.render_widget(Paragraph::new(Line::styled(line, dim)), Rect { height: 1, ..*row_rect });
                continue;
            }
            Row::Tiles { start, end } => (*start, *end),
        };
        for (col, idx) in (start..end).enumerate() {
            let Some(s) = app.sessions.get(idx) else { break };
            let rect = if compact {
                Rect { height: 1, ..*row_rect }
            } else {
                cell_rect(*row_rect, cols, col, 0, view)
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
    let below = placed.last().is_some_and(|(ri, r)| ri + 1 < rows.len() || r.height < heights[*ri]);
    (app.scroll > 0, below)
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
    if app.preview_too_small {
        spans.push(Span::styled(format!("{PREVIEW_TOO_SMALL} · "), dim));
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
    use crate::app::{fake_session, Dir};
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
        // the idle mascot is drawn above the message (Braille dots)
        assert!(screen.chars().any(|c| ('\u{2801}'..='\u{28ff}').contains(&c)), "{screen}");
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
    fn closed_tab_session_leaves_hud_and_selection_stays_valid() {
        let mut app = App::new(Config::default());
        let three: Vec<_> = ["a", "b", "c"].iter().map(|id| fake_session(id, Status::Running)).collect();
        app.apply_snapshot(Snapshot { sessions: three.clone(), ..Default::default() }, 0);
        app.selected = 2;
        assert!(render(&mut app, 120, 30, 0)[0].contains("SESSIONS 3"));
        // the refresh dropped "c" (its tab was closed): no Done tile, count drops at once
        app.apply_snapshot(Snapshot { sessions: three[..2].to_vec(), ..Default::default() }, 5);
        let screen = render(&mut app, 120, 30, 5);
        assert!(screen[0].contains("SESSIONS 2") && screen[0].contains("DONE 0"), "{}", screen[0]);
        assert!(!screen.join("\n").contains("proj-c"));
        assert_eq!(app.selected_session().map(|s| s.id.as_str()), Some("b"), "clamped to a valid tile");
        assert!(!app.has_transition("c"), "no stale fx state for the closed session");
        // the selected tile itself closes: the selection clamps to a tile that exists
        app.selected = 0;
        app.apply_snapshot(Snapshot { sessions: three[1..2].to_vec(), ..Default::default() }, 6);
        assert_eq!(app.selected_session().map(|s| s.id.as_str()), Some("b"));
    }

    fn preview_app(n: usize) -> App {
        let mut cfg = Config::default();
        cfg.view = ViewMode::A;
        let mut app = App::new(cfg);
        let sessions = (0..n).map(|i| fake_session(&format!("t{i}"), Status::Running)).collect();
        app.apply_snapshot(Snapshot { sessions, ..Default::default() }, 0);
        app.preview = true;
        let (cache, _) = app.preview_shared();
        cache.put(
            "t0",
            crate::preview::PreviewEntry { screen: Ok("first\n❯ last prompt line\n\n".into()), fetched_at: Instant::now() },
        );
        app
    }

    /// Rows of the list part (`x < list_w`) of the screen, HUD/footer excluded.
    fn list_part(screen: &[String], list_w: usize, rows: std::ops::Range<usize>) -> Vec<String> {
        screen[rows].iter().map(|r| r.chars().take(list_w).collect()).collect()
    }

    #[test]
    fn preview_turns_the_list_into_one_column_left_with_preview_right() {
        let mut app = preview_app(6);
        let screen = render(&mut app, 150, 30, 0);
        // middle 150x28: list clamp(150/4, 28, 40) = 37 wide, preview the rest
        assert_eq!(app.grid_rect, Rect::new(0, 1, 37, 28));
        assert_eq!(app.cols, 1, "one tile per row");
        let list = list_part(&screen, 37, 1..29);
        for row in &list {
            assert!(row.matches("proj-t").count() <= 1, "one tile per row: {row}");
        }
        let text = list.join("\n");
        for i in 0..4 {
            assert!(text.contains(&format!("proj-t{i}")), "{text}");
        }
        assert!(screen[1].chars().nth(37) == Some('╭') && screen[1].contains("미리보기 · proj-t0"), "{}", screen[1]);
        assert!(screen[28].chars().nth(37) == Some('╰') && screen[28].ends_with('╯'), "{}", screen[28]);
        let right: String = screen[2..28].iter().map(|r| r.chars().skip(38).collect::<String>()).collect::<Vec<_>>().join("\n");
        assert!(right.contains("first") && right.contains("❯ last prompt line"), "{right}");
        assert!(screen.last().unwrap().contains("▼ more"), "6 tiles do not fit 28 rows");
        assert!(!app.preview_too_small);
        // the read asks for the panel's inner height
        assert_eq!(app.preview_target().map(|t| t.lines), Some(26));
        // the selection at the bottom is scrolled into view
        app.selected = 5;
        let screen = render(&mut app, 150, 30, 0);
        assert!(list_part(&screen, 37, 1..29).join("\n").contains("proj-t5"));
        assert!(screen.last().unwrap().contains("▲ more"));
        assert!(screen[1].contains("미리보기 · proj-t5"), "{}", screen[1]);
    }

    #[test]
    fn preview_stacks_list_on_top_of_preview_when_narrow_but_tall() {
        let mut app = preview_app(10);
        app.selected = 9;
        let screen = render(&mut app, 60, 32, 0);
        // middle 60x30: list clamp(30*2/5, 8, 14) = 12 rows, preview 18 below
        assert_eq!(app.grid_rect, Rect::new(0, 1, 60, 12));
        assert_eq!(app.cols, 1, "still one column though 60 fits two tiles");
        assert!(screen[13].starts_with('╭') && screen[13].contains("미리보기 · proj-t9"), "{}", screen[13]);
        assert!(screen[30].starts_with('╰'), "{}", screen[30]);
        assert!(screen.join("\n").contains(crate::ui::preview::PREVIEW_LOADING), "t9 not read yet");
        let list = screen[1..13].join("\n");
        assert!(list.contains("proj-t9") && list.contains("proj-t8") && !list.contains("proj-t7"), "{list}");
        for row in &screen[1..13] {
            assert!(row.matches("proj-t").count() <= 1, "{row}");
        }
        assert_eq!(app.preview_target().map(|t| t.lines), Some(16));
    }

    #[test]
    fn preview_read_lines_follow_the_panel_height_capped_at_80() {
        let mut app = preview_app(3);
        assert_eq!(app.preview_target().map(|t| t.lines), Some(crate::source::cmux::SCREEN_LINES), "before a draw");
        render(&mut app, 150, 50, 0);
        assert_eq!(app.preview_target().map(|t| t.lines), Some(46));
        let (_, slot) = app.preview_shared();
        assert_eq!(slot.lock().unwrap().as_ref().map(|t| t.lines), Some(46), "published to the thread");
        render(&mut app, 150, 200, 0);
        assert_eq!(app.preview_target().map(|t| t.lines), Some(80));
        assert_eq!(slot.lock().unwrap().as_ref().map(|t| t.lines), Some(80));
        render(&mut app, 60, 26, 0); // middle 60x24 -> preview 24 - 9 = 15 rows
        assert_eq!(app.preview_target().map(|t| t.lines), Some(13));
    }

    #[test]
    fn toggling_preview_forces_one_column_and_restores_the_grid_keeping_selection() {
        let mut app = preview_app(12);
        app.preview = false;
        app.selected = 10;
        for _ in 0..2 {
            let screen = render(&mut app, 150, 30, 0);
            assert_eq!(app.grid_rect, Rect::new(0, 1, 150, 28));
            assert_eq!(app.cols, crate::ui::grid::compute_cols(150, app.view), "normal grid");
            assert!(screen.join("\n").contains("proj-t10"));
            app.toggle_preview();
            assert!(app.preview);
            let screen = render(&mut app, 150, 30, 0);
            assert_eq!(app.cols, 1);
            assert_eq!(app.grid_rect.width, 37);
            assert_eq!(app.selected_session().map(|s| s.id.as_str()), Some("t10"), "selection kept by id");
            assert!(list_part(&screen, 37, 1..29).join("\n").contains("proj-t10"), "still visible");
            // j/k move one tile, h/l stay
            app.apply_move(Dir::Down);
            assert_eq!(app.selected, 11);
            app.apply_move(Dir::Up);
            app.apply_move(Dir::Right);
            app.apply_move(Dir::Left);
            assert_eq!(app.selected, 10);
            app.toggle_preview();
        }
        let screen = render(&mut app, 150, 30, 0);
        assert!(app.cols > 1 && screen.join("\n").contains("proj-t10"));
    }

    #[test]
    fn preview_too_small_shows_footer_note_and_the_normal_grid() {
        for (w, h, cols) in [(110, 9, 4), (60, 25, 2), (69, 25, 2)] {
            let mut app = preview_app(4);
            let screen = render(&mut app, w, h, 0);
            assert!(app.preview_too_small, "{w}x{h}");
            assert_eq!(app.grid_rect, Rect::new(0, 1, w, h - 2));
            assert_eq!(app.cols, cols, "{w}x{h}: grid back to normal");
            assert!(screen.last().unwrap().contains(PREVIEW_TOO_SMALL), "{}", screen.last().unwrap());
            assert!(!screen[..h as usize - 1].join("\n").contains("미리보기 · "), "no panel");
        }
        // one more row / column and the panel is back
        for (w, h) in [(110, 10), (60, 26), (70, 10)] {
            let mut app = preview_app(4);
            render(&mut app, w, h, 0);
            assert!(!app.preview_too_small && app.cols == 1, "{w}x{h}");
        }
    }

    #[test]
    fn preview_on_tiny_terminals_does_not_panic() {
        for n in [0, 1, 10] {
            let mut app = preview_app(n);
            app.selected = n.saturating_sub(1);
            for (w, h) in [(0, 0), (1, 1), (10, 3), (10, 26), (40, 26), (70, 10), (121, 3), (200, 5), (3, 60)] {
                assert_eq!(render(&mut app, w, h, 0).len(), h as usize);
            }
        }
    }

    fn windowed(id: &str, w: &str) -> crate::model::Session {
        let mut s = fake_session(id, Status::Running);
        s.window_id = Some(w.into());
        s
    }

    #[test]
    fn two_windows_draw_divider_lines_and_grouped_tiles() {
        let mut cfg = Config::default();
        cfg.view = ViewMode::A;
        let mut app = App::new(cfg);
        let snap = Snapshot {
            sessions: vec![windowed("x", "W2"), windowed("y", "W1"), windowed("z", "W2")],
            window_order: vec!["W1".into(), "W2".into()],
            current_window: Some("W2".into()),
            ..Default::default()
        };
        app.apply_snapshot(snap, 0);
        let screen = render(&mut app, 80, 24, 0);
        assert!(screen[0].contains("SESSIONS 3"), "HUD counts stay global");
        let w = 80;
        assert_eq!(screen[1], crate::ui::groups::divider_text("창 1", 1, w));
        assert!(screen[1].starts_with("── 창 1 · 세션 1 ─") && !screen[1].contains('╭'), "{}", screen[1]);
        assert!(screen[3].contains("proj-y"), "{}", screen.join("\n"));
        assert_eq!(screen[8], crate::ui::groups::divider_text("창 2 (현재)", 2, w));
        assert!(screen[10].contains("proj-x") && screen[10].contains("proj-z"), "{}", screen.join("\n"));
        // dim, plain line
        let buf = draw_buf(80, 24, |f| draw(f, &mut app, 0, Instant::now()));
        assert_eq!(buf[(0, 1)].fg, style::TEXT_DIM);
        // one window only: no divider at all
        let one = Snapshot {
            sessions: vec![windowed("x", "W2"), windowed("z", "W2")],
            window_order: vec!["W1".into(), "W2".into()],
            ..Default::default()
        };
        app.apply_snapshot(one, 1);
        let text = render(&mut app, 80, 24, 0).join("\n");
        assert!(!text.contains("창 "), "{text}");
    }

    #[test]
    fn one_column_list_scrolls_to_selected_with_group_dividers() {
        let mut cfg = Config::default();
        cfg.view = ViewMode::A;
        let mut app = App::new(cfg);
        let mut sessions: Vec<_> = (0..4).map(|i| windowed(&format!("a{i}"), "W1")).collect();
        sessions.extend((0..4).map(|i| windowed(&format!("b{i}"), "W2")));
        app.apply_snapshot(Snapshot { sessions, window_order: vec!["W1".into(), "W2".into()], ..Default::default() }, 0);
        app.preview = true;
        // 150x16: middle 150x14 -> list 37x14, one column: D T T T T D T T T T
        app.selected = 7;
        let screen = render(&mut app, 150, 16, 0);
        assert_eq!((app.grid_rect.width, app.cols), (37, 1));
        let list = list_part(&screen, 37, 1..15).join("\n");
        assert!(list.contains("proj-b3") && list.contains("proj-b2"), "{list}");
        assert!(screen.last().unwrap().contains("▲ more"));
        // first tile of group 2: its divider is shown right above it
        app.selected = 4;
        let screen = render(&mut app, 150, 16, 0);
        let list = list_part(&screen, 37, 1..15);
        let d = list.iter().position(|r| r.starts_with("── 창 2 · 세션 4")).expect("divider of group 2");
        assert!(list[d + 2].contains("proj-b0"), "{list:?}");
        // j walks every tile in order, across the divider
        app.selected = 0;
        let mut seen = vec![0];
        for _ in 0..9 {
            app.apply_move(Dir::Down);
            render(&mut app, 150, 16, 0);
            seen.push(app.selected);
        }
        assert_eq!(seen, vec![0, 1, 2, 3, 4, 5, 6, 7, 7, 7]);
        for sel in (0..8).rev() {
            app.selected = sel;
            let screen = render(&mut app, 150, 16, 0);
            let id = &app.sessions[sel].id;
            assert!(list_part(&screen, 37, 1..15).join("\n").contains(&format!("proj-{id}")), "sel={sel}");
        }
        assert_eq!(app.scroll, 0);
        assert!(screen_starts_with_divider(&render(&mut app, 150, 16, 0)));
    }

    fn screen_starts_with_divider(screen: &[String]) -> bool {
        screen[1].starts_with("── 창 1 · 세션 4")
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
