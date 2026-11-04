//! Session cards: an element spirit (braille, see [`crate::spirit`]) on top
//! and info lines below, in a thin rounded border.
//!
//! Full card: always [`CARD_W`] x [`CARD_H`] = 20 x 9 cells (borders
//! included; inner 18 x 7; user decision: narrowed from 24 because it looked wide), whatever the spirit size: a spirit band of
//! [`SCENE_ROWS`] rows (every spirit size is that tall; the body sits on the
//! left and the magic it casts, scaled by context usage, fills the right) and [`INFO_ROWS`] text rows: name, status badge (+ elapsed / last
//! activity), then one extra: the PR chip, else the branch, else the summary
//! (rows of disabled or unknown extras collapse). A larger area gets the card
//! centered across, at its top.
//!
//! The preview list stretches the card across its column instead
//! ([`render_wide_card`]): same height, the body stays on the left and the
//! extra columns go to the text rows and the magic's room on the right.
//!
//! When a card does not fit a compact one-line card is drawn instead
//! ([`render_compact`]). Empty areas draw nothing; no size panics.

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
use crate::spirit::{self, max_grid_size, Magic, Size, SpiritParams};
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

/// Whether the grid draws compact one-line cards: only when `area` cannot
/// hold one full card (the preview list keeps full cards).
pub fn compact_mode(area: Rect) -> bool {
    area.width < CARD_W || area.height < CARD_H
}

