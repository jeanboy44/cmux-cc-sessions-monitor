//! Session cards: an element spirit (braille, see [`crate::spirit`]) on top
//! and info lines below, in a thin rounded border.
//!
//! Full card: always [`CARD_W`] x [`CARD_H`] = 20 x 9 cells (borders
//! included; inner 18 x 7; user decision: narrowed from 24 because it looked wide), whatever the spirit size: a spirit band of
//! [`SCENE_ROWS`] rows (every spirit size is that tall; the spirit is centered
//! across) and [`INFO_ROWS`] text rows: name, status badge (+ elapsed / last
//! activity), then one extra: the PR chip, else the branch, else the summary
//! (rows of disabled or unknown extras collapse). A larger area gets the card
//! centered across, at its top.
//!
//! When a card does not fit (or the preview is on) a compact one-line card is
//! drawn instead ([`render_compact`]). Empty areas draw nothing; no size panics.

use std::time::SystemTime;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
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
/// Text rows below the spirit band.
pub const INFO_ROWS: u16 = 3;
/// Inner width of a card.
const INNER_W: u16 = 18;
/// Full card size, borders included: 20 x 9.
pub const CARD_W: u16 = INNER_W + 2;
pub const CARD_H: u16 = SCENE_ROWS + INFO_ROWS + 2;
const _: () = assert!(SCENE_COLS <= INNER_W, "the largest spirit fits the card");

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

