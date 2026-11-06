//! Whole-frame drawing: HUD on top, card grid (or empty / loading message)
//! in the middle, key help and messages in the footer.

use std::time::Instant;

use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use crate::app::{App, Mode};
use crate::ui::card::{card_size, compact_mode, render_compact, render_wide_card};
use crate::ui::grid::{card_cols, card_rect, row_heights, solid_heights};
use crate::ui::groups::{divider_text, place_rows, row_of, scroll_to_show_rows, Row};
use crate::ui::hud::{render_hud, StatusCounts};
use crate::ui::preview::{preview_layout, preview_read_lines, render_preview, PREVIEW_TOO_SMALL};
use crate::ui::settings::render_settings;
use crate::ui::style;

pub const EMPTY_MESSAGE: &str = "cmux에서 실행 중인 Claude 세션이 없습니다";
pub const LOADING_MESSAGE: &str = "세션을 불러오는 중…";
pub const KEY_HELP: &str = " hjkl · Enter 포커스 · o 세션기억 · s/m 탭 · p 미리보기 · i 설정 · q 종료 ";

/// Draws the full frame. Updates `app.cols`, `app.scroll`, `app.grid_rect`,
/// `app.preview_too_small` and the preview's read line count from the layout.
/// With the preview panel beside it the session list is one column of
/// full cards (of the configured layout) stretched to the list width.
pub fn draw(f: &mut Frame, app: &mut App, tick: u64, now: Instant) {
    let [hud, middle, footer] =
        Layout::vertical([Constraint::Length(1), Constraint::Fill(1), Constraint::Length(1)]).areas(f.area());
    let counts = StatusCounts::from_statuses(app.sessions.iter().map(|s| s.status));
    render_hud(f, hud, &counts, app.mode);

    let layout = preview_layout(middle, app.preview, app.config.card_layout);
    let grid = layout.list;
    app.grid_rect = grid;
    app.preview_too_small = app.preview && layout.panel.is_none();
    if let Some(panel) = layout.panel {
        app.set_preview_read_lines(preview_read_lines(panel));
        let name = app.selected_session().map(|s| s.name.clone());
        render_preview(f, panel, name.as_deref(), &app.preview_body());
    }

    app.spirits_visible = false;
    let (above, below) = if app.sessions.is_empty() {
        app.cols = 1;
        app.scroll = 0;
        draw_empty(f, grid, app.loaded);
        (false, false)
    } else {
        draw_grid(f, grid, app, layout.one_column, tick)
    };
    draw_footer(f, footer, app, above, below, now);

    if app.mode == Mode::Settings {
        render_settings(f, middle, &app.config, app.settings_cursor, tick, app.pr_tools_note.as_deref());
    } else if app.mode == Mode::Memory {
        let tz = crate::storage::Timezone::parse(&app.config.timezone);
        crate::ui::memory::render_memory_view(f, middle, &app.memory_view, tick, tz);
    }
}

/// Card grid (grouped by cmux window, see [`crate::ui::groups`]) with
/// vertical scrolling in layout rows; returns (more above, more below).
/// `one_column` (the preview's side list): one full card per row, stretched
/// to the list width. Columns, rows and scrolling follow the card size of
/// the configured layout. In compact mode (the area cannot hold one full
/// card of that layout, see [`compact_mode`]) every session is one line in
/// a single column.
/// Only placed (visible) rows are drawn, so off-screen cards never render a
/// spirit; `app.spirits_visible` tells the event loop whether to animate.
fn draw_grid(f: &mut Frame, area: Rect, app: &mut App, one_column: bool, tick: u64) -> (bool, bool) {
    if area.is_empty() {
        return (false, false);
    }
    let card_layout = app.config.card_layout;
    let (card_w, card_h) = card_size(card_layout);
    let compact = compact_mode(area, card_layout);
    let cols = if compact || one_column { 1 } else { card_cols(area.width, card_layout) };
    app.cols = cols;
    app.clamp_selection();
    let rows = app.grid_rows();
    let card_row_h = if compact { 1 } else { card_h };
    let heights = row_heights(&rows, card_row_h);
    let solid = solid_heights(&rows, card_row_h);
    let sel_row = row_of(&rows, app.selected).unwrap_or(0);
    app.scroll = scroll_to_show_rows(&rows, &heights, &solid, app.scroll, sel_row, area.height);
    let placed = place_rows(&heights, &solid, app.scroll, area);
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
            let selected = idx == app.selected;
            if compact {
                render_compact(f, Rect { height: 1, ..*row_rect }, s, &app.config, selected, tick);
            } else {
                let rect = card_rect(*row_rect, cols, col, 0, card_layout);
                app.spirits_visible |= rect.width >= card_w && rect.height >= card_h;
                render_wide_card(f, rect, s, &app.config, selected, tick);
            }
        }
    }
    let below = placed.last().is_some_and(|(ri, r)| ri + 1 < rows.len() || r.height < solid[*ri]);
    (app.scroll > 0, below)
}