/// The card's element: the one mapped to the session's model family, or the
/// configured default when the model is unknown (see [`Config::theme_for_model`]).
fn theme_for(s: &Session, cfg: &Config) -> Theme {
    cfg.theme_for_model(s.model.as_deref())
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

/// Free columns between the card border and the spirit grid.
const SPIRIT_LEFT_PAD: u16 = 1;
const _: () = assert!(
    spirit::grid_size_for(Size::S, Magic::Big).0 + SPIRIT_LEFT_PAD <= INNER_W,
    "big magic fits right of the padded body"
);

/// Renders the spirit for `params` at the top left of `band` (after
/// [`SPIRIT_LEFT_PAD`]) and returns the rect it was drawn into (clipped to
/// the band). User decision: the body sits on the left so the cast magic,
/// which shows the context usage, has the rest of the card to the right
/// (this replaced the centred body of the first spirit cards).
fn draw_spirit(buf: &mut Buffer, band: Rect, params: &SpiritParams) -> Rect {
    #[cfg(test)]
    SPIRIT_RENDERS.with(|c| c.set(c.get() + 1));
    let grid = spirit::render(params);
    let pad = if band.width > SPIRIT_LEFT_PAD { SPIRIT_LEFT_PAD } else { 0 };
    let x = band.x + pad;
    let (cols, rows) = (grid.cols.min(band.right().saturating_sub(x)), grid.rows.min(band.height));
    let at = Rect::new(x, band.y, cols, rows);
    draw_grid(buf, at, &grid);
    at
}

/// Draws one session card with its spirit at animation frame `tick`: a
/// [`CARD_W`] x [`CARD_H`] card centered across the top of `area`. Too small
/// an area falls back to [`render_compact`].
pub fn render_card(f: &mut Frame, area: Rect, s: &Session, cfg: &Config, selected: bool, tick: u64) {
    render_full(f, area, s, cfg, selected, tick, false);
}

/// Like [`render_card`], but the card is as wide as `area` (at least
/// [`CARD_W`]), at its top left: the preview's one-column list.
pub fn render_wide_card(f: &mut Frame, area: Rect, s: &Session, cfg: &Config, selected: bool, tick: u64) {
    render_full(f, area, s, cfg, selected, tick, true);
}

fn render_full(f: &mut Frame, area: Rect, s: &Session, cfg: &Config, selected: bool, tick: u64, stretch: bool) {
    let area = area.intersection(f.area());
    if area.is_empty() {
        return;
    }
    if compact_mode(area) {
        render_compact(f, area, s, cfg, selected, tick);
        return;
    }
    let rect = if stretch {
        Rect { height: CARD_H, ..area }
    } else {
        Rect::new(area.x + (area.width - CARD_W) / 2, area.y, CARD_W, CARD_H)
    };
    let done = s.status == Status::Done;
    let mut block = style::window_block(style::status_color(s.status), selected);
    if done && !selected {
        block = block.border_style(Style::new().fg(style::DONE).add_modifier(Modifier::DIM));
    }
    // Unread cmux notification: on the lit half of the blink the frame turns
    // heavy (and, unless selected, bold notify blue); the other half is the
    // normal frame. A selected card keeps its selection blue throughout, so
    // selected + unread shows both cues at once.
    if s.unread_notification && style::blink_on(tick) {
        block = block.border_set(style::UNREAD_BORDER);
        if !selected {
            block = block.border_style(Style::new().fg(style::NOTIFY_BLUE).add_modifier(Modifier::BOLD));
        }
    }
    let inner = block.inner(rect);
    f.render_widget(Clear, rect);
    f.render_widget(block, rect);

    let band = Rect { height: SCENE_ROWS.min(inner.height), ..inner };
    let params = SpiritParams {
        theme: theme_for(s, cfg),
        status: s.status,
        size: Size::S,
        tick,
        seed: seed_for(s),
        warning: spirit::warning(s.ctx_percent),
        magic: spirit::magic_for(s.ctx_percent),
        stage: spirit::stage_for(s.ctx_percent),
        cols: band.width.saturating_sub(SPIRIT_LEFT_PAD),
    };
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

/// One-line card: `▶ name  STATUS  3h` (cursor only when selected; an
/// unread notification puts a `●` right after the cursor that blinks notify
/// blue / dim at `tick`). The name is kept first; elapsed, then the badge,
/// are dropped when too narrow.
pub fn render_compact(f: &mut Frame, area: Rect, s: &Session, cfg: &Config, selected: bool, tick: u64) {
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
        spans.push(Span::styled(mark, Style::new().fg(style::SELECTED).add_modifier(Modifier::BOLD)));
        if s.unread_notification {
            let fg = if style::blink_on(tick) { style::NOTIFY_BLUE } else { style::TEXT_DIM };
            spans.push(Span::styled("●", Style::new().fg(fg).add_modifier(Modifier::BOLD)));
        } else {
            spans.push(Span::raw(" "));
        }
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
            ctx_percent: None,
            model: None,
            unread_notification: false,
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
        assert!(lo <= 3, "body on the left (user decision): {lo}..{hi}");
        for y in 1..=SCENE_ROWS {
            for x in 1..w - 1 {
                assert_eq!(buf[(x, y)].bg, ratatui::style::Color::Reset, "({x},{y})");
            }
        }
    }

    #[test]
    fn every_spirit_grid_sits_left_inside_the_band() {
        // user decision: the body sits on the left so the magic has room on the right
        let band = Rect::new(1, 1, CARD_W - 2, SCENE_ROWS);
        for size in Size::ALL {
            for magic in Magic::ALL {
                for tick in [0, 30, 61] {
                    let params = SpiritParams { theme: Theme::Water, status: Status::Running, size, tick, seed: 1, warning: false, magic, stage: Default::default(), cols: 0 };
                    let mut buf = ratatui::buffer::Buffer::empty(Rect::new(0, 0, CARD_W, CARD_H));
                    let at = draw_spirit(&mut buf, band, &params);
                    let (w, h) = spirit::grid_size_for(size, magic);
                    assert!(at.width <= w && at.height == h, "{size:?} {magic:?}");
                    assert!(at.x >= band.x && at.right() <= band.right(), "{size:?} {magic:?}: grid inside the band (clipped)");
                    assert!(at.x <= band.x + SPIRIT_LEFT_PAD, "{size:?} {magic:?}: grid at the left");
                    for y in 0..CARD_H {
                        for x in 0..CARD_W {
                            if !at.contains((x, y).into()) {
                                assert_eq!(buf[(x, y)].symbol(), " ", "{size:?} {magic:?}: ({x},{y}) outside the spirit rect");
                            }
                        }
                    }
                }
            }
        }
        // a band too narrow for the pad still draws inside it
        let narrow = Rect::new(1, 1, 5, SCENE_ROWS);
        let params = SpiritParams { theme: Theme::Fire, status: Status::Running, size: Size::S, tick: 30, seed: 1, warning: false, magic: Magic::Big, stage: Default::default(), cols: 0 };
        let mut buf = Buffer::empty(Rect::new(0, 0, CARD_W, CARD_H));
        let at = draw_spirit(&mut buf, narrow, &params);
        assert!(at.x >= narrow.x && at.right() <= narrow.right());
    }

    #[test]
    fn spirit_body_sits_left_in_the_inner_area() {
        let band = Rect::new(1, 1, CARD_W - 2, SCENE_ROWS);
        for theme in Theme::ALL {
            for status in [Status::Idle, Status::Running] {
                for magic in Magic::ALL {
                    let params = SpiritParams { theme, status, size: Size::S, tick: 3, seed: 1, warning: false, magic, stage: Default::default(), cols: 0 };
                    let mut buf = Buffer::empty(Rect::new(0, 0, CARD_W, CARD_H));
                    draw_spirit(&mut buf, band, &params);
                    let lo = (0..CARD_W)
                        .find(|&x| (0..CARD_H).any(|y| buf[(x, y)].symbol().chars().any(is_braille)))
                        .expect("painted");
                    // about one free column between the border and the body
                    assert!((band.x + 1..=band.x + 2).contains(&lo), "{theme:?} {status:?} {magic:?}: body left edge at {lo}");
                    let body_c = band.x as f32 + SPIRIT_LEFT_PAD as f32 + spirit::body_center_x(Size::S);
                    assert!(body_c < band.x as f32 + band.width as f32 / 2.0 - 3.0, "body left of centre: {body_c}");
                }
            }
        }
    }

    /// Columns of the spirit band that hold a braille glyph at `tick`.
    fn lit_cols(s: &Session, tick: u64) -> std::collections::BTreeSet<u16> {
        let buf = draw(CARD_W, CARD_H, |f| render_card(f, f.area(), s, &Config::default(), false, tick));
        (1..=SCENE_ROWS)
            .flat_map(|y| (1..CARD_W - 1).map(move |x| (x, y)))
            .filter(|&p| buf[p].symbol().chars().any(is_braille))
            .map(|(x, _)| x)
            .collect()
    }

    #[test]
    fn context_usage_scales_the_magic_not_the_body() {
        let with = |st: Status, pct: Option<f32>| -> Session {
            let mut s = session("proj-a", st);
            s.ctx_percent = pct;
            s
        };
        // Idle casts nothing: the spirit covers the same columns at every ctx level
        // (the ctx stage changes how it moves, not where it sits)
        let idle: Vec<_> = [None, Some(10.0), Some(50.0), Some(80.0)].into_iter().map(|p| lit_cols(&with(Status::Idle, p), 0)).collect();
        assert!(idle.windows(2).all(|w| w[0] == w[1]), "body unchanged: {idle:?}");
        // Running: the magic reaches farther as ctx grows; unknown ctx = normal magic
        let reach = |pct: Option<f32>| -> u16 { (0..120).filter_map(|t| lit_cols(&with(Status::Running, pct), t).last().copied()).max().unwrap() };
        let (none, low, mid, high) = (reach(None), reach(Some(10.0)), reach(Some(50.0)), reach(Some(80.0)));
        assert_eq!(none, mid, "unknown ctx casts normal magic");
        assert!(low < mid && mid < high, "magic grows with ctx: {low} {mid} {high}");
        // every level stays inside the inner area
        for pct in [10.0, 50.0, 80.0, 95.0] {
            for t in 0..120 {
                let cols = lit_cols(&with(Status::Running, Some(pct)), t);
                assert!(cols.iter().all(|&x| (1..CARD_W - 1).contains(&x)), "{pct} {t}: {cols:?}");
            }
        }
    }

    #[test]
    fn context_over_90_flickers_red_between_ticks() {
        let fgs = |pct: f32, tick: u64| -> Vec<Color> {
            let mut s = session("proj-a", Status::Idle);
            s.ctx_percent = Some(pct);
            let buf = draw(CARD_W, CARD_H, |f| render_card(f, f.area(), &s, &Config::default(), false, tick));
            (1..=SCENE_ROWS).flat_map(|y| (1..CARD_W - 1).map(move |x| (x, y))).map(|p| buf[p].fg).collect()
        };
        let half = spirit::WARN_HALF_PERIOD;
        // same size step (L), ticks in the on and off phases of the flicker
        assert_ne!(fgs(95.0, 0), fgs(80.0, 0), "warning tints the on phase");
        assert_eq!(fgs(95.0, half), fgs(80.0, half), "off phase looks normal");
        assert_ne!(fgs(95.0, 0), fgs(95.0, half), "flicker differs between ticks");
    }

    #[test]
    fn odd_context_values_never_panic() {
        for pct in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -1.0, 0.0, 1e30, f32::MIN_POSITIVE] {
            let mut s = session("proj-a", Status::Running);
            s.ctx_percent = Some(pct);
            for (w, h) in [(CARD_W, CARD_H), (60, 20), (5, 3), (0, 0)] {
                let _ = draw(w.max(1), h.max(1), |f| render_card(f, Rect::new(0, 0, w, h), &s, &Config::default(), true, 3));
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
        water.theme = Theme::Water;
        assert_ne!(fg(&Config::default()), fg(&water), "unknown model: the default theme");
        assert!((1..30).any(|t| scene(&Config::default(), t) != scene(&Config::default(), 0)), "tick moves the spirit");
    }

    #[test]
    fn card_spirit_follows_the_session_model() {
        let with_model = |m: Option<&str>| {
            let mut s = session("proj-a", Status::Running);
            s.model = m.map(String::from);
            s
        };
        let fg = |s: &Session, cfg: &Config| -> Vec<Color> {
            let buf = draw(CARD_W, CARD_H, |f| render_card(f, f.area(), s, cfg, false, 3));
            (1..=SCENE_ROWS).flat_map(|y| (1..CARD_W - 1).map(move |x| (x, y))).map(|p| buf[p].fg).collect()
        };
        let cfg = Config::default();
        let (opus, haiku) = (with_model(Some("claude-opus-4-5")), with_model(Some("claude-haiku-5-5")));
        assert_eq!(theme_for(&opus, &cfg), Theme::Fire);
        assert_eq!(theme_for(&haiku, &cfg), Theme::Poison);
        assert_ne!(fg(&opus, &cfg), fg(&haiku, &cfg), "opus and haiku spirits differ");
        // unknown model: same spirit as the default theme (fire)
        assert_eq!(fg(&with_model(None), &cfg), fg(&opus, &cfg));
        // a custom mapping changes the spirit
        let mut custom = Config::default();
        custom.model_themes.insert("haiku".into(), Theme::Fire);
        assert_eq!(theme_for(&haiku, &custom), Theme::Fire);
        assert_eq!(fg(&haiku, &custom), fg(&opus, &cfg));
        // the model name never shows in the card text
        let text = rows(&draw(CARD_W, CARD_H, |f| render_card(f, f.area(), &opus, &cfg, false, 3))).join("\n");
        assert!(!text.contains("opus") && !text.contains("claude"), "{text}");
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

    /// First tick with the unread blink on, and first with it off.
    fn blink_ticks() -> (u64, u64) {
        ((0..).find(|&t| style::blink_on(t)).unwrap(), (0..).find(|&t| !style::blink_on(t)).unwrap())
    }

    #[test]
    fn unread_card_border_blinks_thick_blue_and_stops_when_read() {
        assert_ne!(style::NOTIFY_BLUE, style::SELECTED, "unread blue differs from the selection blue");
        let Color::Rgb(r, g, b) = style::NOTIFY_BLUE else { panic!("rgb") };
        assert!(b > r && b > g, "unread colour is blue");
        let (on, off) = blink_ticks();
        for st in [Status::Running, Status::NeedsInput, Status::Idle, Status::Done, Status::Unknown] {
            let plain = session("proj-a", st);
            let unread = Session { unread_notification: true, ..plain.clone() };
            let at = |s: &Session, t: u64| draw(CARD_W, CARD_H, |f| render_card(f, f.area(), s, &Config::default(), false, t));
            let lit = at(&unread, on);
            for (x, y, sym) in [(0, 0, "┏"), (CARD_W - 1, 5, "┃"), (5, CARD_H - 1, "━")] {
                let c = &lit[(x, y)];
                assert_eq!((c.symbol(), c.fg), (sym, style::NOTIFY_BLUE), "{st:?} on-phase at ({x},{y})");
                assert!(c.modifier.contains(Modifier::BOLD) && !c.modifier.contains(Modifier::DIM), "{st:?}");
            }
            // off phase: exactly the normal border (thin, status colour, Done dimmed)
            let dark = at(&unread, off);
            let normal = at(&plain, off);
            for (x, y) in [(0, 0), (CARD_W - 1, 5), (5, CARD_H - 1)] {
                let (d, n) = (&dark[(x, y)], &normal[(x, y)]);
                assert_eq!((d.symbol(), d.fg, d.modifier), (n.symbol(), n.fg, n.modifier), "{st:?} off-phase");
            }
            assert_eq!(normal[(0, 0)].symbol(), "╭");
            // marked read: no blink at all
            assert_eq!(at(&plain, on)[(0, 0)], normal[(0, 0)], "{st:?}: a read card never blinks");
        }
    }

    #[test]
    fn selected_unread_card_shows_both_cues() {
        let (on, off) = blink_ticks();
        for st in [Status::Running, Status::Idle, Status::Done] {
            let s = Session { unread_notification: true, ..session("proj-a", st) };
            let at = |t: u64| draw(CARD_W, CARD_H, |f| render_card(f, f.area(), &s, &Config::default(), true, t));
            // selection: always bold cmux blue; unread: the frame turns thick on the beat
            let (lit, dark) = (at(on), at(off));
            assert_eq!((lit[(0, 0)].symbol(), lit[(0, 0)].fg), ("┏", style::SELECTED), "{st:?}");
            assert_eq!((dark[(0, 0)].symbol(), dark[(0, 0)].fg), ("╭", style::SELECTED), "{st:?}");
            assert!(lit[(0, 0)].modifier.contains(Modifier::BOLD) && dark[(0, 0)].modifier.contains(Modifier::BOLD));
            // a selected card without notification never turns thick
            let read = Session { unread_notification: false, ..s.clone() };
            let buf = draw(CARD_W, CARD_H, |f| render_card(f, f.area(), &read, &Config::default(), true, on));
            assert_eq!(buf[(0, 0)].symbol(), "╭");
        }
    }

    #[test]
    fn compact_card_shows_unread_dot() {
        let (on, off) = blink_ticks();
        let s = Session { unread_notification: true, ..session("proj-a", Status::Done) };
        let line = |s: &Session, selected: bool, t: u64| draw(30, 1, |f| render_compact(f, f.area(), s, &Config::default(), selected, t));
        let buf = line(&s, false, on);
        assert_eq!((buf[(1, 0)].symbol(), buf[(1, 0)].fg), ("●", style::NOTIFY_BLUE));
        assert_eq!(rows(&buf)[0].trim_end(), " ●proj-a  DONE  3h");
        let buf = line(&s, false, off);
        assert_eq!((buf[(1, 0)].symbol(), buf[(1, 0)].fg), ("●", style::TEXT_DIM));
        // selected + unread: cursor and dot
        let buf = line(&s, true, on);
        assert_eq!(rows(&buf)[0].trim_end(), "▶●proj-a  DONE  3h");
        // read: back to the plain layout
        let read = Session { unread_notification: false, ..s.clone() };
        assert_eq!(rows(&line(&read, false, on))[0].trim_end(), "  proj-a  DONE  3h");
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
            rows(&draw(w, 1, |f| render_compact(f, f.area(), &s, cfg, selected, 0)))[0].trim_end().to_string()
        };
        assert_eq!(line(30, true, &Config::default()), "▶ proj-a  RUN  3h");
        assert_eq!(line(30, false, &Config::default()), "  proj-a  RUN  3h");
        assert_eq!(line(30, false, &no_extras()), "  proj-a  RUN");
        assert_eq!(line(14, false, &Config::default()), "  proj-a  RUN", "elapsed dropped first");
        assert_eq!(line(9, false, &Config::default()), "  proj-a", "then the badge");
        assert_eq!(line(6, false, &Config::default()), "  pro…");
        assert_eq!(line(2, false, &Config::default()), "p…");
        assert_eq!(line(1, false, &Config::default()), "…");
        let buf = draw(30, 1, |f| render_compact(f, f.area(), &s, &Config::default(), true, 0));
        assert_eq!(buf[(2, 0)].fg, style::SELECTED, "selected name is blue");
        let buf = draw(30, 1, |f| render_compact(f, f.area(), &s, &Config::default(), false, 0));
        assert_eq!(buf[(2, 0)].fg, style::RUNNING);
    }

    #[test]
    fn compact_mode_rules() {
        let full = Rect::new(0, 0, CARD_W, CARD_H);
        assert!(!compact_mode(full));
        assert!(!compact_mode(Rect::new(0, 0, 500, 500)), "the preview list keeps full cards");
        assert!(compact_mode(Rect { width: CARD_W - 1, ..full }));
        assert!(compact_mode(Rect { height: CARD_H - 1, ..full }));
        assert!(compact_mode(Rect::default()));
    }

    fn wide(s: &Session, cfg: &Config, w: u16, selected: bool, tick: u64) -> Buffer {
        draw(w, CARD_H + 2, |f| render_wide_card(f, f.area(), s, cfg, selected, tick))
    }

    #[test]
    fn wide_card_stretches_to_the_area_width_body_left_text_wider() {
        let s = session("proj-a", Status::Running);
        for w in [CARD_W, CARD_W + 1, 26, 30, 34] {
            let buf = wide(&s, &all_extras(), w, false, 0);
            let rows = rows(&buf);
            let last = usize::from(CARD_H) - 1;
            assert!(rows[0].starts_with('╭') && rows[0].ends_with('╮'), "w={w}: {rows:?}");
            assert!(rows[last].starts_with('╰') && rows[last].ends_with('╯'), "w={w}: {rows:?}");
            assert!(rows[1..last].iter().all(|r| r.starts_with('│') && r.ends_with('│')), "w={w}: {rows:?}");
            assert_eq!(rows[0].width(), usize::from(w), "card width == area width");
            assert!(rows[last + 1..].iter().all(|r| r.trim().is_empty()), "exactly CARD_H rows tall");
            // spirit in the band, body on the left
            let lit: Vec<u16> = (1..=SCENE_ROWS)
                .flat_map(|y| (1..w - 1).map(move |x| (x, y)))
                .filter(|&p| buf[p].symbol().chars().any(is_braille))
                .map(|(x, _)| x)
                .collect();
            assert!(!lit.is_empty() && *lit.iter().min().unwrap() <= 3, "w={w}: {lit:?}");
            let info_rows: Vec<String> = (1 + SCENE_ROWS as usize..last).map(|y| info(&rows[y])).collect();
            assert_eq!(info_rows, vec!["proj-a", " RUN  3h ·2m ago", "#42 approved"], "w={w}");
        }
        // text truncation follows the wider inner width
        let long = session("proj-abcdefghijklmnopqrstu", Status::Running); // 25 columns
        let name_row = |w: u16| info(&rows(&wide(&long, &Config::default(), w, false, 0))[1 + SCENE_ROWS as usize]);
        assert!(name_row(CARD_W).ends_with('…'), "{}", name_row(CARD_W));
        assert_eq!(name_row(30), "proj-abcdefghijklmnopqrstu", "inner 28 holds the whole name");
        assert!(name_row(26).ends_with('…') && name_row(26).width() == 24, "{}", name_row(26));
    }

    #[test]
    fn wide_card_keeps_selection_unread_blink_and_done_dim() {
        let (on, off) = blink_ticks();
        let w = 30;
        let s = session("proj-a", Status::Running);
        let buf = wide(&s, &Config::default(), w, true, 0);
        assert_eq!((buf[(0, 0)].symbol(), buf[(0, 0)].fg), ("╭", style::SELECTED));
        assert_eq!(buf[(w - 1, 5)].fg, style::SELECTED, "right border at the stretched edge");
        let unread = Session { unread_notification: true, ..s.clone() };
        let lit = wide(&unread, &Config::default(), w, false, on);
        assert_eq!((lit[(w - 1, 0)].symbol(), lit[(w - 1, 0)].fg), ("┓", style::NOTIFY_BLUE));
        assert_eq!(wide(&unread, &Config::default(), w, false, off)[(w - 1, 0)].symbol(), "╮");
        let done = session("proj-a", Status::Done);
        let buf = wide(&done, &Config::default(), w, false, 0);
        assert!(buf[(0, 0)].modifier.contains(Modifier::DIM) && buf[(2, 1 + SCENE_ROWS)].modifier.contains(Modifier::DIM));
    }

    #[test]
    fn wide_card_falls_back_to_compact_and_never_panics() {
        let s = Session { unread_notification: true, ..session("프로젝트-아주-긴-이름", Status::Idle) };
        for (w, h) in [(CARD_W - 1, CARD_H), (CARD_W, CARD_H - 1), (34, 8)] {
            let rows = rows(&draw(w, h, |f| render_wide_card(f, f.area(), &s, &Config::default(), false, 0)));
            assert!(rows[0].contains("프로젝") && !rows.join("").contains('╭'), "{w}x{h}: {rows:?}");
        }
        for (w, h) in [(0, 0), (1, 1), (5, 2), (0, 30), (80, 0), (21, 9), (25, 9), (400, 9)] {
            for selected in [false, true] {
                draw(w, h, |f| render_wide_card(f, f.area(), &s, &Config::default(), selected, 0));
                draw(w, h, |f| render_wide_card(f, Rect::new(u16::MAX - 5, u16::MAX - 5, 30, CARD_H), &s, &Config::default(), selected, 0));
            }
        }
    }

    #[test]
    fn zero_and_tiny_areas_do_not_panic() {
        // unread too: the blinking border and dot must not change the bounds
        let s = Session { unread_notification: true, ..session("프로젝트-아주-긴-이름", Status::Idle) };
        for (w, h) in [(0, 0), (1, 1), (2, 1), (5, 2), (0, 30), (80, 0), (CARD_W - 1, CARD_H), (CARD_W, CARD_H - 1)] {
            for selected in [false, true] {
                draw(w, h, |f| render_card(f, f.area(), &s, &Config::default(), selected, 0));
                draw(w, h, |f| render_compact(f, f.area(), &s, &Config::default(), selected, 0));
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
        for r in rows(&draw(30, 1, |f| render_compact(f, f.area(), &s, &Config::default(), true, 0))) {
            println!("{r}");
        }
    }
}
