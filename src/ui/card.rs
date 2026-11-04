//! Session cards: an element spirit (braille, see [`crate::spirit`]) on top
//! and info lines below, in a thin rounded border.
//!
//! Full card: [`CARD_W`] x [`CARD_H`] (borders included): a spirit band of
//! [`SCENE_ROWS`] rows (room for the largest spirit) plus [`INFO_ROWS`] text
//! rows. Info rows in priority order: name, status badge (+ elapsed / last
//! activity), branch, then PR or summary (PR first; rows of disabled or
//! unknown extras collapse).
//!
//! When a card does not fit (or the preview is on) a compact one-line card is
//! drawn instead ([`render_compact`]). Empty areas draw nothing; no size panics.

use std::time::SystemTime;

use ratatui::layout::{Margin, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use crate::config::Config;
use crate::model::{PrState, Session, Status};
use crate::spirit::theme::Theme;
use crate::spirit::{self, max_grid_size, Size, SpiritParams};
use crate::ui::scene::draw_grid;
use crate::ui::style::{self, fmt_duration, truncate_to_width};

/// Spirit band size in cells: room for the largest spirit.
pub const SCENE_COLS: u16 = max_grid_size().0;
pub const SCENE_ROWS: u16 = max_grid_size().1;
/// Narrowest inner width: the info lines keep the previous card width.
const MIN_INNER_W: u16 = 48;
/// Text rows below the scene.
pub const INFO_ROWS: u16 = 4;
/// Full card size, borders included.
pub const CARD_W: u16 = if SCENE_COLS > MIN_INNER_W { SCENE_COLS } else { MIN_INNER_W } + 2;
pub const CARD_H: u16 = SCENE_ROWS + INFO_ROWS + 2;

/// Whether the grid draws compact one-line cards: always while the preview
/// lists sessions in one column, else when `area` cannot hold one full card.
pub fn compact_mode(one_column: bool, area: Rect) -> bool {
    one_column || area.width < CARD_W || area.height < CARD_H
}

/// The card's element: a per-card choice (keyed by project dir, then session
/// id) or the configured default.
fn theme_for(s: &Session, cfg: &Config) -> Theme {
    let cwd = s.cwd.to_string_lossy();
    cfg.card_themes.get(cwd.as_ref()).or_else(|| cfg.card_themes.get(&s.id)).copied().unwrap_or(cfg.theme)
}

/// Stable per-session seed (FNV-1a of the session id).
fn seed_for(s: &Session) -> u32 {
    s.id.bytes().fold(0x811c_9dc5u32, |h, b| (h ^ u32::from(b)).wrapping_mul(0x0100_0193))
}

/// Draws one session card with its spirit at animation frame `tick`. Too
/// small an area falls back to [`render_compact`].
pub fn render_card(f: &mut Frame, area: Rect, s: &Session, cfg: &Config, selected: bool, tick: u64) {
    let rect = area.intersection(f.area());
    if rect.is_empty() {
        return;
    }
    if rect.width < CARD_W || rect.height < CARD_H {
        render_compact(f, rect, s, cfg, selected);
        return;
    }
    let done = s.status == Status::Done;
    let mut block = style::window_block(style::status_color(s.status), selected);
    if done && !selected {
        block = block.border_style(Style::new().fg(style::DONE).add_modifier(Modifier::DIM));
    }
    let inner = block.inner(rect);
    f.render_widget(Clear, rect);
    f.render_widget(block, rect);

    // Spirit band: the spirit is centered in it.
    let band = Rect { height: SCENE_ROWS.min(inner.height), ..inner };
    let params = SpiritParams { theme: theme_for(s, cfg), status: s.status, size: Size::S, tick, seed: seed_for(s), warning: false };
    let grid = spirit::render(&params);
    let (cols, rows) = (grid.cols.min(band.width), grid.rows.min(band.height));
    let scene_rect = Rect::new(band.x + (band.width - cols) / 2, band.y + (band.height - rows) / 2, cols, rows);
    draw_grid(f.buffer_mut(), scene_rect, &grid);

    let text_area = Rect::new(inner.x, inner.y + band.height, inner.width, inner.height - band.height)
        .inner(Margin::new(1, 0));
    let mut lines = text_lines(s, cfg, usize::from(text_area.width), SystemTime::now());
    lines.truncate(usize::from(INFO_ROWS.min(text_area.height)));
    let mut para = Paragraph::new(lines);
    if done {
        para = para.style(Style::new().add_modifier(Modifier::DIM));
    }
    f.render_widget(para, text_area);
}

/// One-line card: `▶ name  STATUS  3h` (cursor only when selected). The
/// name is kept first; elapsed, then the badge, are dropped when too narrow.
pub fn render_compact(f: &mut Frame, area: Rect, s: &Session, cfg: &Config, selected: bool) {
    let area = area.intersection(f.area());
    if area.is_empty() {
        return;
    }
    let width = usize::from(area.width);
    let color = if selected { style::SELECTED } else { style::status_color(s.status) };
    let mut spans = Vec::new();
    let mut avail = width;
    if width >= 3 {
        let mark = if selected { style::CURSOR } else { " " };
        spans.push(Span::styled(format!("{mark} "), Style::new().fg(style::SELECTED).add_modifier(Modifier::BOLD)));
        avail -= 2;
    }
    let tag = format!(" {} ", style::status_tag(s.status));
    let elapsed = cfg.extras.elapsed.then(|| elapsed_text(s, SystemTime::now())).flatten().map(|d| format!(" {d}"));
    let name_w = s.name.width();
    // Keep at least a few name columns before adding the badge and elapsed.
    let need = name_w.min(6) + 1;
    let tag_w = tag.width();
    let el_w = elapsed.as_deref().map_or(0, str::width);
    let (show_tag, show_el) = if need + tag_w + el_w <= avail {
        (true, true)
    } else {
        (need + tag_w <= avail, false)
    };
    let tail_w = if show_tag { 1 + tag_w } else { 0 } + if show_el { el_w } else { 0 };
    let name = truncate_to_width(&s.name, avail - tail_w);
    let name_style = Style::new().fg(color).add_modifier(Modifier::BOLD);
    spans.push(Span::styled(name, name_style));
    if show_tag {
        spans.push(Span::raw(" "));
        spans.push(Span::styled(
            tag,
            Style::new().fg(style::BADGE_FG).bg(style::status_color(s.status)).add_modifier(Modifier::BOLD),
        ));
    }
    if let Some(e) = elapsed.filter(|_| show_el) {
        spans.push(Span::styled(e, Style::new().fg(style::TEXT)));
    }
    let mut para = Paragraph::new(Line::from(spans));
    if s.status == Status::Done {
        para = para.style(Style::new().add_modifier(Modifier::DIM));
    }
    f.render_widget(para, Rect { height: 1, ..area });
}

fn since(t: Option<SystemTime>, now: SystemTime) -> Option<String> {
    t.map(|t| fmt_duration(now.duration_since(t).map(|d| d.as_secs()).unwrap_or(0)))
}

/// Time since the session started; a finished session's clock stops at its end.
fn elapsed_text(s: &Session, now: SystemTime) -> Option<String> {
    let end = match (s.status, s.ended_at) {
        (Status::Done, Some(end)) => end,
        _ => now,
    };
    since(s.started_at, end)
}

fn pr_label(state: PrState) -> &'static str {
    match state {
        PrState::Draft => "draft",
        PrState::ReviewPending => "review",
        PrState::Approved => "approved",
        PrState::Conflict => "conflict",
        PrState::Unknown => "?",
    }
}