#[cfg(test)]
thread_local! {
    /// Spirit renders on this thread (tests check that only visible cards render one).
    static SPIRIT_RENDERS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Spirit renders on the current thread since the last reset.
#[cfg(test)]
pub fn spirit_renders() -> usize {
    SPIRIT_RENDERS.with(std::cell::Cell::get)
}

#[cfg(test)]
pub fn reset_spirit_renders() {
    SPIRIT_RENDERS.with(|c| c.set(0));
}

/// Renders the spirit for `params` centered across the top of `band` and
/// returns the rect it was drawn into (clipped to the band).
fn draw_spirit(buf: &mut Buffer, band: Rect, params: &SpiritParams) -> Rect {
    #[cfg(test)]
    SPIRIT_RENDERS.with(|c| c.set(c.get() + 1));
    let grid = spirit::render(params);
    // Centre the BODY (not the grid) in the band; the particle side of the
    // grid is clipped at the band's right edge.
    let want = band.x as f32 + band.width as f32 / 2.0 - spirit::body_center_x(params.size);
    let x = (want.round().max(band.x as f32) as u16).min(band.right().saturating_sub(1)).max(band.x);
    let (cols, rows) = (grid.cols.min(band.right().saturating_sub(x)), grid.rows.min(band.height));
    let at = Rect::new(x, band.y, cols, rows);
    draw_grid(buf, at, &grid);
    at
}

/// Draws one session card with its spirit at animation frame `tick`: a
/// [`CARD_W`] x [`CARD_H`] card centered across the top of `area`. Too small
/// an area falls back to [`render_compact`].
pub fn render_card(f: &mut Frame, area: Rect, s: &Session, cfg: &Config, selected: bool, tick: u64) {
    let area = area.intersection(f.area());
    if area.is_empty() {
        return;
    }
    if area.width < CARD_W || area.height < CARD_H {
        render_compact(f, area, s, cfg, selected);
        return;
    }
    let rect = Rect::new(area.x + (area.width - CARD_W) / 2, area.y, CARD_W, CARD_H);
    let done = s.status == Status::Done;
    let mut block = style::window_block(style::status_color(s.status), selected);
    if done && !selected {
        block = block.border_style(Style::new().fg(style::DONE).add_modifier(Modifier::DIM));
    }
    let inner = block.inner(rect);
    f.render_widget(Clear, rect);
    f.render_widget(block, rect);

    let band = Rect { height: SCENE_ROWS.min(inner.height), ..inner };
    let params = SpiritParams { theme: theme_for(s, cfg), status: s.status, size: Size::S, tick, seed: seed_for(s), warning: false };
    draw_spirit(f.buffer_mut(), band, &params);

    let text_area = Rect::new(inner.x, inner.y + band.height, inner.width, inner.height - band.height);
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
/// PR chip, branch (`~` when guessed), `PR ?` (lookup failed), one-line
/// summary. A full card keeps three rows, so the third is the PR when there
/// is one, else the branch, else the summary.
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

    if let Some(pr) = s.pr.as_ref().filter(|_| ex.pr) {
        let pr_color = match pr.state {
            PrState::Approved => style::DONE,
            PrState::Conflict => style::FLASH_ALERT,
            _ => style::TEXT_DIM,
        };
        lines.push(Line::styled(truncate_to_width(&format!("#{} {}", pr.number, pr_label(pr.state)), width), Style::new().fg(pr_color)));
    }
    if let Some(b) = s.branch.as_ref().filter(|_| ex.branch) {
        let tilde = if s.branch_guessed { "~" } else { "" };
        lines.push(Line::styled(truncate_to_width(&format!("⎇ {tilde}{b}"), width), text));
    }
    if ex.pr && s.pr.is_none() && s.pr_unavailable {
        // The lookup failed; the footer says why.
        lines.push(Line::styled(truncate_to_width("PR ?", width), dim));
    }
    if let Some(sum) = s.summary.as_ref().filter(|_| ex.summary) {
        lines.push(Line::styled(truncate_to_width(sum, width), dim));
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

    /// Text of an info row: border stripped.
    fn info(row: &str) -> String {
        row.chars().skip(1).collect::<String>().trim_end().trim_end_matches('│').trim_end().to_string()
    }

    #[test]
    fn card_is_exactly_20x9() {
        assert_eq!((CARD_W, CARD_H), (20, 9), "user decision: narrowed from 24");
        assert_eq!((SCENE_ROWS, INFO_ROWS), (4, 3), "inner 18x7: spirit band + 3 text rows");
        assert_eq!((SCENE_COLS, SCENE_ROWS), max_grid_size());
        for size in Size::ALL {
            let (w, h) = spirit::grid_size(size);
            assert!(w <= CARD_W - 2 && h == SCENE_ROWS, "{size:?} fits the band: {w}x{h}");
        }
        let s = session("proj-a", Status::Running);
        let rows = card(&s, &all_extras(), true);
        assert_eq!(rows.len(), 9);
        assert!(rows[0].starts_with('╭') && rows[0].ends_with('╮'), "{rows:?}");
        assert!(rows[8].starts_with('╰') && rows[8].ends_with('╯'), "{rows:?}");
        assert!(rows[1..8].iter().all(|r| r.starts_with('│') && r.ends_with('│')), "{rows:?}");
        // a larger area still gets a 20x9 card (centered across, at the top); nothing else drawn
        let (w, h) = (40u16, 15u16);
        let buf = draw(w, h, |f| render_card(f, f.area(), &s, &all_extras(), false, 0));
        let x0 = (w - CARD_W) / 2;
        assert_eq!(buf[(x0, 0)].symbol(), "╭");
        assert_eq!(buf[(x0 + CARD_W - 1, CARD_H - 1)].symbol(), "╯");
        for y in 0..h {
            for x in 0..w {
                let inside = (x0..x0 + CARD_W).contains(&x) && y < CARD_H;
                assert!(inside || buf[(x, y)].symbol() == " ", "({x},{y}) drawn outside the card");
            }
        }
    }

    #[test]
    fn card_contains_spirit_and_three_info_lines() {
        let s = session("proj-a", Status::Running);
        let rows = rows(&draw(80, 24, |f| render_card(f, Rect::new(0, 0, CARD_W, CARD_H), &s, &Config::default(), false, 0)));
        let text = rows.join("\n");
        let last = CARD_H as usize - 1;
        assert!(rows[0].starts_with('╭') && rows[last].starts_with('╰'), "thin rounded border: {text}");
        let scene_rows: Vec<usize> = (1..last).filter(|&y| rows[y].chars().any(is_braille)).collect();
        assert!(!scene_rows.is_empty() && scene_rows.iter().all(|&y| y <= SCENE_ROWS as usize), "spirit in the 4-row band: {text}");
        let info_rows: Vec<String> = (1 + SCENE_ROWS as usize..last).map(|y| info(&rows[y])).collect();
        assert_eq!(info_rows.len(), 3);
        assert_eq!(info_rows[0], "proj-a", "{text}");
        assert!(info_rows[1].contains("RUN") && info_rows[1].contains("3h"), "{text}");
        assert_eq!(info_rows[2], "⎇ feat/login", "{text}");
        assert!(!text.contains("fixing the parser") && !text.contains("#42"), "summary dropped, PR off: {text}");
        // nothing outside the card rect
        assert!(rows[last + 1..].iter().all(|r| r.trim().is_empty()));
        assert!(rows[..=last].iter().all(|r| r.chars().skip(CARD_W as usize).all(|c| c == ' ')));
    }

    #[test]
    fn spirit_is_centered_and_paints_no_background() {
        let s = session("proj-a", Status::Idle);
        let buf = draw(CARD_W, CARD_H, |f| render_card(f, f.area(), &s, &Config::default(), false, 0));
        let w = CARD_W;
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
    fn every_spirit_size_is_centered_in_the_band() {
        let band = Rect::new(1, 1, CARD_W - 2, SCENE_ROWS);
        for size in Size::ALL {
            let params = SpiritParams { theme: Theme::Water, status: Status::Running, size, tick: 0, seed: 1, warning: false };
            let mut buf = ratatui::buffer::Buffer::empty(Rect::new(0, 0, CARD_W, CARD_H));
            let at = draw_spirit(&mut buf, band, &params);
            let (w, h) = spirit::grid_size(size);
            assert!(at.width <= w && at.height == h, "{size:?}");
            assert!(at.x >= band.x && at.right() <= band.right(), "{size:?}: grid inside the band");
            for y in 0..CARD_H {
                for x in 0..CARD_W {
                    if !at.contains((x, y).into()) {
                        assert_eq!(buf[(x, y)].symbol(), " ", "{size:?}: ({x},{y}) outside the spirit rect");
                    }
                }
            }
        }
    }

    #[test]
    fn spirit_body_is_centered_in_the_inner_area() {
        let band = Rect::new(1, 1, CARD_W - 2, SCENE_ROWS);
        let center = band.x as f32 + band.width as f32 / 2.0;
        for theme in [Theme::Fire, Theme::Water, Theme::Ice, Theme::Lightning] {
            for status in [Status::Idle, Status::Running] {
                for size in Size::ALL {
                    let params = SpiritParams { theme, status, size, tick: 3, seed: 1, warning: false };
                    let mut buf = Buffer::empty(Rect::new(0, 0, CARD_W, CARD_H));
                    draw_spirit(&mut buf, band, &params);
                    let lo = (0..CARD_W)
                        .find(|&x| (0..CARD_H).any(|y| buf[(x, y)].symbol().chars().any(is_braille)))
                        .expect("painted");
                    // body radius in sub-pixels: 5.5 / 6 / 6.5; the left edge cell holds the body's left edge
                    let r = match size {
                        Size::S => 5.5,
                        Size::M => 6.0,
                        Size::L => 6.5,
                    };
                    let body_c = lo as f32 + 0.5 + r / 2.0;
                    assert!((body_c - center).abs() <= 1.0, "{theme:?} {status:?} {size:?}: body centre {body_c} vs {center}");
                }
            }
        }
    }

    #[test]
    fn status_changes_spirit_cues() {
        let band = |st: Status, tick: u64| -> Vec<String> {
            let s = session("proj-a", st);
            rows(&draw(CARD_W, CARD_H, |f| render_card(f, f.area(), &s, &Config::default(), false, tick)))[1..=SCENE_ROWS as usize].to_vec()
        };
        for tick in [0, 7, 30] {
            assert_ne!(band(Status::Running, tick), band(Status::Idle, tick), "tick {tick}");
        }
        assert!((0..30).all(|t| band(Status::Idle, t).concat().contains(['z', 'Z'])), "idle z");
        assert!((0..30).all(|t| band(Status::NeedsInput, t).concat().contains('?')), "input ?");
        assert!(!band(Status::Running, 5).concat().contains(['?', 'z', 'Z']));
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
    fn info_rows_keep_pr_over_branch_over_summary_and_hide_disabled_extras() {
        let mut s = session("proj-a", Status::Running);
        let three = |s: &Session, cfg: &Config| -> Vec<String> {
            let first = 1 + SCENE_ROWS as usize;
            card(s, cfg, false)[first..first + 3].iter().map(|r| info(r)).collect()
        };
        assert_eq!(three(&s, &all_extras()), vec!["proj-a", " RUN  3h ·2m ago", "#42 approved"], "PR first");
        let mut cfg = all_extras();
        cfg.extras.pr = false;
        assert_eq!(three(&s, &cfg)[2], "⎇ feat/login", "then the branch");
        s.branch = None;
        assert_eq!(three(&s, &cfg)[2], "fixing the parser", "summary only when neither");
        s.branch = Some("feat/login".into());
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
        let with_branch = card(&s, &all_extras(), false).join("\n");
        assert!(with_branch.contains("feat/login") && !with_branch.contains("PR ?"), "a known branch beats an unknown PR: {with_branch}");
        s.branch = None;
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
            let row = rows[1 + SCENE_ROWS as usize].trim();
            assert!(row.contains("프로젝트") && row.contains('…'), "{row}");
            assert!(row.starts_with('│') && row.ends_with('│'), "border intact: {row:?}");
            assert_eq!(row.width(), usize::from(CARD_W), "{row:?}");
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