fn draw_empty(f: &mut Frame, area: Rect, loaded: bool) {
    if area.is_empty() {
        return;
    }
    let text = if loaded { EMPTY_MESSAGE } else { LOADING_MESSAGE };
    let line_area = Rect::new(area.x, area.y + area.height / 2, area.width, 1);
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
    use crate::config::{CardLayout, Config};
    use crate::model::Status;
    use crate::refresh::Snapshot;
    use crate::ui::style;
    use crate::ui::card::{CARD_H, CARD_W};
    use crate::ui::grid::{GAP_X, GAP_Y};
    use crate::ui::test_util::{draw as draw_buf, rows};

    /// Rows from the top of one card row to the top of the next.
    const STEP: u16 = CARD_H + GAP_Y;

    /// Terminal height with room for exactly 2 and 4 card rows.
    const TWO_ROWS: u16 = 2 + 2 * STEP - GAP_Y;
    const FOUR_ROWS: u16 = 2 + 4 * STEP - GAP_Y;

    /// Terminal width that exactly fits `n` standard cards and their gaps.
    const fn row_w(n: u16) -> u16 {
        n * CARD_W + (n - 1) * GAP_X
    }

    fn render(app: &mut App, w: u16, h: u16, tick: u64) -> Vec<String> {
        rows(&draw_buf(w, h, |f| draw(f, app, tick, Instant::now())))
    }

    fn ten_sessions() -> Vec<crate::model::Session> {
        let statuses = [Status::Running, Status::NeedsInput, Status::Idle, Status::Done, Status::Unknown];
        (0..10).map(|i| fake_session(&format!("t{i}"), statuses[i % 5])).collect()
    }

    fn has_braille(text: &str) -> bool {
        text.chars().any(|c| ('\u{2801}'..='\u{28ff}').contains(&c))
    }

    #[test]
    fn draw_with_zero_sessions_shows_message() {
        let mut app = App::new(Config::default());
        let screen = render(&mut app, 80, 24, 0).join("\n");
        assert!(screen.contains(LOADING_MESSAGE));
        app.apply_snapshot(Snapshot::default());
        let screen = render(&mut app, 80, 24, 0).join("\n");
        assert!(screen.contains(EMPTY_MESSAGE), "{screen}");
        assert!(screen.contains("SESSIONS 0"));
        assert!(screen.contains("q 종료"));
        assert!(!has_braille(&screen), "text only: {screen}");
    }

    #[test]
    fn draw_small_terminal_does_not_panic() {
        let mut app = App::new(Config::default());
        app.set_sessions(ten_sessions());
        app.selected = 9;
        for (w, h) in [(0, 0), (1, 1), (5, 2), (10, 3), (20, 6), (29, 7), (40, 11), (49, 18), (50, 17), (200, 3)] {
            let screen = render(&mut app, w, h, 3);
            assert_eq!(screen.len(), h as usize);
        }
        // compact one-line cards on a terminal smaller than one card, selection visible
        let screen = render(&mut app, 20, 6, 0).join("\n");
        assert!(screen.contains("proj-t9"), "{screen}");
        assert!(!screen.contains('╭'), "{screen}");
        let mut app = App::new(Config::default());
        app.apply_snapshot(Snapshot::default());
        for (w, h) in [(0, 0), (1, 1), (3, 3), (10, 8)] {
            render(&mut app, w, h, 0);
        }
    }

    #[test]
    fn draw_smoke_cards_with_scenes() {
        let mut app = App::new(Config::default());
        app.apply_snapshot(Snapshot { sessions: ten_sessions(), ..Default::default() });
        let buf = draw_buf(120, 40, |f| draw(f, &mut app, 0, Instant::now()));
        let screen = rows(&buf);
        let text = screen.join("\n");
        assert!(screen[0].contains("SESSIONS 10"), "{}", screen[0]);
        assert!(screen[0].contains("RUN 2") && screen[0].contains("INPUT 2") && screen[0].contains("DONE 2"));
        assert!(text.contains("proj-t0") && text.contains("proj-t1"), "{text}");
        assert!(has_braille(&text), "scenes drawn: {text}");
        assert!(screen.last().unwrap().contains("Enter 포커스"), "{}", screen.last().unwrap());
        assert_eq!(app.cols, 3, "(120 + 2) / (29 + 2)");
        // the cards keep their size from the left: the selected (first) card
        // has the blue border, the second does not
        let second = CARD_W + GAP_X;
        assert_eq!(buf[(0, 1)].fg, style::SELECTED);
        assert_eq!(buf[(CARD_W - 1, 1)].symbol(), "╮");
        assert!(buf[(CARD_W, 1)].symbol() == " " && buf[(CARD_W + 1, 1)].symbol() == " ", "the gap");
        assert_eq!(buf[(second, 1)].symbol(), "╭");
        assert_ne!(buf[(second, 1)].fg, style::SELECTED);
        assert_eq!(buf[(2 * second, 1)].symbol(), "╭");
        assert_eq!(buf[(row_w(3) - 1, 1)].symbol(), "╮");
        assert_eq!(buf[(row_w(3), 1)].symbol(), " ", "the spare width stays at the right");
    }

    #[test]
    fn draw_scrolls_to_selected_and_hints_more() {
        let mut app = App::new(Config::default());
        app.set_sessions((0..10).map(|i| fake_session(&format!("t{i}"), Status::Running)).collect());
        // 3 cols, 2 visible card rows out of 4
        let screen = render(&mut app, row_w(3), TWO_ROWS, 0);
        assert_eq!(app.cols, 3);
        assert!(screen.last().unwrap().contains("▼ more"));
        app.selected = 9;
        let screen = render(&mut app, row_w(3), TWO_ROWS, 0);
        assert_eq!(app.scroll, 2);
        assert!(screen.join("\n").contains("proj-t9"));
        assert!(screen.last().unwrap().contains("▲ more"));
        assert!(!screen.last().unwrap().contains("▼ more"));
    }

    fn twenty_sessions() -> App {
        let mut app = App::new(Config::default());
        app.set_sessions((0..20).map(|i| fake_session(&format!("t{i}"), Status::Running)).collect());
        app
    }

    #[test]
    fn offscreen_cards_are_not_rendered_by_spirit() {
        use crate::ui::card::{reset_spirit_renders, spirit_renders};
        let mut app = twenty_sessions();
        // 5 columns, 2 card rows = 10 of 20 cards visible
        reset_spirit_renders();
        render(&mut app, row_w(5), TWO_ROWS, 0);
        assert_eq!(app.cols, 5);
        assert_eq!(spirit_renders(), 10, "only visible cards");
        assert!(app.spirits_visible);
        // scrolled to the last card: still only what is on screen
        app.selected = 19;
        reset_spirit_renders();
        render(&mut app, row_w(5), TWO_ROWS, 1);
        assert_eq!(app.scroll, 2);
        assert_eq!(spirit_renders(), 10);
        // preview list: one column of full cards, only the 4 that fit
        app.preview = true;
        reset_spirit_renders();
        render(&mut app, 150, FOUR_ROWS, 2);
        assert_eq!(spirit_renders(), 4);
        assert!(app.spirits_visible);
        // compact one-line cards (a tiny terminal) draw no spirit
        app.preview = false;
        reset_spirit_renders();
        render(&mut app, 20, 6, 3);
        assert_eq!(spirit_renders(), 0);
        assert!(!app.spirits_visible);
        let mut empty = App::new(Config::default());
        render(&mut empty, 100, 30, 0);
        assert!(!empty.spirits_visible);
    }

    #[test]
    fn moving_down_keeps_the_selected_card_visible() {
        let mut app = twenty_sessions();
        let mut seen = Vec::new();
        let w = row_w(5);
        for _ in 0..7 {
            let buf = draw_buf(w, TWO_ROWS, |f| draw(f, &mut app, 0, Instant::now()));
            let text = rows(&buf)[1..usize::from(TWO_ROWS) - 1].join("\n");
            let sel = app.selected;
            assert!(text.contains(&format!("proj-t{sel} ")), "sel={sel} scroll={}: {text}", app.scroll);
            let blue: Vec<(u16, u16)> = (1..TWO_ROWS - 1).flat_map(|y| (0..w).map(move |x| (x, y))).filter(|&p| buf[p].symbol() == "╭" && buf[p].fg == style::SELECTED).collect();
            assert_eq!(blue.len(), 1, "sel={sel}: one blue card on screen");
            seen.push(sel);
            app.apply_move(Dir::Down);
        }
        assert_eq!(seen, vec![0, 5, 10, 15, 15, 15, 15]);
        assert_eq!(app.scroll, 2);
        // and back up again; k on the first card stays, l on the last stays
        for _ in 0..3 {
            app.apply_move(Dir::Up);
            let text = render(&mut app, w, TWO_ROWS, 0)[1..usize::from(TWO_ROWS) - 1].join("\n");
            assert!(text.contains(&format!("proj-t{} ", app.selected)), "{text}");
        }
        assert_eq!((app.selected, app.scroll), (0, 0));
        app.apply_move(Dir::Up);
        assert_eq!((app.selected, app.scroll), (0, 0), "k on the first card stays");
        app.selected = 19;
        app.apply_move(Dir::Right);
        assert_eq!(app.selected, 19, "l on the last card stays");
    }

    #[test]
    fn settings_mode_draws_window() {
        let mut app = App::new(Config::default());
        app.set_sessions(ten_sessions());
        app.mode = Mode::Settings;
        let screen = render(&mut app, 80, 24, 0).join("\n");
        assert!(screen.contains("SETTINGS") && screen.contains("Esc 닫기") && screen.contains("기본 테마"), "{screen}");
    }

    #[test]
    fn footer_message_survives_narrow_terminal() {
        let mut app = App::new(Config::default());
        app.set_sessions(ten_sessions());
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
        app.apply_snapshot(snap);
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
        app.apply_snapshot(Snapshot { sessions: three.clone(), ..Default::default() });
        app.selected = 2;
        assert!(render(&mut app, 120, 40, 0)[0].contains("SESSIONS 3"));
        // the refresh dropped "c" (its tab was closed): no Done card, count drops at once
        app.apply_snapshot(Snapshot { sessions: three[..2].to_vec(), ..Default::default() });
        let screen = render(&mut app, 120, 40, 5);
        assert!(screen[0].contains("SESSIONS 2") && screen[0].contains("DONE 0"), "{}", screen[0]);
        assert!(!screen.join("\n").contains("proj-c"));
        assert_eq!(app.selected_session().map(|s| s.id.as_str()), Some("b"), "clamped to a valid card");
        // the selected card itself closes: the selection clamps to a card that exists
        app.selected = 0;
        app.apply_snapshot(Snapshot { sessions: three[1..2].to_vec(), ..Default::default() });
        assert_eq!(app.selected_session().map(|s| s.id.as_str()), Some("b"));
    }

    fn preview_app(n: usize) -> App {
        let mut app = App::new(Config::default());
        let sessions = (0..n).map(|i| fake_session(&format!("t{i}"), Status::Running)).collect();
        app.apply_snapshot(Snapshot { sessions, ..Default::default() });
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

    /// Top-left corners of full cards in the list part (`x < list_w`, rows 1..h-1).
    fn card_corners(buf: &ratatui::buffer::Buffer, list_w: u16) -> Vec<(u16, u16)> {
        let h = buf.area.height;
        (1..h.saturating_sub(1)).flat_map(|y| (0..list_w).map(move |x| (x, y))).filter(|&p| matches!(buf[p].symbol(), "╭" | "┏")).collect()
    }

    #[test]
    fn preview_mode_lists_full_cards() {
        let mut app = preview_app(6);
        let buf = draw_buf(200, 62, |f| draw(f, &mut app, 0, Instant::now()));
        let screen = rows(&buf);
        let list_w = app.grid_rect.width;
        assert_eq!(list_w, CARD_W, "one card wide");
        assert_eq!(app.cols, 1);
        // six cards stacked, a gap row between them, each as wide as the list column
        let corners = card_corners(&buf, list_w);
        let want: Vec<(u16, u16)> = (0..6).map(|i| (0, 1 + i * STEP)).collect();
        assert_eq!(corners, want, "{}", screen.join("\n"));
        for &(x, y) in &corners {
            assert!(matches!(buf[(x + list_w - 1, y)].symbol(), "╮" | "┓"), "card spans the column at row {y}");
            assert!(matches!(buf[(x, y + CARD_H - 1)].symbol(), "╰" | "┗"), "card is CARD_H tall at row {y}");
            assert_eq!(rows(&buf)[usize::from(y + CARD_H)].chars().take(usize::from(list_w)).collect::<String>().trim(), "", "gap row below the card at {y}");
        }
        let list = list_part(&screen, list_w as usize, 1..61);
        assert!(has_braille(&list.join("\n")), "spirits drawn in the preview list");
        for i in 0..6usize {
            let top = i * STEP as usize;
            let name_row = &list[top + 1];
            assert!(name_row.contains(&format!("proj-t{i}")), "card {i}: {name_row}");
            assert!(has_braille(&list[top..top + CARD_H as usize].join("")), "card {i} has its spirit");
        }
        assert_eq!(buf[(0, 1)].fg, style::SELECTED, "selected card border");
        assert_ne!(buf[(0, 1 + STEP)].fg, style::SELECTED);
        assert!(app.spirits_visible);
    }

    #[test]
    fn preview_turns_the_list_into_one_column_left_with_preview_right() {
        let mut app = preview_app(6);
        let (list_w, h) = (usize::from(CARD_W), usize::from(FOUR_ROWS));
        let buf = draw_buf(150, FOUR_ROWS, |f| draw(f, &mut app, 0, Instant::now()));
        let screen = rows(&buf);
        // the list is one card wide, the preview gets the rest
        assert_eq!(app.grid_rect, Rect::new(0, 1, CARD_W, FOUR_ROWS - 2));
        assert_eq!(app.cols, 1, "one card per row");
        assert_eq!(card_corners(&buf, CARD_W), (0..4).map(|i| (0, 1 + i * STEP)).collect::<Vec<_>>(), "4 full cards fit");
        let list = list_part(&screen, list_w, 1..h - 1);
        for i in 0..4 {
            let name_row = &list[i * STEP as usize + 1];
            assert!(name_row.contains(&format!("proj-t{i}")), "{name_row}");
        }
        assert!(screen[1].chars().nth(list_w - 1) == Some('╮') && screen[1].chars().nth(list_w) == Some('╭'), "{}", screen[1]);
        assert!(screen[1].contains("미리보기 · proj-t0"), "{}", screen[1]);
        assert!(screen[h - 2].chars().nth(list_w) == Some('╰') && screen[h - 2].ends_with('╯'), "{}", screen[h - 2]);
        let right: String = screen[2..h - 2].iter().map(|r| r.chars().skip(list_w + 1).collect::<String>()).collect::<Vec<_>>().join("\n");
        assert!(right.contains("first") && right.contains("❯ last prompt line"), "{right}");
        assert!(screen.last().unwrap().contains("▼ more"), "6 cards and their gap rows do not fit");
        assert!(!app.preview_too_small);
        // the read asks for the panel's inner height
        assert_eq!(app.preview_target().map(|t| t.lines), Some(FOUR_ROWS - 4));
        app.selected = 5;
        let buf = draw_buf(150, FOUR_ROWS, |f| draw(f, &mut app, 0, Instant::now()));
        let screen = rows(&buf);
        let list = list_part(&screen, list_w, 1..h - 1).join("\n");
        assert!(list.contains("proj-t5"), "{list}");
        let blue: Vec<_> = card_corners(&buf, CARD_W).into_iter().filter(|&p| buf[p].fg == style::SELECTED).collect();
        assert_eq!(blue, vec![(0, 1 + 3 * STEP)], "the selected (last) card is the bottom one");
        assert!(screen[1].contains("미리보기 · proj-t5"), "{}", screen[1]);
    }

    #[test]
    fn preview_list_of_twenty_keeps_the_selected_card_visible() {
        let mut app = twenty_sessions();
        app.preview = true;
        for sel in (0..20).chain((0..20).rev()) {
            app.selected = sel;
            let buf = draw_buf(150, 30, |f| draw(f, &mut app, 0, Instant::now()));
            let list = list_part(&rows(&buf), usize::from(CARD_W), 1..29).join("\n");
            assert!(list.contains(&format!("proj-t{sel} ")), "sel={sel} scroll={}: {list}", app.scroll);
            let blue = card_corners(&buf, CARD_W).into_iter().filter(|&p| buf[p].fg == style::SELECTED).count();
            assert_eq!(blue, 1, "sel={sel}: the selected full card is on screen");
            assert!(has_braille(&list));
        }
        assert_eq!(app.scroll, 0);
        app.selected = 19;
        render(&mut app, 150, 30, 0);
        assert_eq!(app.scroll, 18, "the last two cards shown");
    }

    #[test]
    fn preview_stacks_a_row_of_full_cards_on_top_of_preview_when_narrow_but_tall() {
        let mut app = preview_app(10);
        app.selected = 9;
        let buf = draw_buf(row_w(2), 32, |f| draw(f, &mut app, 0, Instant::now()));
        let screen = rows(&buf);
        // middle 30 rows: list clamp(30*2/5, CARD_H, CARD_H + 1) rows, the preview below
        let strip_end = usize::from(CARD_H) + 2;
        assert_eq!(app.grid_rect, Rect::new(0, 1, row_w(2), CARD_H + 1));
        assert_eq!(app.cols, crate::ui::grid::card_cols(row_w(2), CardLayout::Standard), "the usual grid inside the strip");
        assert!(screen[strip_end].starts_with('╭') && screen[strip_end].contains("미리보기 · proj-t9"), "{}", screen[strip_end]);
        assert!(screen[30].starts_with('╰'), "{}", screen[30]);
        assert!(screen.join("\n").contains(crate::ui::preview::PREVIEW_LOADING), "t9 not read yet");
        let strip = screen[1..strip_end].join("\n");
        assert!(strip.contains("proj-t9") && has_braille(&strip), "{strip}");
        assert!(screen[1].contains('╭') && screen[usize::from(CARD_H)].contains('╰'), "a full card row: {strip}");
        assert_eq!(app.preview_target().map(|t| t.lines), Some(30 - CARD_H - 3));
        // first row: two full cards across
        app.selected = 0;
        let screen = render(&mut app, row_w(2), 32, 0);
        assert_eq!(screen[1].matches('╭').count(), 2, "{}", screen[1]);
        let strip = screen[1..strip_end].join("\n");
        assert!(strip.contains("proj-t0") && strip.contains("proj-t1") && !strip.contains("proj-t2"), "{strip}");
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
        render(&mut app, 60, 26, 0); // middle 60x24 -> preview 24 - CARD_H rows
        assert_eq!(app.preview_target().map(|t| t.lines), Some(24 - CARD_H - 2));
    }

    #[test]
    fn toggling_preview_forces_one_column_and_restores_the_grid_keeping_selection() {
        let mut app = preview_app(12);
        app.preview = false;
        app.selected = 10;
        for _ in 0..2 {
            let screen = render(&mut app, 150, 30, 0);
            assert_eq!(app.grid_rect, Rect::new(0, 1, 150, 28));
            assert_eq!(app.cols, crate::ui::grid::card_cols(150, CardLayout::Standard), "normal grid");
            assert!(screen.join("\n").contains("proj-t10"));
            app.toggle_preview();
            assert!(app.preview);
            let screen = render(&mut app, 150, 30, 0);
            assert_eq!(app.cols, 1);
            assert_eq!(app.grid_rect.width, CARD_W);
            assert_eq!(app.selected_session().map(|s| s.id.as_str()), Some("t10"), "selection kept by id");
            assert!(list_part(&screen, usize::from(CARD_W), 1..29).join("\n").contains("proj-t10"), "still visible");
            // j/k move one card, h/l wrap to the neighbouring row and back
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
        // (w, h, full cards fit)
        for (w, h, cards) in [(110, 7, false), (110, 9, false), (60, 25, true), (75, 25, true)] {
            let mut app = preview_app(4);
            let screen = render(&mut app, w, h, 0);
            assert!(app.preview_too_small, "{w}x{h}");
            assert_eq!(app.grid_rect, Rect::new(0, 1, w, h - 2));
            let want_cols = if cards { crate::ui::grid::card_cols(w, CardLayout::Standard) } else { 1 };
            assert_eq!(app.cols, want_cols, "{w}x{h}: the normal grid (2 cards across at 60 and 75)");
            // the cards fill the row from its left edge
            assert_eq!(screen[1].starts_with('╭'), cards, "{w}x{h}: normal grid, not the preview list");
            assert!(screen.last().unwrap().contains(PREVIEW_TOO_SMALL), "{}", screen.last().unwrap());
            assert!(!screen[..h as usize - 1].join("\n").contains("미리보기 · "), "no panel");
        }
        // one more row / column and the panel is back (side: one column; stacked: the usual grid)
        for (w, h, cols) in [(110, 10, 1), (60, 26, 2), (75, 26, 2), (76, 10, 1)] {
            let mut app = preview_app(4);
            render(&mut app, w, h, 0);
            assert!(!app.preview_too_small && app.cols == cols, "{w}x{h}");
        }
    }

    #[test]
    fn preview_on_tiny_terminals_does_not_panic() {
        for n in [0, 1, 10] {
            let mut app = preview_app(n);
            app.selected = n.saturating_sub(1);
            for (w, h) in [(0, 0), (1, 1), (10, 3), (10, 26), (40, 26), (76, 10), (121, 3), (200, 5), (3, 60)] {
                assert_eq!(render(&mut app, w, h, 0).len(), h as usize);
            }
        }
        // side layout too low for one large card (middle 8 rows): compact one-line fallback
        use crate::ui::card::LARGE_W;
        let mut app = with_layout(preview_app(4), CardLayout::Large);
        let screen = render(&mut app, 120, 10, 0);
        assert_eq!((app.grid_rect.width, app.cols), (LARGE_W, 1));
        let list = list_part(&screen, usize::from(LARGE_W), 1..9);
        assert!(list[0].starts_with("▶ proj-t0") && list[3].contains("proj-t3"), "{list:?}");
        assert!(!list.join("").contains('╭') && !has_braille(&list.join("")), "{list:?}");
        assert!(!app.spirits_visible);
        // one more row holds a full standard card
        app.config.card_layout = CardLayout::Standard;
        let screen = render(&mut app, 120, 2 + CARD_H, 0);
        let list = list_part(&screen, usize::from(CARD_W), 1..usize::from(CARD_H) + 1);
        assert!(list[0].starts_with('╭') && has_braille(&list.join("")) && app.spirits_visible, "{list:?}");
    }

    fn windowed(id: &str, w: &str) -> crate::model::Session {
        let mut s = fake_session(id, Status::Running);
        s.window_id = Some(w.into());
        s
    }

    #[test]
    fn two_windows_draw_divider_lines_and_grouped_cards() {
        let mut app = App::new(Config::default());
        let snap = Snapshot {
            sessions: vec![windowed("x", "W2"), windowed("y", "W1"), windowed("z", "W2")],
            window_order: vec!["W1".into(), "W2".into()],
            current_window: Some("W2".into()),
            ..Default::default()
        };
        app.apply_snapshot(snap);
        let screen = render(&mut app, 120, 40, 0);
        assert!(screen[0].contains("SESSIONS 3"), "HUD counts stay global");
        let w = 120;
        let name_row = 1 + 1; // divider, border
        assert_eq!(screen[1], crate::ui::groups::divider_text("창 1", 1, w));
        assert!(screen[1].starts_with("── 창 1 · 세션 1 ─") && !screen[1].contains('╭'), "{}", screen[1]);
        assert!(screen[1 + name_row].contains("proj-y"), "{}", screen.join("\n"));
        let d2 = 2 + STEP as usize; // HUD, divider, the card row and its gap
        assert_eq!(screen[d2], crate::ui::groups::divider_text("창 2 (현재)", 2, w));
        assert!(screen[d2 + name_row].contains("proj-x") && screen[d2 + name_row].contains("proj-z"), "{}", screen.join("\n"));
        // dim, plain line
        let buf = draw_buf(120, 40, |f| draw(f, &mut app, 0, Instant::now()));
        assert_eq!(buf[(0, 1)].fg, style::TEXT_DIM);
        // one window only: no divider at all
        let one = Snapshot {
            sessions: vec![windowed("x", "W2"), windowed("z", "W2")],
            window_order: vec!["W1".into(), "W2".into()],
            ..Default::default()
        };
        app.apply_snapshot(one);
        let text = render(&mut app, 120, 40, 0).join("\n");
        assert!(!text.contains("창 "), "{text}");
    }

    #[test]
    fn one_column_list_scrolls_to_selected_with_group_dividers() {
        let mut app = App::new(Config::default());
        let mut sessions: Vec<_> = (0..12).map(|i| windowed(&format!("a{i}"), "W1")).collect();
        sessions.extend((0..12).map(|i| windowed(&format!("b{i}"), "W2")));
        app.apply_snapshot(Snapshot { sessions, window_order: vec!["W1".into(), "W2".into()], ..Default::default() });
        app.preview = true;
        // 150x22: middle 150x20 -> list CARD_W x 20, full cards: D + 12*STEP + D + 12*STEP - 1 rows
        app.selected = 23;
        let screen = render(&mut app, 150, 22, 0);
        assert_eq!((app.grid_rect.width, app.cols), (CARD_W, 1));
        let list = list_part(&screen, usize::from(CARD_W), 1..21).join("\n");
        assert!(list.contains("proj-b11") && list.contains("proj-b10"), "{list}");
        assert!(screen.last().unwrap().contains("▲ more"));
        // first card of group 2: its divider is shown right above it
        app.selected = 12;
        let screen = render(&mut app, 150, 22, 0);
        let list = list_part(&screen, usize::from(CARD_W), 1..21);
        let d = list.iter().position(|r| r.starts_with("── 창 2 · 세션 12")).expect("divider of group 2");
        assert!(list[d + 1].starts_with('╭') && list[d + 2].contains("proj-b0"), "{list:?}");
        // j walks every card in order, across the divider
        app.selected = 0;
        let mut seen = vec![0];
        for _ in 0..25 {
            app.apply_move(Dir::Down);
            render(&mut app, 150, 22, 0);
            seen.push(app.selected);
        }
        let want: Vec<usize> = (0..24).chain([23, 23]).collect();
        assert_eq!(seen, want);
        for sel in (0..24).rev() {
            app.selected = sel;
            let buf = draw_buf(150, 22, |f| draw(f, &mut app, 0, Instant::now()));
            let id = &app.sessions[sel].id;
            let blue: Vec<_> = card_corners(&buf, CARD_W).into_iter().filter(|&p| buf[p].fg == style::SELECTED).collect();
            assert_eq!(blue.len(), 1, "sel={sel}");
            let name_row = &rows(&buf)[usize::from(blue[0].1) + 1];
            assert!(name_row.contains(&format!("proj-{id} ")), "sel={sel}: {name_row}");
        }
        assert_eq!(app.scroll, 0);
        assert!(render(&mut app, 150, 22, 0)[1].starts_with("── 창 1 · 세션 12"));
    }

    // ---- both card layouts ----

    fn with_layout(mut app: App, layout: CardLayout) -> App {
        app.config.card_layout = layout;
        app
    }

    #[test]
    fn large_layout_grid_at_120x40() {
        use crate::ui::card::{LARGE_H, LARGE_W};
        let mut app = with_layout(App::new(Config::default()), CardLayout::Large);
        app.apply_snapshot(Snapshot { sessions: ten_sessions(), ..Default::default() });
        let buf = draw_buf(120, 40, |f| draw(f, &mut app, 0, Instant::now()));
        let screen = rows(&buf);
        assert_eq!(app.cols, 2, "120 / 46");
        // the cards fill the width: 59 + 2 + 59 columns; rows of 10 + a gap row
        let corners: Vec<(u16, u16)> = card_corners(&buf, 120);
        let want: Vec<(u16, u16)> = [1u16, 12, 23].iter().flat_map(|&y| [0u16, 61].map(|x| (x, y))).collect();
        assert_eq!(corners, want, "{}", screen.join("\n"));
        for (&(x, y), w) in corners.iter().zip([59u16, 59].iter().cycle()) {
            assert!(*w >= LARGE_W);
            assert_eq!(buf[(x + w - 1, y)].symbol(), "╮");
            assert_eq!(buf[(x, y + LARGE_H - 1)].symbol(), "╰");
            assert_eq!(buf[(x + w - 1, y + LARGE_H - 1)].symbol(), "╯");
        }
        assert_large_gaps(&buf, &[59, 60], 1..34);
        assert!(screen[11].trim().is_empty() && screen[22].trim().is_empty(), "gap rows: {}", screen.join("\n"));
        assert!(screen[1 + 1].contains("proj-t0"), "{}", screen.join("\n"));
        assert!(has_braille(&screen.join("\n")) && app.spirits_visible);
        assert!(screen.last().unwrap().contains("▼ more"), "6 of 10 cards fit");
        assert_eq!(buf[(1, 1)].fg, style::SELECTED);
    }

    /// The gap columns between large cards stay blank on rows `ys`.
    fn assert_large_gaps(buf: &ratatui::buffer::Buffer, gaps: &[u16], ys: std::ops::Range<u16>) {
        for &x in gaps {
            for y in ys.clone() {
                assert_eq!(buf[(x, y)].symbol(), " ", "gap column {x} row {y}: {}", rows(buf).join("\n"));
            }
        }
    }

    #[test]
    fn large_cards_fill_the_width_with_two_column_gaps_at_143() {
        use crate::ui::card::{LARGE_H, LARGE_SPIRIT_W, SCENE_ROWS};
        let mut app = with_layout(twenty_sessions(), CardLayout::Large);
        let buf = draw_buf(143, 40, |f| draw(f, &mut app, 0, Instant::now()));
        assert_eq!(app.cols, 3);
        // 46 + 2 + 46 + 2 + 47 = 143
        let lefts = [0u16, 48, 96];
        let corners = card_corners(&buf, 143);
        assert_eq!(corners, [1u16, 12, 23].iter().flat_map(|&y| lefts.map(|x| (x, y))).collect::<Vec<_>>(), "{}", rows(&buf).join("\n"));
        for &(x, y) in &corners {
            let w = if x == 96 { 47 } else { 46 };
            assert_eq!(buf[(x + w - 1, y)].symbol(), "╮", "({x},{y})");
            assert_eq!(buf[(x + w - 1, y + LARGE_H - 1)].symbol(), "╯", "({x},{y})");
        }
        assert_eq!(buf[(142, 1)].symbol(), "╮", "the last card reaches the right edge");
        assert_large_gaps(&buf, &[46, 47, 94, 95], 1..34);
        // the spirit stays inside each card's spirit area
        for y in 1..34u16 {
            for x in 0..143u16 {
                if buf[(x, y)].symbol().chars().any(|c| ('\u{2801}'..='\u{28ff}').contains(&c)) {
                    let row = (y - 1) % (LARGE_H + GAP_Y);
                    assert!((1..=SCENE_ROWS).contains(&row), "braille at ({x},{y}) outside the spirit rows");
                    let left = lefts.into_iter().rfind(|&l| l <= x).unwrap();
                    assert!((left + 1..=left + LARGE_SPIRIT_W).contains(&x), "braille at ({x},{y}) outside the spirit area");
                }
            }
        }
    }

    #[test]
    fn grid_cols_and_scroll_follow_the_layout() {
        for layout in [CardLayout::Standard, CardLayout::Large] {
            for w in [60u16, 100, 150] {
                let mut app = with_layout(twenty_sessions(), layout);
                render(&mut app, w, 40, 0);
                assert_eq!(app.cols, crate::ui::grid::card_cols(w, layout), "{layout:?} {w}");
            }
            // 20 sessions: walking every card keeps the selected one fully on screen
            let mut app = with_layout(twenty_sessions(), layout);
            let (_, card_h) = crate::ui::card::card_size(layout);
            for sel in (0..20).chain((0..20).rev()) {
                app.selected = sel;
                let buf = draw_buf(100, 30, |f| draw(f, &mut app, 0, Instant::now()));
                let blue: Vec<(u16, u16)> = card_corners(&buf, 100).into_iter().filter(|&p| buf[p].fg == style::SELECTED).collect();
                assert_eq!(blue.len(), 1, "{layout:?} sel={sel} scroll={}", app.scroll);
                assert!(blue[0].1 + card_h <= 29, "{layout:?} sel={sel}: the whole card is above the footer");
                let text = rows(&buf)[1..29].join("\n");
                assert!(text.contains(&format!("proj-t{sel} ")), "{layout:?} sel={sel}: {text}");
            }
            assert_eq!(app.scroll, 0);
        }
    }

    /// At every height a full card fits (the gap rows must not push it out),
    /// walking down and back keeps the selected card whole above the footer,
    /// with and without group dividers, in the grid and the preview list.
    #[test]
    fn scrolling_keeps_the_selected_card_whole_at_every_height() {
        let windows = |n: usize| -> Vec<crate::model::Session> { (0..n).map(|i| windowed(&format!("t{i}"), if i < n / 2 { "W1" } else { "W2" })).collect() };
        for layout in [CardLayout::Standard, CardLayout::Large] {
            let (_, card_h) = crate::ui::card::card_size(layout);
            for grouped in [false, true] {
                for (w, preview) in [(150u16, false), (150, true), (60, false)] {
                    for h in card_h + 2..=44 {
                        let mut app = with_layout(App::new(Config::default()), layout);
                        let window_order = if grouped { vec!["W1".to_string(), "W2".to_string()] } else { Vec::new() };
                        let sessions = if grouped { windows(8) } else { (0..8).map(|i| fake_session(&format!("t{i}"), Status::Running)).collect() };
                        app.apply_snapshot(Snapshot { sessions, window_order, ..Default::default() });
                        app.preview = preview;
                        for sel in (0..8).chain((0..8).rev()) {
                            app.selected = sel;
                            let buf = draw_buf(w, h, |f| draw(f, &mut app, 0, Instant::now()));
                            let footer_y = h - 1;
                            let list = app.grid_rect;
                            let blue: Vec<(u16, u16)> = card_corners(&buf, list.width).into_iter().filter(|&p| buf[p].fg == style::SELECTED).collect();
                            let what = format!("{layout:?} grouped={grouped} {w}x{h} preview={preview} sel={sel} scroll={}", app.scroll);
                            assert_eq!(blue.len(), 1, "{what}: {}", rows(&buf).join("\n"));
                            let (x, y) = blue[0];
                            assert!(y + card_h <= footer_y && y + card_h <= list.bottom(), "{what}: the whole card is on screen");
                            assert_eq!(buf[(x, y + card_h - 1)].symbol(), "╰", "{what}: its bottom border is drawn");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn spirits_render_only_for_visible_cards_in_both_layouts() {
        use crate::ui::card::{reset_spirit_renders, spirit_renders};
        // (layout, normal 142x30, scrolled to the end, preview 150x30): middle 28 rows,
        // standard: 4 across, 2 rows of 10; large: 3 across, 2 rows of 11, the last holds 2
        for (layout, normal, scrolled, side) in [(CardLayout::Standard, 8, 8, 2), (CardLayout::Large, 6, 5, 2)] {
            let mut app = with_layout(twenty_sessions(), layout);
            reset_spirit_renders();
            render(&mut app, 142, 30, 0);
            assert_eq!(spirit_renders(), normal, "{layout:?}: only visible cards");
            app.selected = 19;
            reset_spirit_renders();
            render(&mut app, 142, 30, 1);
            assert_eq!(spirit_renders(), scrolled, "{layout:?}: scrolled");
            app.preview = true;
            reset_spirit_renders();
            render(&mut app, 150, 30, 2);
            assert_eq!(spirit_renders(), side, "{layout:?}: preview list");
            assert!(app.spirits_visible);
        }
    }

    #[test]
    fn preview_with_the_large_layout_side_and_stacked() {
        use crate::ui::card::{LARGE_H, LARGE_W};
        let mut app = with_layout(preview_app(6), CardLayout::Large);
        let buf = draw_buf(120, 40, |f| draw(f, &mut app, 0, Instant::now()));
        let screen = rows(&buf);
        assert_eq!(app.grid_rect, Rect::new(0, 1, LARGE_W, 38), "the side list is one large card wide");
        assert_eq!(app.cols, 1);
        assert_eq!(card_corners(&buf, LARGE_W), vec![(0, 1), (0, 1 + LARGE_H + GAP_Y), (0, 1 + 2 * (LARGE_H + GAP_Y))], "three 10-row cards and their gaps fit 38 rows: {}", screen.join("\n"));
        assert!(screen[1].chars().nth(45) == Some('╮') && screen[1].chars().nth(46) == Some('╭'), "{}", screen[1]);
        assert!(screen[1].contains("미리보기 · proj-t0"), "{}", screen[1]);
        assert_eq!(app.preview_target().map(|t| t.lines), Some(36));
        // stacked (narrow but tall): one row of large cards on top
        app.selected = 5;
        let screen = render(&mut app, 60, 32, 0);
        // middle 60x30: strip clamp(30*2/5, 10, 11) = 11 rows, preview 19 below
        assert_eq!(app.grid_rect, Rect::new(0, 1, 60, LARGE_H + 1));
        assert_eq!(app.cols, crate::ui::grid::card_cols(60, CardLayout::Large));
        let strip = screen[1..12].join("\n");
        assert!(strip.contains("proj-t5") && has_braille(&strip), "{strip}");
        assert!(screen[1].starts_with('╭') && screen[1].chars().nth(59) == Some('╮'), "one card fills the strip: {}", screen[1]);
        assert!(screen[12].starts_with('╭') && screen[12].contains("미리보기 · proj-t5"), "{}", screen[12]);
        // v flips the list between the layouts while the preview is on
        app.config.card_layout = CardLayout::Standard;
        render(&mut app, 120, 40, 0);
        assert_eq!(app.grid_rect.width, crate::ui::preview::LIST_MIN_WIDTH);
    }

    #[test]
    fn tiny_terminals_fall_back_to_compact_cards_in_the_large_layout() {
        use crate::ui::card::{LARGE_H, LARGE_W};
        let mut app = with_layout(App::new(Config::default()), CardLayout::Large);
        app.set_sessions(ten_sessions());
        app.selected = 9;
        // middle one row short of a large card, and 20 rows but one column short: one-line cards
        for (w, h) in [(LARGE_W, LARGE_H + 1), (LARGE_W - 1, 22)] {
            let screen = render(&mut app, w, h, 0);
            let text = screen.join("\n");
            assert!(!text.contains('╭') && !has_braille(&text), "{w}x{h}: {text}");
            assert!(text.contains("proj-t9"), "{w}x{h}: selection visible: {text}");
            assert!(!app.spirits_visible);
        }
        // exactly one large card fits
        let screen = render(&mut app, LARGE_W, LARGE_H + 2, 0).join("\n");
        assert!(screen.contains('╭') && has_braille(&screen), "{screen}");
        for (w, h) in [(0, 0), (1, 1), (5, 2), (LARGE_W - 1, LARGE_H + 2), (LARGE_W, LARGE_H + 1), (141, LARGE_H + 2), (200, 3), (3, 60)] {
            for preview in [false, true] {
                app.preview = preview;
                assert_eq!(render(&mut app, w, h, u64::MAX).len(), h as usize);
            }
        }
    }

    /// Text screenshots of both layouts in the normal and the preview mode
    /// (`cargo test dump_card_layouts -- --ignored --nocapture`).
    #[test]
    #[ignore]
    fn dump_card_layouts() {
        let sessions = || -> Vec<crate::model::Session> {
            let statuses = [Status::Running, Status::NeedsInput, Status::Idle, Status::Done, Status::Running, Status::Unknown];
            let models = ["claude-opus-4-5", "claude-fable-5-1", "claude-sonnet-5-5", "claude-haiku-5-5"];
            let sums = ["로그인 화면의 토큰 갱신 버그를 고치고 테스트를 추가하는 중", "fixing the flaky parser test", "", "README 정리"];
            (0..10)
                .map(|i| {
                    let mut s = fake_session(&format!("t{i}"), statuses[i % statuses.len()]);
                    s.model = Some(models[i % 4].into());
                    s.summary = Some(sums[i % 4].into()).filter(|x: &String| !x.is_empty());
                    s.branch = Some(format!("feat/task-{i}"));
                    s.ctx_percent = Some([12.0, 45.0, 80.0][i % 3]);
                    if i % 3 == 0 {
                        s.pr = Some(crate::model::Pr { number: 100 + i as u64, title: "x".into(), state: crate::model::PrState::ReviewPending });
                    }
                    s
                })
                .collect()
        };
        for layout in [CardLayout::Standard, CardLayout::Large] {
            for (preview, w) in [(false, 120u16), (false, 111), (true, 120)] {
                let mut cfg = Config::default();
                cfg.card_layout = layout;
                cfg.extras.pr = true;
                let mut app = App::new(cfg);
                app.apply_snapshot(Snapshot { sessions: sessions(), ..Default::default() });
                if preview {
                    app.toggle_preview();
                    let (cache, _) = app.preview_shared();
                    cache.put("t0", crate::preview::PreviewEntry { screen: Ok("$ cargo test\n   Compiling demo v0.1.0\n    Finished test\ntest result: ok. 12 passed\n\n❯ ".into()), fetched_at: Instant::now() });
                }
                println!("--- {layout:?}, preview {preview}, {w}x40 ---");
                for r in render(&mut app, w, 40, 44) {
                    println!("{}", r.trim_end());
                }
            }
        }
    }

    /// Text look of the preview layout. Run with
    /// `cargo test dump_preview_layout -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn dump_preview_layout() {
        for (w, h) in [(120u16, 40u16), (80, 24)] {
            let mut app = preview_app(6);
            println!("--- {w}x{h} ---");
            for r in render(&mut app, w, h, 0) {
                println!("{}", r.trim_end());
            }
        }
    }

    #[test]
    fn footer_shows_flash_message() {
        let mut app = App::new(Config::default());
        app.set_sessions(ten_sessions());
        app.notify("이동할 수 없는 세션", Instant::now());
        let screen = render(&mut app, 120, 30, 0);
        assert!(screen.last().unwrap().contains("이동할 수 없는 세션"));
    }
}