/// Info lines in priority order (callers cut the tail to their row budget):
/// name, status badge (+ elapsed / last activity), then the enabled extras:
/// branch (`~` when guessed), PR, one-line summary.
fn text_lines(s: &Session, cfg: &Config, width: usize, now: SystemTime) -> Vec<Line<'static>> {
    let ex = &cfg.extras;
    let color = style::status_color(s.status);
    let text = Style::new().fg(style::TEXT);
    let dim = Style::new().fg(style::TEXT_DIM);
    let mut lines = vec![Line::styled(truncate_to_width(&s.name, width), text.add_modifier(Modifier::BOLD))];

    // Badge line: whole tokens only. When too narrow, last activity goes
    // first, then elapsed; the badge itself is truncated as a last resort.
    let tag = truncate_to_width(&format!(" {} ", style::status_tag(s.status)), width);
    let mut used = tag.width();
    let mut badge = vec![Span::styled(tag, Style::new().fg(style::BADGE_FG).bg(color).add_modifier(Modifier::BOLD))];
    let elapsed = ex.elapsed.then(|| elapsed_text(s, now)).flatten().map(|d| format!(" {d}"));
    let last = ex.last_activity.then(|| since(s.updated_at, now)).flatten().map(|d| format!(" ·{d} ago"));
    let ew = elapsed.as_deref().map_or(0, str::width);
    let lw = last.as_deref().map_or(0, str::width);
    let (keep_elapsed, keep_last) = if used + ew + lw <= width {
        (true, true)
    } else {
        (used + ew <= width, false)
    };
    if let Some(e) = elapsed.filter(|_| keep_elapsed) {
        used += ew;
        badge.push(Span::styled(e, text));
    }
    if let Some(l) = last.filter(|_| keep_last && used + lw <= width) {
        badge.push(Span::styled(l, dim));
    }
    lines.push(Line::from(badge));

    if ex.branch {
        if let Some(b) = &s.branch {
            let tilde = if s.branch_guessed { "~" } else { "" };
            lines.push(Line::styled(truncate_to_width(&format!("⎇ {tilde}{b}"), width), text));
        }
    }
    if ex.pr {
        if let Some(pr) = &s.pr {
            let pr_color = match pr.state {
                PrState::Approved => style::DONE,
                PrState::Conflict => style::FLASH_ALERT,
                _ => style::TEXT_DIM,
            };
            lines.push(Line::styled(truncate_to_width(&format!("#{} {}", pr.number, pr_label(pr.state)), width), Style::new().fg(pr_color)));
        } else if s.pr_unavailable {
            // The lookup failed; the footer says why.
            lines.push(Line::styled(truncate_to_width("PR ?", width), dim));
        }
    }
    if ex.summary {
        if let Some(sum) = &s.summary {
            lines.push(Line::styled(truncate_to_width(sum, width), dim));
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Extras;
    use crate::model::Pr;
    use crate::ui::test_util::{draw, rows};
    use ratatui::style::Color;
    use std::path::PathBuf;
    use std::time::Duration;

    fn session(name: &str, status: Status) -> Session {
        let now = SystemTime::now();
        Session {
            id: "sess-1".into(),
            name: name.into(),
            status,
            pid: 4242,
            cwd: PathBuf::from("/home/user/proj-a"),
            workspace_id: "ws-1".into(),
            surface_id: "sf-1".into(),
            started_at: Some(now - Duration::from_secs(3 * 3600 + 10)),
            updated_at: Some(now - Duration::from_secs(125)),
            ended_at: None,
            summary: Some("fixing the parser".into()),
            branch: Some("feat/login".into()),
            branch_guessed: false,
            pr: Some(Pr { number: 42, title: "Add login".into(), state: PrState::Approved }),
            pr_unavailable: false,
            transcript: None,
            window_id: None,
        }
    }

    fn all_extras() -> Config {
        Config {
            extras: Extras { branch: true, elapsed: true, last_activity: true, summary: true, pr: true },
            ..Config::default()
        }
    }

    fn no_extras() -> Config {
        Config {
            extras: Extras { branch: false, elapsed: false, last_activity: false, summary: false, pr: false },
            ..Config::default()
        }
    }

    fn is_braille(c: char) -> bool {
        ('\u{2801}'..='\u{28ff}').contains(&c)
    }

    fn card(s: &Session, cfg: &Config, selected: bool) -> Vec<String> {
        rows(&draw(CARD_W, CARD_H, |f| render_card(f, f.area(), s, cfg, selected, 0)))
    }

    /// Text of an info row: border and padding stripped.
    fn info(row: &str) -> String {
        row.chars().skip(2).collect::<String>().trim_end_matches('│').trim_end().to_string()
    }

    #[test]
    fn card_size_fits_the_largest_spirit() {
        assert_eq!((SCENE_COLS, SCENE_ROWS), max_grid_size());
        assert!(CARD_W >= SCENE_COLS + 2);
        assert_eq!(CARD_H, SCENE_ROWS + INFO_ROWS + 2);
    }

    #[test]
    fn card_shows_scene_and_info_lines() {
        let s = session("proj-a", Status::Running);
        let rows = rows(&draw(80, 24, |f| render_card(f, Rect::new(0, 0, CARD_W, CARD_H), &s, &Config::default(), false, 0)));
        let text = rows.join("\n");
        let last = CARD_H as usize - 1;
        assert!(rows[0].starts_with('╭') && rows[last].starts_with('╰'), "thin rounded border: {text}");
        let scene_rows: Vec<usize> =
            (1..CARD_H as usize - 1).filter(|&y| rows[y].chars().any(is_braille)).collect();
        assert!(!scene_rows.is_empty(), "{text}");
        let name_row = rows.iter().position(|r| r.contains("proj-a")).expect(&text);
        assert!(scene_rows.iter().all(|&y| y < name_row), "scene above the info lines: {text}");
        assert_eq!(name_row, 1 + SCENE_ROWS as usize, "name right below the scene");
        assert!(text.contains("RUN") && text.contains("feat/login"), "{text}");
        // default extras: branch, elapsed and summary shown; PR hidden
        assert!(text.contains("3h") && text.contains("fixing the parser") && !text.contains("#42"), "{text}");
        // nothing outside the card rect
        assert!(rows[last + 1..].iter().all(|r| r.trim().is_empty()));
        assert!(rows[..=last].iter().all(|r| r.chars().skip(CARD_W as usize).all(|c| c == ' ')));
    }

    #[test]
    fn spirit_is_centered_and_paints_no_background() {
        let s = session("proj-a", Status::Idle);
        let w = CARD_W + 10;
        let buf = draw(w, CARD_H, |f| render_card(f, f.area(), &s, &Config::default(), false, 0));
        let lit: Vec<u16> =
            (1..=SCENE_ROWS).flat_map(|y| (1..w - 1).map(move |x| (x, y))).filter(|&(x, y)| buf[(x, y)].symbol().chars().any(is_braille)).map(|(x, _)| x).collect();
        assert!(!lit.is_empty());
        let (lo, hi) = (*lit.iter().min().unwrap(), *lit.iter().max().unwrap());
        assert!(lo > 1 && hi < w - 2, "spirit inside the band: {lo}..{hi}");
        assert!(((lo + hi) / 2).abs_diff(w / 2) <= 3, "roughly centered: {lo}..{hi}");
        for y in 1..=SCENE_ROWS {
            for x in 1..w - 1 {
                assert_eq!(buf[(x, y)].bg, ratatui::style::Color::Reset, "({x},{y})");
            }
        }
    }

    #[test]
    fn card_spirit_uses_the_card_theme_and_animates() {
        let s = session("proj-a", Status::Running);
        let scene = |cfg: &Config, tick: u64| -> Vec<String> { rows(&draw(CARD_W, CARD_H, |f| render_card(f, f.area(), &s, cfg, false, tick)))[1..=SCENE_ROWS as usize].to_vec() };
        let fg = |cfg: &Config| -> Vec<ratatui::style::Color> {
            let buf = draw(CARD_W, CARD_H, |f| render_card(f, f.area(), &s, cfg, false, 3));
            (1..=SCENE_ROWS).flat_map(|y| (1..CARD_W - 1).map(move |x| (x, y))).map(|p| buf[p].fg).collect()
        };
        let mut water = Config::default();
        water.card_themes.insert("/home/user/proj-a".into(), Theme::Water);
        assert_ne!(fg(&Config::default()), fg(&water), "per-card theme by project dir");
        assert!((1..30).any(|t| scene(&Config::default(), t) != scene(&Config::default(), 0)), "tick moves the spirit");
    }

    #[test]
    fn card_seeds_are_full_range_and_spirits_still_move() {
        for id in ["sess-1", "4f1c2a9e-session", "zzzz"] {
            let mut s = session("proj-a", Status::Done);
            s.id = id.into();
            assert_eq!(seed_for(&s), seed_for(&s.clone()), "stable");
            // DONE bounce: the spirit band changes over a bounce period
            let band = |t: u64| -> Vec<String> { rows(&draw(CARD_W, CARD_H, |f| render_card(f, f.area(), &s, &Config::default(), false, t)))[1..=SCENE_ROWS as usize].to_vec() };
            let frames: std::collections::BTreeSet<Vec<String>> = (0..30).map(band).collect();
            assert!(frames.len() > 3, "{id} (seed {}): {} distinct frames", seed_for(&s), frames.len());
        }
        let s = session("proj-a", Status::Running);
        assert!(seed_for(&s) > 1 << 24, "realistic seeds exceed f32 integer precision");
    }

    #[test]
    fn info_rows_keep_pr_over_summary_and_hide_disabled_extras() {
        let s = session("proj-a", Status::Running);
        let rows = card(&s, &all_extras(), false);
        let first = 1 + SCENE_ROWS as usize;
        let text: Vec<String> = rows[first..first + 4].iter().map(|r| info(r)).collect();
        assert_eq!(text, vec!["proj-a", " RUN  3h ·2m ago", "⎇ feat/login", "#42 approved"]);
        let mut cfg = all_extras();
        cfg.extras.pr = false;
        let all = card(&s, &cfg, false).join("\n");
        assert!(all.contains("fixing the parser") && !all.contains("#42"), "{all}");
        let hidden = card(&s, &no_extras(), false).join("\n");
        assert!(hidden.contains("proj-a") && hidden.contains("RUN"), "{hidden}");
        for gone in ["feat/login", "fixing the parser", "#42", "3h", "2m"] {
            assert!(!hidden.contains(gone), "{gone} should be hidden: {hidden}");
        }
    }

    #[test]
    fn badge_line_drops_last_activity_then_elapsed_instead_of_cutting() {
        let now = SystemTime::now();
        let mut s = session("proj-a", Status::NeedsInput);
        s.started_at = Some(now - Duration::from_secs(400 * 86_400 + 5));
        let flat = |w: usize| -> String {
            let lines = text_lines(&s, &all_extras(), w, now);
            lines[1].spans.iter().map(|sp| sp.content.as_ref()).collect()
        };
        assert_eq!(flat(20), " INPUT  400d ·2m ago");
        assert_eq!(flat(19), " INPUT  400d");
        assert_eq!(flat(12), " INPUT  400d");
        assert_eq!(flat(11), " INPUT ");
        assert_eq!(flat(7), " INPUT ");
        assert_eq!(flat(4), " IN…");
        assert_eq!(flat(0), "");
    }

    #[test]
    fn pr_question_mark_is_dim_when_lookup_unavailable() {
        let mut s = session("proj-a", Status::Running);
        s.pr = None;
        s.pr_unavailable = true;
        let buf = draw(CARD_W, CARD_H, |f| render_card(f, f.area(), &s, &all_extras(), false, 0));
        let text = rows(&buf);
        let y = text.iter().position(|r| r.contains("PR ?")).expect(&text.join("\n"));
        let x = text[y].find("PR ?").map(|b| text[y][..b].chars().count()).unwrap() as u16;
        assert_eq!(buf[(x, y as u16)].fg, style::TEXT_DIM);
        s.pr_unavailable = false;
        assert!(!card(&s, &all_extras(), false).join("\n").contains("PR ?"));
        s.pr_unavailable = true;
        assert!(!card(&s, &Config::default(), false).join("\n").contains("PR ?"), "PR extra off");
    }

    #[test]
    fn done_elapsed_freezes_at_end_time() {
        let now = SystemTime::now();
        let mut s = session("proj-a", Status::Done);
        s.started_at = Some(now - Duration::from_secs(3 * 3600 + 10));
        s.ended_at = Some(now - Duration::from_secs(2 * 3600));
        let badge = |s: &Session, at: SystemTime| -> String {
            text_lines(s, &Config::default(), 40, at)[1].spans.iter().map(|sp| sp.content.as_ref()).collect()
        };
        assert_eq!(badge(&s, now), " DONE  1h");
        assert_eq!(badge(&s, now + Duration::from_secs(5 * 3600)), " DONE  1h", "does not keep growing");
        s.status = Status::Running;
        assert_eq!(badge(&s, now), " RUN  3h");
    }

    #[test]
    fn guessed_branch_has_tilde() {
        let mut s = session("proj-a", Status::Running);
        let exact = card(&s, &Config::default(), false).join("\n");
        assert!(exact.contains("feat/login") && !exact.contains("~feat/login"), "{exact}");
        s.branch_guessed = true;
        assert!(card(&s, &Config::default(), false).join("\n").contains("~feat/login"));
    }

    #[test]
    fn wide_korean_name_is_truncated_inside_the_border() {
        let s = session("프로젝트-아주-아주-아주-아주-아주-긴-이름-입니다-정말로-정말", Status::Running);
        for w in CARD_W..CARD_W + 6 {
            let rows = rows(&draw(w, CARD_H, |f| render_card(f, f.area(), &s, &Config::default(), false, 0)));
            let row = &rows[1 + SCENE_ROWS as usize];
            assert!(row.contains("프로젝트") && row.contains('…'), "{row}");
            assert!(row.ends_with('│'), "border intact: {row:?}");
        }
    }

    #[test]
    fn selected_card_border_is_cmux_blue_and_unselected_borders_are_not() {
        assert_eq!(style::SELECTED, Color::Rgb(0x4C, 0x8D, 0xFF), "cmux blue");
        for st in [Status::Running, Status::NeedsInput, Status::Idle, Status::Done, Status::Unknown] {
            let s = session("proj-a", st);
            let buf = draw(CARD_W, CARD_H, |f| render_card(f, f.area(), &s, &Config::default(), true, 0));
            let corner = &buf[(0, 0)];
            assert_eq!((corner.symbol(), corner.fg), ("╭", style::SELECTED), "{st:?}");
            assert!(corner.modifier.contains(Modifier::BOLD), "{st:?}");
            assert_eq!(buf[(CARD_W - 1, 5)].fg, style::SELECTED, "{st:?}: right border");
            let buf = draw(CARD_W, CARD_H, |f| render_card(f, f.area(), &s, &Config::default(), false, 0));
            assert_ne!(buf[(0, 0)].fg, style::SELECTED, "{st:?}: unselected border differs");
            assert!(!buf[(0, 0)].modifier.contains(Modifier::BOLD), "{st:?}");
            let Color::Rgb(r, g, b) = style::status_color(st) else { panic!("rgb") };
            assert!(!(b > r && b > g), "{st:?} border is blue-ish");
        }
        let s = session("proj-a", Status::Running);
        let buf = draw(CARD_W, CARD_H, |f| render_card(f, f.area(), &s, &Config::default(), false, 0));
        assert_eq!(buf[(0, 0)].fg, style::RUNNING);
    }

    #[test]
    fn done_card_is_dimmed() {
        let s = session("proj-a", Status::Done);
        let buf = draw(CARD_W, CARD_H, |f| render_card(f, f.area(), &s, &Config::default(), false, 0));
        assert!(buf[(0, 0)].modifier.contains(Modifier::DIM));
        let y = 1 + SCENE_ROWS;
        assert!(buf[(2, y)].modifier.contains(Modifier::DIM), "name row");
    }

    #[test]
    fn narrow_terminal_uses_compact_card_without_panic() {
        let s = session("proj-a", Status::NeedsInput);
        let lines = rows(&draw(30, 8, |f| render_card(f, f.area(), &s, &Config::default(), true, 0)));
        let text = lines.join("\n");
        assert!(lines[0].contains("proj-a") && lines[0].contains("INPUT"), "{text}");
        assert!(!text.contains('╭') && !text.chars().any(is_braille), "no frame, no scene: {text}");
        assert!(lines[1..].iter().all(|r| r.trim().is_empty()), "one line only: {text}");
        // one column or one row short of a card: compact as well
        for (w, h) in [(CARD_W - 1, CARD_H), (CARD_W, CARD_H - 1)] {
            let rows = rows(&draw(w, h, |f| render_card(f, f.area(), &s, &Config::default(), false, 0)));
            assert!(rows[0].contains("proj-a") && !rows.join("").contains('╭'), "{w}x{h}");
        }
    }

    #[test]
    fn compact_card_layout_and_truncation() {
        let s = session("proj-a", Status::Running);
        let line = |w: u16, selected: bool, cfg: &Config| -> String {
            rows(&draw(w, 1, |f| render_compact(f, f.area(), &s, cfg, selected)))[0].trim_end().to_string()
        };
        assert_eq!(line(30, true, &Config::default()), "▶ proj-a  RUN  3h");
        assert_eq!(line(30, false, &Config::default()), "  proj-a  RUN  3h");
        assert_eq!(line(30, false, &no_extras()), "  proj-a  RUN");
        assert_eq!(line(14, false, &Config::default()), "  proj-a  RUN", "elapsed dropped first");
        assert_eq!(line(9, false, &Config::default()), "  proj-a", "then the badge");
        assert_eq!(line(6, false, &Config::default()), "  pro…");
        assert_eq!(line(2, false, &Config::default()), "p…");
        assert_eq!(line(1, false, &Config::default()), "…");
        let buf = draw(30, 1, |f| render_compact(f, f.area(), &s, &Config::default(), true));
        assert_eq!(buf[(2, 0)].fg, style::SELECTED, "selected name is blue");
        let buf = draw(30, 1, |f| render_compact(f, f.area(), &s, &Config::default(), false));
        assert_eq!(buf[(2, 0)].fg, style::RUNNING);
    }

    #[test]
    fn compact_mode_rules() {
        let full = Rect::new(0, 0, CARD_W, CARD_H);
        assert!(!compact_mode(false, full));
        assert!(compact_mode(true, full), "preview");
        assert!(compact_mode(false, Rect { width: CARD_W - 1, ..full }));
        assert!(compact_mode(false, Rect { height: CARD_H - 1, ..full }));
        assert!(compact_mode(false, Rect::default()));
    }

    #[test]
    fn zero_and_tiny_areas_do_not_panic() {
        let s = session("프로젝트-아주-긴-이름", Status::Idle);
        for (w, h) in [(0, 0), (1, 1), (2, 1), (5, 2), (0, 30), (80, 0), (CARD_W - 1, CARD_H), (CARD_W, CARD_H - 1)] {
            for selected in [false, true] {
                draw(w, h, |f| render_card(f, f.area(), &s, &Config::default(), selected, 0));
                draw(w, h, |f| render_compact(f, f.area(), &s, &Config::default(), selected));
                // rect partly outside the frame
                draw(w, h, |f| render_card(f, Rect::new(w.saturating_sub(1), h.saturating_sub(1), CARD_W, CARD_H), &s, &Config::default(), selected, 0));
                draw(w, h, |f| render_card(f, Rect::new(u16::MAX - 5, u16::MAX - 5, CARD_W, CARD_H), &s, &Config::default(), selected, 0));
            }
        }
    }

    /// Plain-text look of a full card (with its spirit) and a narrow compact card.
    /// Run with `cargo test dump_card -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn dump_card() {
        let s = session("proj-a", Status::Running);
        for r in rows(&draw(CARD_W, CARD_H, |f| render_card(f, f.area(), &s, &all_extras(), true, 0))) {
            println!("{r}");
        }
        for r in rows(&draw(30, 1, |f| render_compact(f, f.area(), &s, &Config::default(), true))) {
            println!("{r}");
        }
    }
}
