//! Session cards: an element spirit (braille, see [`crate::spirit`]) on top
//! and info lines below, in a thin rounded border.
//!
//! Full card: always [`CARD_W`] x [`CARD_H`] = 20 x 10 cells (borders
//! included; inner 18 x 8; user decision: narrowed from 24 because it looked
//! wide, then one row taller for the summary): a spirit band of
//! [`SCENE_ROWS`] rows with the small [`Size::XS`] spirit (user decision: a
//! small card gets the small spirit; its body sits on the left and the
//! magic it casts, scaled by context usage, fills the right) and
//! [`INFO_ROWS`] text rows: name, status badge (+
//! elapsed / last activity), one extra (the PR chip, else the branch), then
//! the one-line summary (rows of disabled or unknown extras collapse; with no
//! summary the last row shows the next extra, e.g. the branch the PR took the
//! row from). A larger area gets the card centered across, at its top.
//!
//! Large card ([`CardLayout::Large`], `v`): at least [`LARGE_W`] x
//! [`LARGE_H`] = 32 x 14 (inner 30 x 12, as wide as the preview's side
//! list): the bigger [`Size::S`] spirit in a [`LARGE_SCENE_ROWS`]-row band at
//! the top left with its magic in the wide area to the right, then name,
//! status badge
//! (+ elapsed / last activity, the model family word on the right), branch,
//! PR, and the summary on up to three lines; unused rows stay blank. In the
//! grid a large card fills its cell (see [`crate::ui::grid::card_rect`]).
//!
//! The preview list and the large grid stretch the card across its rect
//! ([`render_wide_card`]): same height, the body stays on the left and the
//! extra columns go to the text rows and the magic's room on the right.
//!
//! Running Claude Code subagents (see [`crate::source::subagents`]): a
//! [`SUBAGENT_GLYPH`]`×N` marker at the end of the standard card's badge row,
//! `◇N` on compact cards, one `◇ kind · description` row each (at most three,
//! then `+N`) in the large card's leftover rows, and one round bead (one
//! braille cell, an empty cell between two) each in a row along the bottom
//! of the spirit band, from the body's lower right, in the element of the
//! subagent's model (see `spirit::subdots`).
//!
//! When a card of the configured layout does not fit a compact one-line card
//! is drawn instead ([`render_compact`]). Empty areas draw nothing; no size
//! panics.

use std::time::SystemTime;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use crate::config::{CardLayout, Config};
use crate::model::{model_family, PrState, Session, Status, SubagentInfo};
use crate::source::subagents::running_count;
use crate::spirit::theme::Theme;
use crate::spirit::{self, max_grid_size, Magic, Size, SpiritParams, SubagentDot, MAX_DOTS};
use crate::ui::scene::draw_grid;
use crate::ui::style::{self, fmt_duration, truncate_to_width};

/// Spirit band size in cells: room for the largest spirit.
pub const SCENE_COLS: u16 = max_grid_size().0;
pub const SCENE_ROWS: u16 = max_grid_size().1;
const _: () = assert!(spirit::grid_size(Size::XS).1 == SCENE_ROWS, "the XS spirit fills the standard band");
/// Text rows below the spirit band.
pub const INFO_ROWS: u16 = 4;
/// Inner width of a card.
const INNER_W: u16 = 18;
/// Full card size, borders included: 20 x 10.
pub const CARD_W: u16 = INNER_W + 2;
pub const CARD_H: u16 = SCENE_ROWS + INFO_ROWS + 2;
const _: () = assert!(SCENE_COLS <= INNER_W, "the largest spirit fits the card");

/// Large card size, borders included: 32 x 14 (inner 30 x 12). The grid
/// stretches it wider to fill its cell; never narrower.
pub const LARGE_W: u16 = 32;
pub const LARGE_H: u16 = 14;
/// Spirit band of the large card: as tall as the S spirit.
pub const LARGE_SCENE_ROWS: u16 = spirit::grid_size(Size::S).1;
/// Summary lines of the large card.
const LARGE_SUMMARY_ROWS: usize = 3;
/// Running-subagent rows of the large card at most (then `+N`).
const LARGE_SUBAGENT_ROWS: usize = 3;
/// Marker of running subagents (`◇×2` on the badge row, `◇2` on compact
/// cards, `◇ kind · description` rows on the large card). A plain geometric
/// shape, not an emoji, one column wide like the other card glyphs.
pub const SUBAGENT_GLYPH: &str = "◇";
const _: () = assert!(
    spirit::grid_size_for(Size::S, Magic::Big).0 + SPIRIT_LEFT_PAD <= LARGE_W - 2,
    "big magic fits right of the padded S body"
);
const _: () = assert!(LARGE_SCENE_ROWS + 7 <= LARGE_H - 2, "band, name, badge, branch, PR and three summary lines");

/// The spirit size a card layout draws (user decision: the standard card
/// gets the small spirit, the large card the bigger one).
pub const fn spirit_size(layout: CardLayout) -> Size {
    match layout {
        CardLayout::Standard => Size::XS,
        CardLayout::Large => Size::S,
    }
}

/// Full card size (columns, rows) of a layout, borders included.
pub const fn card_size(layout: CardLayout) -> (u16, u16) {
    match layout {
        CardLayout::Standard => (CARD_W, CARD_H),
        CardLayout::Large => (LARGE_W, LARGE_H),
    }
}

/// Whether the grid draws compact one-line cards: only when `area` cannot
/// hold one full card of `layout` (the preview list keeps full cards).
pub fn compact_mode(area: Rect, layout: CardLayout) -> bool {
    let (w, h) = card_size(layout);
    area.width < w || area.height < h
}

/// The card's element: the one mapped to the session's model family, or the
/// configured default when the model is unknown (see [`Config::theme_for_model`]).
fn theme_for(s: &Session, cfg: &Config) -> Theme {
    cfg.theme_for_model(s.model.as_deref())
}

/// One dot per running subagent (newest first, at most [`MAX_DOTS`]) in the
/// element of its model family; an unknown model takes the card's element.
fn subagent_dots(s: &Session, cfg: &Config) -> Vec<SubagentDot> {
    let parent = theme_for(s, cfg);
    s.subagents
        .iter()
        .filter(|a| a.running)
        .take(MAX_DOTS)
        .map(|a| SubagentDot { theme: a.model_family.as_deref().and_then(model_family).map_or(parent, |f| cfg.model_theme(f)) })
        .collect()
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
    spirit::grid_size_for(Size::XS, Magic::Big).0 + SPIRIT_LEFT_PAD <= INNER_W,
    "big magic fits right of the padded body"
);

/// Renders the spirit for `params` at the top left of `band` (after
/// [`SPIRIT_LEFT_PAD`]) and returns the rect it was drawn into (clipped to
/// the band). User decision: the body sits on the left so the cast magic,
/// which shows the context usage, has the rest of the card to the right
/// (this replaced the centred body of the first spirit cards).
fn draw_spirit(buf: &mut Buffer, band: Rect, params: &SpiritParams, dots: &[SubagentDot]) -> Rect {
    #[cfg(test)]
    SPIRIT_RENDERS.with(|c| c.set(c.get() + 1));
    let grid = spirit::render_dots(params, dots);
    let pad = if band.width > SPIRIT_LEFT_PAD { SPIRIT_LEFT_PAD } else { 0 };
    let x = band.x + pad;
    let (cols, rows) = (grid.cols.min(band.right().saturating_sub(x)), grid.rows.min(band.height));
    let at = Rect::new(x, band.y, cols, rows);
    draw_grid(buf, at, &grid);
    at
}

/// Draws one session card with its spirit at animation frame `tick`: a card
/// of the configured layout ([`card_size`]) centered across the top of
/// `area`. Too small an area falls back to [`render_compact`].
pub fn render_card(f: &mut Frame, area: Rect, s: &Session, cfg: &Config, selected: bool, tick: u64) {
    render_full(f, area, s, cfg, selected, tick, false);
}

/// Like [`render_card`], but the card is as wide as `area` (at least the
/// layout's card width), at its top left: the preview's one-column list.
pub fn render_wide_card(f: &mut Frame, area: Rect, s: &Session, cfg: &Config, selected: bool, tick: u64) {
    render_full(f, area, s, cfg, selected, tick, true);
}

fn render_full(f: &mut Frame, area: Rect, s: &Session, cfg: &Config, selected: bool, tick: u64, stretch: bool) {
    let area = area.intersection(f.area());
    if area.is_empty() {
        return;
    }
    let layout = cfg.card_layout;
    if compact_mode(area, layout) {
        render_compact(f, area, s, cfg, selected, tick);
        return;
    }
    let (card_w, card_h) = card_size(layout);
    let rect = if stretch {
        Rect { height: card_h, ..area }
    } else {
        Rect::new(area.x + (area.width - card_w) / 2, area.y, card_w, card_h)
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

    let (size, scene_rows) = match layout {
        CardLayout::Standard => (spirit_size(layout), SCENE_ROWS),
        CardLayout::Large => (spirit_size(layout), LARGE_SCENE_ROWS),
    };
    let band = Rect { height: scene_rows.min(inner.height), ..inner };
    let params = SpiritParams {
        theme: theme_for(s, cfg),
        status: s.status,
        size,
        tick,
        seed: seed_for(s),
        warning: spirit::warning(s.ctx_percent),
        magic: spirit::magic_for(s.ctx_percent),
        stage: spirit::stage_for(s.ctx_percent),
        cols: band.width.saturating_sub(SPIRIT_LEFT_PAD),
    };
    draw_spirit(f.buffer_mut(), band, &params, &subagent_dots(s, cfg));

    let text_area = Rect::new(inner.x, inner.y + band.height, inner.width, inner.height - band.height);
    let (width, now) = (usize::from(text_area.width), SystemTime::now());
    let mut lines = match layout {
        CardLayout::Standard => text_lines(s, cfg, width, now),
        CardLayout::Large => large_text_lines(s, cfg, width, usize::from(text_area.height), now),
    };
    lines.truncate(usize::from(text_area.height));
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
    let running = running_count(&s.subagents);
    let marker = (running > 0).then(|| format!(" {SUBAGENT_GLYPH}{running}"));
    let name_w = s.name.width();
    // Keep at least a few name columns before adding the badge and elapsed.
    let need = name_w.min(6) + 1;
    let tag_w = tag.width();
    let el_w = elapsed.as_deref().map_or(0, str::width);
    let mk_w = marker.as_deref().map_or(0, str::width);
    // Dropped when too narrow: elapsed first, then the marker, then the badge.
    let (show_tag, show_el, show_mk) = if need + tag_w + el_w + mk_w <= avail {
        (true, true, true)
    } else if need + tag_w + mk_w <= avail {
        (true, false, true)
    } else {
        (need + tag_w <= avail, false, false)
    };
    let tail_w = if show_tag { 1 + tag_w } else { 0 } + if show_el { el_w } else { 0 } + if show_mk { mk_w } else { 0 };
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
    if let Some(m) = marker.filter(|_| show_mk) {
        spans.push(Span::styled(m, subagent_style()));
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
/// name, status badge (+ elapsed / last activity), the first enabled extra
/// (PR chip, else branch with `~` when guessed, else `PR ?` when the lookup
/// failed), the one-line summary, then the remaining extras. A full card
/// keeps four rows: the summary is always the fourth when there is one, and
/// with no summary the next extra (e.g. the branch the PR took the row from)
/// takes its place.
fn text_lines(s: &Session, cfg: &Config, width: usize, now: SystemTime) -> Vec<Line<'static>> {
    let running = running_count(&s.subagents);
    let marker = (running > 0).then(|| format!(" {SUBAGENT_GLYPH}×{running}"));
    let mut lines = vec![name_line(s, width), Line::from(badge_spans(s, cfg, width, now, marker))];
    let mut extras = extra_lines(s, cfg, width);
    let summary = s.summary.as_ref().filter(|_| cfg.extras.summary).map(|sum| Line::styled(truncate_to_width(sum, width), dim()));
    if !extras.is_empty() {
        lines.push(extras.remove(0));
    }
    lines.extend(summary);
    lines.extend(extras);
    lines
}

/// Info lines of the large card: name, status badge (+ elapsed / last
/// activity) with the model family word on the right when it fits, branch,
/// PR chip (or `PR ?`), the summary on up to three lines, then the running
/// subagents in the rows left of `rows` ([`subagent_lines`]). Rows of
/// disabled or unknown extras collapse.
fn large_text_lines(s: &Session, cfg: &Config, width: usize, rows: usize, now: SystemTime) -> Vec<Line<'static>> {
    let ex = &cfg.extras;
    let mut badge = badge_spans(s, cfg, width, now, None);
    let used: usize = badge.iter().map(Span::width).sum();
    if let Some(word) = s.model.as_deref().and_then(model_family).map(|f| f.as_key()) {
        // the badge keeps its tokens; the word goes when there is no room
        if used + 1 + word.width() <= width {
            badge.push(Span::raw(" ".repeat(width - used - word.width())));
            badge.push(Span::styled(word, dim()));
        }
    }
    let mut lines = vec![name_line(s, width), Line::from(badge)];
    lines.extend(branch_line(s, cfg, width));
    lines.extend(pr_line(s, cfg, width));
    if let Some(sum) = s.summary.as_ref().filter(|_| ex.summary) {
        lines.extend(summary_lines(sum, width, LARGE_SUMMARY_ROWS).into_iter().map(|l| Line::styled(l, dim())));
    }
    let free = rows.saturating_sub(lines.len());
    lines.extend(subagent_lines(s, width, free));
    lines
}

fn subagent_style() -> Style {
    Style::new().fg(style::SELECTED)
}

/// Rows of the running subagents (newest first), at most `rows` and
/// [`LARGE_SUBAGENT_ROWS`]: `◇ kind · description` cut to the width
/// (CJK-aware) with the model family word on the right when it fits; when
/// more are running than shown, the last row ends with `+N` instead.
fn subagent_lines(s: &Session, width: usize, rows: usize) -> Vec<Line<'static>> {
    let running: Vec<&SubagentInfo> = s.subagents.iter().filter(|a| a.running).collect();
    let shown = running.len().min(rows).min(LARGE_SUBAGENT_ROWS);
    if width == 0 {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(shown);
    for (i, a) in running.iter().take(shown).enumerate() {
        let more = running.len() - shown;
        let right = if i + 1 == shown && more > 0 { Some(format!("+{more}")) } else { a.model_family.clone() };
        let left = if a.description.is_empty() {
            format!("{SUBAGENT_GLYPH} {}", a.kind)
        } else {
            format!("{SUBAGENT_GLYPH} {} · {}", a.kind, a.description)
        };
        let mut spans = Vec::new();
        match right.filter(|r| r.width() + 4 <= width) {
            Some(r) => {
                let text = truncate_to_width(&left, width - r.width() - 1);
                let pad = width - text.width() - r.width();
                spans.push(Span::styled(text, Style::new().fg(style::TEXT)));
                spans.push(Span::raw(" ".repeat(pad)));
                spans.push(Span::styled(r, dim()));
            }
            None => spans.push(Span::styled(truncate_to_width(&left, width), Style::new().fg(style::TEXT))),
        }
        out.push(Line::from(spans));
    }
    out
}

fn dim() -> Style {
    Style::new().fg(style::TEXT_DIM)
}

fn name_line(s: &Session, width: usize) -> Line<'static> {
    Line::styled(truncate_to_width(&s.name, width), Style::new().fg(style::TEXT).add_modifier(Modifier::BOLD))
}

/// Badge row: whole tokens only, then the subagent `marker` at the end. When
/// too narrow, last activity goes first, then the marker, then elapsed; the
/// badge itself is truncated as a last resort.
fn badge_spans(s: &Session, cfg: &Config, width: usize, now: SystemTime, marker: Option<String>) -> Vec<Span<'static>> {
    let ex = &cfg.extras;
    let text = Style::new().fg(style::TEXT);
    let tag = truncate_to_width(&format!(" {} ", style::status_tag(s.status)), width);
    let used = tag.width();
    let color = style::status_color(s.status);
    let mut badge = vec![Span::styled(tag, Style::new().fg(style::BADGE_FG).bg(color).add_modifier(Modifier::BOLD))];
    let elapsed = ex.elapsed.then(|| elapsed_text(s, now)).flatten().map(|d| format!(" {d}"));
    let last = ex.last_activity.then(|| since(s.updated_at, now)).flatten().map(|d| format!(" ·{d} ago"));
    let ew = elapsed.as_deref().map_or(0, str::width);
    let lw = last.as_deref().map_or(0, str::width);
    let mw = marker.as_deref().map_or(0, str::width);
    let keep_elapsed = used + ew <= width;
    let keep_marker = used + if keep_elapsed { ew } else { 0 } + mw <= width;
    let keep_last = keep_elapsed && used + ew + lw + if keep_marker { mw } else { 0 } <= width;
    if let Some(e) = elapsed.filter(|_| keep_elapsed) {
        badge.push(Span::styled(e, text));
    }
    if let Some(l) = last.filter(|_| keep_last) {
        badge.push(Span::styled(l, dim()));
    }
    if let Some(m) = marker.filter(|_| keep_marker) {
        badge.push(Span::styled(m, subagent_style()));
    }
    badge
}

/// The PR chip, else `PR ?` when the lookup failed (PR extra on).
fn pr_line(s: &Session, cfg: &Config, width: usize) -> Option<Line<'static>> {
    if !cfg.extras.pr {
        return None;
    }
    match &s.pr {
        Some(pr) => {
            let pr_color = match pr.state {
                PrState::Approved => style::DONE,
                PrState::Conflict => style::FLASH_ALERT,
                _ => style::TEXT_DIM,
            };
            Some(Line::styled(truncate_to_width(&format!("#{} {}", pr.number, pr_label(pr.state)), width), Style::new().fg(pr_color)))
        }
        // The lookup failed; the footer says why.
        None if s.pr_unavailable => Some(Line::styled(truncate_to_width("PR ?", width), dim())),
        None => None,
    }
}

/// The branch (`~` when guessed), branch extra on.
fn branch_line(s: &Session, cfg: &Config, width: usize) -> Option<Line<'static>> {
    let b = s.branch.as_ref().filter(|_| cfg.extras.branch)?;
    let tilde = if s.branch_guessed { "~" } else { "" };
    Some(Line::styled(truncate_to_width(&format!("⎇ {tilde}{b}"), width), Style::new().fg(style::TEXT)))
}

/// The enabled extras in priority order: PR chip, branch, `PR ?` (a known
/// branch beats an unknown PR).
fn extra_lines(s: &Session, cfg: &Config, width: usize) -> Vec<Line<'static>> {
    let (pr, branch) = (pr_line(s, cfg, width), branch_line(s, cfg, width));
    match (s.pr.is_some(), pr, branch) {
        (true, pr, branch) => pr.into_iter().chain(branch).collect(),
        (false, pr, branch) => branch.into_iter().chain(pr).collect(),
    }
}

/// `text` wrapped onto at most `rows` lines of `width` columns (CJK-aware:
/// wide characters count two). A line breaks at the last space when the
/// break would split a word and that space is past half the line; the last
/// line is cut with `…`. Control characters show as spaces.
fn summary_lines(text: &str, width: usize, rows: usize) -> Vec<String> {
    let mut out = Vec::new();
    if width == 0 {
        return out;
    }
    let clean: String = text.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    let mut rest = clean.trim();
    while !rest.is_empty() && out.len() < rows {
        if out.len() + 1 == rows || rest.width() <= width {
            out.push(truncate_to_width(rest, width));
            break;
        }
        let (mut used, mut cut) = (0, 0);
        for (i, c) in rest.char_indices() {
            let w = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
            if used + w > width {
                break;
            }
            used += w;
            cut = i + c.len_utf8();
        }
        if cut == 0 {
            // a character wider than the line
            out.push(truncate_to_width(rest, width));
            break;
        }
        if rest[cut..].starts_with(|c: char| !c.is_whitespace()) {
            if let Some(sp) = rest[..cut].rfind(char::is_whitespace) {
                if rest[..sp].width() * 2 >= width {
                    cut = sp;
                }
            }
        }
        out.push(rest[..cut].trim_end().to_string());
        rest = rest[cut..].trim_start();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{CardLayout, Extras};
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
            subagents: Vec::new(),
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
    fn card_is_exactly_20x10() {
        assert_eq!((CARD_W, CARD_H), (20, 10), "user decision: narrowed from 24, one summary row added");
        assert_eq!((SCENE_ROWS, INFO_ROWS), (4, 4), "inner 18x8: spirit band + 4 text rows");
        assert_eq!((SCENE_COLS, SCENE_ROWS), max_grid_size());
        for size in Size::ALL {
            let (w, h) = spirit::grid_size(size);
            assert!(w <= CARD_W - 2 && h == SCENE_ROWS, "{size:?} fits the band: {w}x{h}");
        }
        let s = session("proj-a", Status::Running);
        let rows = card(&s, &all_extras(), true);
        assert_eq!(rows.len(), 10);
        assert!(rows[0].starts_with('╭') && rows[0].ends_with('╮'), "{rows:?}");
        assert!(rows[9].starts_with('╰') && rows[9].ends_with('╯'), "{rows:?}");
        assert!(rows[1..9].iter().all(|r| r.starts_with('│') && r.ends_with('│')), "{rows:?}");
        // a larger area still gets a 20x10 card (centered across, at the top); nothing else drawn
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
    fn card_contains_spirit_and_four_info_lines() {
        let s = session("proj-a", Status::Running);
        let rows = rows(&draw(80, 24, |f| render_card(f, Rect::new(0, 0, CARD_W, CARD_H), &s, &Config::default(), false, 0)));
        let text = rows.join("\n");
        let last = CARD_H as usize - 1;
        assert!(rows[0].starts_with('╭') && rows[last].starts_with('╰'), "thin rounded border: {text}");
        let scene_rows: Vec<usize> = (1..last).filter(|&y| rows[y].chars().any(is_braille)).collect();
        assert!(!scene_rows.is_empty() && scene_rows.iter().all(|&y| y <= SCENE_ROWS as usize), "spirit in the 4-row band: {text}");
        let info_rows: Vec<String> = (1 + SCENE_ROWS as usize..last).map(|y| info(&rows[y])).collect();
        assert_eq!(info_rows.len(), 4);
        assert_eq!(info_rows[0], "proj-a", "{text}");
        assert!(info_rows[1].contains("RUN") && info_rows[1].contains("3h"), "{text}");
        assert_eq!(info_rows[2], "⎇ feat/login", "{text}");
        assert_eq!(info_rows[3], "fixing the parser", "the one-line summary: {text}");
        assert!(!text.contains("#42"), "PR off: {text}");
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
        for size in [Size::XS, Size::S, Size::M, Size::L] {
            for magic in Magic::ALL {
                for tick in [0, 30, 61] {
                    let params = SpiritParams { theme: Theme::Water, status: Status::Running, size, tick, seed: 1, warning: false, magic, stage: Default::default(), cols: 0 };
                    let mut buf = ratatui::buffer::Buffer::empty(Rect::new(0, 0, CARD_W, CARD_H));
                    let at = draw_spirit(&mut buf, band, &params, &[]);
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
        let at = draw_spirit(&mut buf, narrow, &params, &[]);
        assert!(at.x >= narrow.x && at.right() <= narrow.right());
    }

    #[test]
    fn spirit_body_sits_left_in_the_inner_area() {
        let band = Rect::new(1, 1, CARD_W - 2, SCENE_ROWS);
        for theme in Theme::ALL {
            for status in [Status::Idle, Status::Running] {
                for magic in Magic::ALL {
                    let size = spirit_size(CardLayout::Standard);
                    let params = SpiritParams { theme, status, size, tick: 3, seed: 1, warning: false, magic, stage: Default::default(), cols: 0 };
                    let mut buf = Buffer::empty(Rect::new(0, 0, CARD_W, CARD_H));
                    draw_spirit(&mut buf, band, &params, &[]);
                    let lo = (0..CARD_W)
                        .find(|&x| (0..CARD_H).any(|y| buf[(x, y)].symbol().chars().any(is_braille)))
                        .expect("painted");
                    // about one free column between the border and the body
                    assert!((band.x + 1..=band.x + 2).contains(&lo), "{theme:?} {status:?} {magic:?}: body left edge at {lo}");
                    let body_c = band.x as f32 + SPIRIT_LEFT_PAD as f32 + spirit::body_center_x(size);
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
        let (none, low, mid, high) = (reach(None), reach(Some(10.0)), reach(Some(30.0)), reach(Some(80.0)));
        assert_eq!(none, mid, "unknown ctx casts normal magic");
        assert!(low < mid && mid < high, "magic grows with ctx: {low} {mid} {high}");
        // every level stays inside the inner area
        for pct in [10.0, 30.0, 50.0, 80.0, 95.0] {
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
    fn info_rows_keep_pr_over_branch_then_the_summary_and_hide_disabled_extras() {
        let mut s = session("proj-a", Status::Running);
        let four = |s: &Session, cfg: &Config| -> Vec<String> {
            let first = 1 + SCENE_ROWS as usize;
            card(s, cfg, false)[first..first + 4].iter().map(|r| info(r)).collect()
        };
        assert_eq!(four(&s, &all_extras()), vec!["proj-a", " RUN  3h ·2m ago", "#42 approved", "fixing the parser"], "PR first, then the summary");
        let mut cfg = all_extras();
        cfg.extras.pr = false;
        assert_eq!(four(&s, &cfg)[2..], ["⎇ feat/login", "fixing the parser"], "then the branch");
        s.branch = None;
        assert_eq!(four(&s, &cfg)[2..], ["fixing the parser", ""], "neither: the summary moves up");
        s.branch = Some("feat/login".into());
        // no summary: the next best text (the branch the PR took the row from)
        s.summary = None;
        assert_eq!(four(&s, &all_extras())[2..], ["#42 approved", "⎇ feat/login"]);
        assert_eq!(four(&s, &cfg)[2..], ["⎇ feat/login", ""], "else blank");
        s.summary = Some("fixing the parser".into());
        let mut no_sum = all_extras();
        no_sum.extras.summary = false;
        assert_eq!(four(&s, &no_sum)[2..], ["#42 approved", "⎇ feat/login"], "summary off counts as empty");
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
    fn summary_row_is_cut_to_the_inner_width_cjk_aware() {
        let mut s = session("proj-a", Status::Running);
        s.summary = Some("한글 요약이 아주 길어서 카드 안에서 잘려야 합니다".into());
        let rows = card(&s, &Config::default(), false);
        let row = &rows[usize::from(SCENE_ROWS) + 4];
        assert!(row.starts_with('│') && row.ends_with('│'), "border intact: {row:?}");
        assert_eq!(row.width(), usize::from(CARD_W), "{row:?}");
        let text = info(row);
        assert!(text.starts_with("한글 요약이") && text.ends_with('…'), "{text:?}");
        assert!(text.width() <= usize::from(CARD_W - 2), "{text:?}");
        // a summary that fits is shown whole
        s.summary = Some("짧은 요약".into());
        assert_eq!(info(&card(&s, &Config::default(), false)[usize::from(SCENE_ROWS) + 4]), "짧은 요약");
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
        for layout in [CardLayout::Standard, CardLayout::Large] {
            let (w, h) = card_size(layout);
            let full = Rect::new(0, 0, w, h);
            assert!(!compact_mode(full, layout));
            assert!(!compact_mode(Rect::new(0, 0, 500, 500), layout), "the preview list keeps full cards");
            assert!(compact_mode(Rect { width: w - 1, ..full }, layout));
            assert!(compact_mode(Rect { height: h - 1, ..full }, layout));
            assert!(compact_mode(Rect::default(), layout));
        }
        // a standard-sized area is too small for a large card
        assert!(compact_mode(Rect::new(0, 0, CARD_W, CARD_H), CardLayout::Large));
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
            assert_eq!(info_rows, vec!["proj-a", " RUN  3h ·2m ago", "#42 approved", "fixing the parser"], "w={w}");
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

    // ---- the large layout (B) ----

    fn large_cfg() -> Config {
        Config { card_layout: CardLayout::Large, ..all_extras() }
    }

    fn opus(name: &str, status: Status) -> Session {
        Session { model: Some("claude-opus-4-5".into()), ..session(name, status) }
    }

    fn large_rows(s: &Session, cfg: &Config) -> Vec<String> {
        rows(&draw(LARGE_W, LARGE_H, |f| render_card(f, f.area(), s, cfg, false, 0)))
    }

    /// The text rows of a large card (inside the border, below the spirit band).
    fn large_info(s: &Session, cfg: &Config) -> Vec<String> {
        let rows = large_rows(s, cfg);
        rows[1 + usize::from(LARGE_SCENE_ROWS)..usize::from(LARGE_H) - 1].iter().map(|r| info(r)).collect()
    }

    #[test]
    fn layouts_have_exact_card_sizes() {
        assert_eq!(card_size(CardLayout::Standard), (20, 10));
        assert_eq!(card_size(CardLayout::Large), (32, 14), "about 2.3:1, as wide as the preview's side list");
        assert_eq!((LARGE_W, LARGE_H, LARGE_SCENE_ROWS), (32, 14, 4));
        assert_eq!(spirit::grid_size(Size::S).1, LARGE_SCENE_ROWS, "the S spirit fills the large band");
        assert_eq!(spirit::grid_size(Size::XS).1, SCENE_ROWS, "the XS spirit fills the standard band");
        let s = opus("proj-a", Status::Running);
        for (layout, (w, h)) in [(CardLayout::Standard, (20u16, 10u16)), (CardLayout::Large, (32, 14))] {
            let cfg = Config { card_layout: layout, ..all_extras() };
            let rows = rows(&draw(w, h, |f| render_card(f, f.area(), &s, &cfg, true, 0)));
            let last = usize::from(h) - 1;
            assert!(rows[0].starts_with('╭') && rows[0].ends_with('╮'), "{layout:?}: {rows:?}");
            assert!(rows[last].starts_with('╰') && rows[last].ends_with('╯'), "{layout:?}: {rows:?}");
            assert!(rows[1..last].iter().all(|r| r.starts_with('│') && r.ends_with('│') && r.width() == usize::from(w)), "{layout:?}: {rows:?}");
            // a larger area: the card centered across at the top, nothing drawn outside it
            let (aw, ah) = (w + 21, h + 7);
            let buf = draw(aw, ah, |f| render_card(f, f.area(), &s, &cfg, false, 3));
            let x0 = (aw - w) / 2;
            assert_eq!(buf[(x0, 0)].symbol(), "╭", "{layout:?}");
            assert_eq!(buf[(x0 + w - 1, h - 1)].symbol(), "╯", "{layout:?}");
            for y in 0..ah {
                for x in 0..aw {
                    let inside = (x0..x0 + w).contains(&x) && y < h;
                    assert!(inside || buf[(x, y)].symbol() == " ", "{layout:?}: ({x},{y}) drawn outside the card");
                }
            }
            // one row or column short: the compact one-line card
            for (cw, ch) in [(w - 1, h), (w, h - 1)] {
                let r = rows_of(&s, &cfg, cw, ch);
                assert!(r[0].contains("proj-a") && !r.join("").contains('╭'), "{layout:?} {cw}x{ch}: {r:?}");
            }
        }
    }

    /// The spirit band of a card drawn into `buf` at (x0, y0) matches the
    /// spirit of `size` rendered for the session, cell for cell.
    fn band_shows(buf: &Buffer, x0: u16, y0: u16, cols: u16, s: &Session, cfg: &Config, size: Size, tick: u64) -> bool {
        let p = SpiritParams {
            theme: theme_for(s, cfg),
            status: s.status,
            size,
            tick,
            seed: seed_for(s),
            warning: spirit::warning(s.ctx_percent),
            magic: spirit::magic_for(s.ctx_percent),
            stage: spirit::stage_for(s.ctx_percent),
            cols,
        };
        let g = spirit::render(&p);
        (0..g.rows).all(|r| {
            (0..g.cols.min(cols)).all(|c| {
                let want = g.cells[usize::from(r) * usize::from(g.cols) + usize::from(c)].ch;
                buf[(x0 + 1 + SPIRIT_LEFT_PAD + c, y0 + 1 + r)].symbol() == want.to_string()
            })
        })
    }

    #[test]
    fn standard_card_draws_the_small_sprite_and_large_card_the_s_sprite() {
        // user decision: a small card gets the small spirit, a large card the bigger one
        assert_eq!(spirit_size(CardLayout::Standard), Size::XS);
        assert_eq!(spirit_size(CardLayout::Large), Size::S);
        for status in [Status::Running, Status::Idle, Status::NeedsInput, Status::Done] {
            for pct in [None, Some(10.0), Some(60.0)] {
                for tick in [0, 7, 44] {
                    let s = Session { ctx_percent: pct, ..session("proj-a", status) };
                    for (layout, (w, h), stretch) in [(CardLayout::Standard, (CARD_W, CARD_H), 0u16), (CardLayout::Standard, (CARD_W, CARD_H), 12), (CardLayout::Large, (LARGE_W, LARGE_H), 0), (CardLayout::Large, (LARGE_W, LARGE_H), 9)] {
                        let cfg = Config { card_layout: layout, ..all_extras() };
                        let w = w + stretch;
                        let buf = draw(w, h, |f| render_wide_card(f, f.area(), &s, &cfg, false, tick));
                        let cols = w - 2 - SPIRIT_LEFT_PAD;
                        let other = if layout == CardLayout::Standard { Size::S } else { Size::XS };
                        assert!(band_shows(&buf, 0, 0, cols, &s, &cfg, spirit_size(layout), tick), "{layout:?} +{stretch} {status:?} {pct:?} {tick}");
                        assert!(!band_shows(&buf, 0, 0, cols, &s, &cfg, other, tick), "{layout:?} {status:?}: not the {other:?} spirit");
                    }
                }
            }
        }
    }

    #[test]
    fn standard_cards_keep_the_small_sprite_and_its_magic_inside() {
        // centred standard cards and the stretched ones of the preview column
        let cfg = all_extras();
        for (cw, pct) in [(CARD_W, 80.0), (CARD_W, 30.0), (CARD_W, 10.0), (26, 80.0), (34, 60.0), (40, 30.0)] {
            for theme in Theme::ALL {
                let cfg = Config { theme, ..cfg.clone() };
                let s = Session { ctx_percent: Some(pct), ..session("proj-a", Status::Running) };
                for t in [0, 9, 23, 44, 61, 90] {
                    let rect = Rect::new(3, 2, cw, CARD_H);
                    for stretch in [false, true] {
                        let buf = draw(cw + 8, CARD_H + 5, |f| if stretch { render_wide_card(f, rect, &s, &cfg, false, t) } else { render_card(f, rect, &s, &cfg, false, t) });
                        for y in 0..CARD_H + 5 {
                            for x in 0..cw + 8 {
                                let inside = (3..3 + cw).contains(&x) && (2..2 + CARD_H).contains(&y);
                                assert!(inside || buf[(x, y)].symbol() == " ", "w={cw} t={t}: ({x},{y}) outside the card");
                            }
                        }
                        let lit: Vec<(u16, u16)> = (0..CARD_H + 5).flat_map(|y| (0..cw + 8).map(move |x| (x, y))).filter(|&p| buf[p].symbol().chars().any(is_braille)).collect();
                        assert!(!lit.is_empty(), "w={cw} t={t}");
                        // the card's left and right border columns
                        let (l, r) = if stretch { (3, 3 + cw - 1) } else { (3 + (cw - CARD_W) / 2, 3 + (cw - CARD_W) / 2 + CARD_W - 1) };
                        assert!(lit.iter().all(|&(x, y)| (3..=2 + SCENE_ROWS).contains(&y) && (l + 1..r).contains(&x)), "{theme:?} w={cw} t={t}: spirit and magic in the band: {lit:?}");
                        assert!(lit.iter().map(|p| p.0).min().unwrap() <= l + 4, "w={cw}: body on the left");
                    }
                }
            }
        }
    }

    fn rows_of(s: &Session, cfg: &Config, w: u16, h: u16) -> Vec<String> {
        rows(&draw(w, h, |f| render_card(f, f.area(), s, cfg, false, 0)))
    }

    #[test]
    fn large_card_spirit_is_s_in_a_four_row_band_body_left() {
        for theme in Theme::ALL {
            let mut cfg = large_cfg();
            cfg.theme = theme;
            for status in [Status::Running, Status::Idle, Status::NeedsInput, Status::Done, Status::Unknown] {
                for tick in [0, 7, 44] {
                    let s = session("proj-a", status);
                    let buf = draw(LARGE_W, LARGE_H, |f| render_card(f, f.area(), &s, &cfg, false, tick));
                    let lit: Vec<(u16, u16)> = (0..LARGE_H).flat_map(|y| (0..LARGE_W).map(move |x| (x, y))).filter(|&p| buf[p].symbol().chars().any(is_braille)).collect();
                    assert!(!lit.is_empty(), "{theme:?} {status:?} {tick}");
                    assert!(lit.iter().all(|&(x, y)| (1..=LARGE_SCENE_ROWS).contains(&y) && (1..LARGE_W - 1).contains(&x)), "{theme:?} {status:?} {tick}: spirit in the band: {lit:?}");
                    let lo = lit.iter().map(|p| p.0).min().unwrap();
                    assert!(lo <= 3, "{theme:?} {status:?}: body on the left ({lo})");
                    for (x, y) in lit {
                        assert_eq!(buf[(x, y)].bg, Color::Reset, "no background");
                    }
                }
            }
        }
        // the magic grows with ctx in the area right of the body (cells
        // painted from column 9 on, summed over a few casts)
        let painted = |pct: f32| -> usize {
            let s = Session { ctx_percent: Some(pct), ..session("proj-a", Status::Running) };
            (0..120)
                .map(|t| {
                    let buf = draw(LARGE_W, LARGE_H, |f| render_card(f, f.area(), &s, &large_cfg(), false, t));
                    (1..=LARGE_SCENE_ROWS).flat_map(|y| (9..LARGE_W - 1).map(move |x| (x, y))).filter(|&p| buf[p].symbol().chars().any(is_braille)).count()
                })
                .sum()
        };
        let (low, mid, high) = (painted(10.0), painted(30.0), painted(80.0));
        assert!(low < mid && mid < high, "fire breath grows: {low} {mid} {high}");
    }

    #[test]
    fn large_card_text_rows_name_badge_model_branch_pr_summary() {
        let s = opus("proj-a", Status::Running);
        let rows = large_info(&s, &large_cfg());
        assert_eq!(rows.len(), usize::from(LARGE_H - 2 - LARGE_SCENE_ROWS));
        assert_eq!(rows[0], "proj-a");
        assert!(rows[1].starts_with(" RUN  3h ·2m ago") && rows[1].ends_with("opus"), "model word on the right: {:?}", rows[1]);
        assert_eq!(rows[1].width(), usize::from(LARGE_W - 2), "right-aligned to the inner edge: {:?}", rows[1]);
        assert_eq!(rows[2..6], ["⎇ feat/login", "#42 approved", "fixing the parser", ""]);
        assert!(rows[6..].iter().all(String::is_empty), "the rest is blank: {rows:?}");
        // PR off: the summary moves up; no model: no word
        let mut cfg = large_cfg();
        cfg.extras.pr = false;
        let plain = Session { model: None, ..s.clone() };
        let rows = large_info(&plain, &cfg);
        assert_eq!(rows[1], " RUN  3h ·2m ago");
        assert_eq!(rows[2..4], ["⎇ feat/login", "fixing the parser"]);
        // a failed PR lookup shows `PR ?` on the PR row
        let unknown = Session { pr: None, pr_unavailable: true, ..s.clone() };
        assert_eq!(large_info(&unknown, &large_cfg())[2..5], ["⎇ feat/login", "PR ?", "fixing the parser"]);
        // every family word
        for (m, w) in [("claude-fable-5-1", "fable"), ("claude-sonnet-5-5", "sonnet"), ("claude-haiku-5-5", "haiku")] {
            let x = Session { model: Some(m.into()), ..s.clone() };
            assert!(large_info(&x, &large_cfg())[1].ends_with(w), "{m}");
        }
        // a narrow badge row drops the model word rather than cutting the badge
        let lines = large_text_lines(&s, &all_extras(), 18, 8, SystemTime::now());
        let badge: String = lines[1].spans.iter().map(|sp| sp.content.as_ref()).collect();
        assert_eq!(badge, " RUN  3h ·2m ago", "{badge:?}");
        // the standard card still never names the model
        let std = rows_of(&s, &all_extras(), CARD_W, CARD_H).join("\n");
        assert!(!std.contains("opus"), "{std}");
    }

    #[test]
    fn large_card_wraps_the_summary_on_three_lines_cjk_aware() {
        let mut s = session("proj-a", Status::Running);
        s.pr = None;
        s.summary = Some("한글 요약이 아주 길어서 세 줄로 나뉘고 그래도 넘치면 마지막 줄 끝에서 잘려야 합니다 정말로 그렇습니다 끝까지 읽어 보세요".into());
        let rows = large_rows(&s, &large_cfg());
        assert!(rows.iter().all(|r| r.width() == usize::from(LARGE_W)), "borders intact: {rows:?}");
        let info = large_info(&s, &large_cfg());
        let (l1, l2, l3) = (&info[3], &info[4], &info[5]);
        assert!(l1.starts_with("한글 요약이") && !l1.ends_with('…') && !l2.ends_with('…'), "{l1:?} {l2:?}");
        assert!(l3.ends_with('…') && !l3.is_empty(), "{l3:?}");
        assert!([l1, l2, l3].iter().all(|l| l.width() <= 30), "{info:?}");
        assert!(info[6..].iter().all(String::is_empty), "three lines at most, the rest blank: {info:?}");
        // words are not split when a space is near the end of a line
        s.summary = Some("alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu".into());
        let info = large_info(&s, &large_cfg());
        assert_eq!(info[3..6], ["alpha beta gamma delta epsilon", "zeta eta theta iota kappa", "lambda mu"]);
        // a summary that fits takes one line
        s.summary = Some("가".repeat(15));
        let info = large_info(&s, &large_cfg());
        assert_eq!((info[3].as_str(), info[4].as_str()), ("가".repeat(15).as_str(), ""));
        assert_eq!(summary_lines("", 10, 2), Vec::<String>::new());
        assert_eq!(summary_lines("abc", 0, 2), Vec::<String>::new());
        assert_eq!(summary_lines("가나다라", 3, 2), vec!["가", "나…"], "wide chars never overflow");
    }

    #[test]
    fn large_card_keeps_selection_unread_blink_and_done_dim() {
        let (on, off) = blink_ticks();
        let cfg = large_cfg();
        let s = session("proj-a", Status::Running);
        let at = |s: &Session, sel: bool, t: u64| draw(LARGE_W, LARGE_H, |f| render_card(f, f.area(), s, &cfg, sel, t));
        let buf = at(&s, true, 0);
        assert_eq!((buf[(0, 0)].symbol(), buf[(0, 0)].fg), ("╭", style::SELECTED));
        assert_eq!(buf[(LARGE_W - 1, 6)].fg, style::SELECTED);
        let unread = Session { unread_notification: true, ..s.clone() };
        assert_eq!((at(&unread, false, on)[(0, 0)].symbol(), at(&unread, false, on)[(0, 0)].fg), ("┏", style::NOTIFY_BLUE));
        assert_eq!(at(&unread, false, off)[(0, 0)].symbol(), "╭");
        let done = session("proj-a", Status::Done);
        let buf = at(&done, false, 0);
        assert!(buf[(0, 0)].modifier.contains(Modifier::DIM) && buf[(2, 1 + LARGE_SCENE_ROWS)].modifier.contains(Modifier::DIM));
    }

    #[test]
    fn large_wide_card_fills_the_side_list_and_never_panics() {
        let s = opus("proj-abcdefghijklmnopqrstuvwxyz", Status::Idle);
        let cfg = large_cfg();
        for w in [LARGE_W, 36, 40] {
            let rows = rows(&draw(w, LARGE_H + 2, |f| render_wide_card(f, f.area(), &s, &cfg, false, 0)));
            assert_eq!(rows[0].width(), usize::from(w), "{rows:?}");
            assert!(rows[0].starts_with('╭') && rows[0].ends_with('╮') && rows[usize::from(LARGE_H) - 1].starts_with('╰'), "{rows:?}");
            assert!(rows[usize::from(LARGE_H)..].iter().all(|r| r.trim().is_empty()), "exactly LARGE_H tall");
        }
        let u = Session { unread_notification: true, ..s.clone() };
        for (w, h) in [(0, 0), (1, 1), (5, 2), (31, 14), (32, 13), (32, 14), (400, 14), (0, 30)] {
            for selected in [false, true] {
                draw(w, h, |f| render_wide_card(f, f.area(), &u, &cfg, selected, 0));
                draw(w, h, |f| render_card(f, f.area(), &u, &cfg, selected, u64::MAX));
                draw(w, h, |f| render_card(f, Rect::new(u16::MAX - 5, u16::MAX - 5, LARGE_W, LARGE_H), &u, &cfg, selected, 0));
            }
        }
    }

    /// A grid cell wider than the large card: the card fills it, the body
    /// stays on the left, the text rows and the magic get the extra columns,
    /// and nothing is drawn outside the card.
    #[test]
    fn large_card_stretched_to_its_cell_keeps_everything_inside() {
        let cfg = large_cfg();
        let mut s = opus("proj-a", Status::Running);
        s.summary = Some("x".repeat(130));
        for (cw, ctx) in [(LARGE_W, 80.0), (37, 80.0), (40, 45.0), (37, 10.0)] {
            s.ctx_percent = Some(ctx);
            for t in [0, 9, 44] {
                // the card rect inside a larger frame
                let rect = Rect::new(3, 2, cw, LARGE_H);
                let buf = draw(cw + 8, LARGE_H + 5, |f| render_wide_card(f, rect, &s, &cfg, false, t));
                for y in 0..LARGE_H + 5 {
                    for x in 0..cw + 8 {
                        let inside = (3..3 + cw).contains(&x) && (2..2 + LARGE_H).contains(&y);
                        assert!(inside || buf[(x, y)].symbol() == " ", "w={cw} t={t}: ({x},{y}) outside the card");
                    }
                }
                assert_eq!((buf[(3, 2)].symbol(), buf[(3 + cw - 1, 2 + LARGE_H - 1)].symbol()), ("╭", "╯"));
                let lit: Vec<(u16, u16)> = (0..LARGE_H + 5).flat_map(|y| (0..cw + 8).map(move |x| (x, y))).filter(|&p| buf[p].symbol().chars().any(is_braille)).collect();
                assert!(!lit.is_empty(), "w={cw} t={t}");
                assert!(lit.iter().all(|&(x, y)| (3..=2 + LARGE_SCENE_ROWS).contains(&y) && (4..3 + cw - 1).contains(&x)), "w={cw} t={t}: spirit and magic in the band: {lit:?}");
                assert!(lit.iter().map(|p| p.0).min().unwrap() <= 6, "w={cw}: body on the left");
                let r = rows(&buf);
                let row = |i: u16| info(&r[usize::from(2 + 1 + LARGE_SCENE_ROWS + i)].chars().skip(3).take(usize::from(cw)).collect::<String>());
                assert!(row(1).ends_with("opus") && row(1).width() == usize::from(cw - 2), "w={cw}: model word at the inner right edge: {:?}", row(1));
                assert_eq!(row(4).width(), usize::from(cw - 2), "w={cw}: the summary uses the extra columns");
                assert!(row(6).ends_with('…'), "w={cw}: {:?}", row(6));
            }
        }
    }

    // ---- subagents ----

    fn sub(id: &str, kind: &str, desc: &str, family: Option<&str>, running: bool) -> SubagentInfo {
        SubagentInfo { id: id.into(), kind: kind.into(), description: desc.into(), model_family: family.map(String::from), running, last_activity_secs: 1 }
    }

    fn with_subs(mut s: Session, running: usize, stale: usize) -> Session {
        let fams = ["sonnet", "opus", "haiku", "fable"];
        s.subagents = (0..running)
            .map(|i| sub(&format!("r{i}"), "Explore", &format!("task {i}"), Some(fams[i % 4]), true))
            .chain((0..stale).map(|i| sub(&format!("s{i}"), "Plan", "old", None, false)))
            .collect();
        s
    }

    #[test]
    fn standard_card_badge_shows_the_subagent_marker_only_while_running() {
        assert_eq!(SUBAGENT_GLYPH, "◇");
        let badge = |s: &Session, cfg: &Config| info(&card(s, cfg, false)[1 + SCENE_ROWS as usize + 1]);
        let two = with_subs(session("proj-a", Status::Running), 2, 1);
        assert_eq!(badge(&two, &Config::default()), " RUN  3h ◇×2");
        // last activity makes room for the marker first
        assert_eq!(badge(&two, &all_extras()), " RUN  3h ◇×2");
        let none = with_subs(session("proj-a", Status::Running), 0, 3);
        assert_eq!(badge(&none, &Config::default()), " RUN  3h");
        assert!(!card(&none, &all_extras(), false).join("").contains('◇'));
        // the other rows and the geometry are unchanged
        let rows = card(&two, &Config::default(), false);
        assert_eq!(rows.len(), usize::from(CARD_H));
        assert!(rows.iter().all(|r| r.width() == usize::from(CARD_W)), "{rows:?}");
        assert_eq!(info(&rows[1 + SCENE_ROWS as usize + 3]), "fixing the parser");
        // a Done session never shows it (its subagents are not running)
        let mut done = with_subs(session("proj-a", Status::Done), 0, 2);
        done.ended_at = done.started_at;
        assert!(!card(&done, &Config::default(), false).join("").contains('◇'));
        // narrow badge rows keep whole tokens only
        let now = SystemTime::now();
        let flat = |w: usize| -> String { text_lines(&two, &all_extras(), w, now)[1].spans.iter().map(|sp| sp.content.as_ref()).collect() };
        assert_eq!(flat(12), " RUN  3h ◇×2");
        assert_eq!(flat(11), " RUN  3h");
        assert_eq!(flat(9), " RUN  3h");
    }

    #[test]
    fn compact_card_shows_the_subagent_marker() {
        let line = |s: &Session, w: u16| rows(&draw(w, 1, |f| render_compact(f, f.area(), s, &Config::default(), false, 0)))[0].trim_end().to_string();
        let two = with_subs(session("proj-a", Status::Running), 2, 0);
        assert_eq!(line(&two, 30), "  proj-a  RUN  3h ◇2");
        assert_eq!(line(&with_subs(session("proj-a", Status::Running), 0, 2), 30), "  proj-a  RUN  3h");
        // too narrow: elapsed goes first, then the marker
        assert_eq!(line(&two, 17), "  proj-a  RUN  ◇2");
        assert_eq!(line(&two, 14), "  proj-a  RUN");
        for w in [0, 1, 2, 3, 5] {
            draw(w.max(1), 1, |f| render_compact(f, Rect::new(0, 0, w, 1), &two, &Config::default(), true, 0));
        }
    }

    #[test]
    fn large_card_lists_running_subagents_below_the_summary() {
        let cfg = large_cfg();
        let s = with_subs(opus("proj-a", Status::Running), 2, 1);
        let rows = large_info(&s, &cfg);
        assert_eq!(rows[..5].to_vec(), large_info(&opus("proj-a", Status::Running), &cfg)[..5].to_vec(), "priority rows intact");
        assert!(rows[5].starts_with("◇ Explore · task 0") && rows[5].ends_with("sonnet"), "{:?}", rows[5]);
        assert!(rows[6].starts_with("◇ Explore · task 1") && rows[6].ends_with("opus"), "{:?}", rows[6]);
        assert_eq!(rows[5].width(), usize::from(LARGE_W - 2));
        assert_eq!(rows[7], "", "the stale one is not listed");
        // no badge marker on the large card: the rows say it
        assert!(!rows[1].contains('◇'));
        // five running: three rows, the last ends with +2
        let five = with_subs(opus("proj-a", Status::Running), 5, 0);
        let rows = large_info(&five, &cfg);
        assert!(rows[5].starts_with("◇ Explore · task 0") && rows[6].starts_with("◇ Explore · task 1"));
        assert!(rows[7].starts_with("◇ Explore · task 2") && rows[7].ends_with("+2"), "{:?}", rows[7]);
        // a three-line summary leaves one row: one subagent and +N
        let mut long = five.clone();
        long.summary = Some("alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu nu xi omicron pi rho sigma tau".into());
        let rows = large_info(&long, &cfg);
        assert_eq!(rows[..7].to_vec(), large_info(&Session { subagents: Vec::new(), ..long.clone() }, &cfg)[..7].to_vec());
        assert!(rows[7].starts_with("◇ Explore · task 0") && rows[7].ends_with("+4"), "{:?}", rows[7]);
        // none running: no rows
        let idle = with_subs(opus("proj-a", Status::Running), 0, 4);
        assert!(large_info(&idle, &cfg)[5..].iter().all(String::is_empty));
        // empty description: just the kind; unknown model: no word
        let mut bare = opus("proj-a", Status::Running);
        bare.subagents = vec![sub("z", "general-purpose", "", None, true)];
        assert_eq!(large_info(&bare, &cfg)[5], "◇ general-purpose");
    }

    #[test]
    fn large_card_subagent_rows_cut_cjk_descriptions_inside_the_card() {
        let cfg = large_cfg();
        let mut s = opus("proj-a", Status::Running);
        s.subagents = vec![sub("k", "Explore", "한글 설명이 아주 길어서 카드 폭을 넘으면 잘려야 합니다", Some("haiku"), true)];
        let rows = large_rows(&s, &cfg);
        assert!(rows.iter().all(|r| r.width() == usize::from(LARGE_W)), "borders intact: {rows:?}");
        let row = &large_info(&s, &cfg)[5];
        assert!(row.starts_with("◇ Explore · 한글") && row.contains('…') && row.ends_with("haiku"), "{row:?}");
        assert_eq!(row.width(), usize::from(LARGE_W - 2));
        // a stretched card and a bigger area: nothing drawn outside the card
        let five = with_subs(opus("proj-a", Status::Running), 5, 0);
        let rect = Rect::new(3, 2, 40, LARGE_H);
        let buf = draw(48, LARGE_H + 5, |f| render_wide_card(f, rect, &five, &cfg, false, 0));
        for y in 0..LARGE_H + 5 {
            for x in 0..48 {
                assert!(rect.contains((x, y).into()) || buf[(x, y)].symbol() == " ", "({x},{y})");
            }
        }
        // a narrow width drops the model word and +N rather than overflowing
        for w in [0usize, 1, 3, 8, 12] {
            for l in subagent_lines(&five, w, 3) {
                assert!(l.width() <= w, "{w}: {l:?}");
            }
        }
        for (w, h) in [(0, 0), (1, 1), (31, 14), (32, 13), (5, 2)] {
            draw(w, h, |f| render_card(f, f.area(), &five, &cfg, true, 0));
            draw(w, h, |f| render_card(f, f.area(), &five, &Config::default(), true, 0));
        }
    }

    #[test]
    fn running_subagents_add_coloured_dots_inside_the_spirit_band() {
        // sonnet = lightning, opus = fire, haiku = poison, fable = water (defaults)
        let cfg_for = |layout| Config { card_layout: layout, ..all_extras() };
        for (layout, (w, h), band_rows) in [(CardLayout::Standard, (CARD_W, CARD_H), SCENE_ROWS), (CardLayout::Large, (LARGE_W, LARGE_H), LARGE_SCENE_ROWS)] {
            let cfg = cfg_for(layout);
            for status in [Status::Idle, Status::NeedsInput, Status::Running] {
                for t in [0, 17, 44] {
                    let plain = session("proj-a", status);
                    let busy = with_subs(plain.clone(), 5, 2);
                    let (a, b) = (draw(w, h, |f| render_card(f, f.area(), &plain, &cfg, false, t)), draw(w, h, |f| render_card(f, f.area(), &busy, &cfg, false, t)));
                    let diff: Vec<(u16, u16)> = (1..=band_rows).flat_map(|y| (1..w - 1).map(move |x| (x, y))).filter(|&p| a[p] != b[p]).collect();
                    let beads: Vec<u16> = diff.iter().filter(|&&p| matches!(b[p].symbol(), "⠿" | "⣶")).map(|p| p.0).collect();
                    assert_eq!(beads.len(), MAX_DOTS, "{layout:?} {status:?} t={t}: five beads for five running: {diff:?}");
                    assert!(beads.windows(2).all(|w| w[1] == w[0] + 2 && b[(w[0] + 1, band_rows)].symbol() == " "), "{layout:?}: {beads:?}");
                    assert!(!diff.is_empty() && diff.iter().all(|&(_, y)| y == band_rows), "{layout:?} {status:?}: bottom band row: {diff:?}");
                    // the border is untouched
                    for x in 0..w {
                        assert_eq!(a[(x, 0)], b[(x, 0)]);
                        assert_eq!(a[(x, h - 1)], b[(x, h - 1)]);
                    }
                    for y in 0..h {
                        assert_eq!(a[(0, y)], b[(0, y)]);
                        assert_eq!(a[(w - 1, y)], b[(w - 1, y)]);
                    }
                }
            }
            // a stale-only or finished session draws exactly the plain spirit
            let stale = with_subs(session("proj-a", Status::Idle), 0, 3);
            let plain = session("proj-a", Status::Idle);
            assert_eq!(draw(w, h, |f| render_card(f, f.area(), &stale, &cfg, false, 3)), draw(w, h, |f| render_card(f, f.area(), &plain, &cfg, false, 3)));
            let done = with_subs(session("proj-a", Status::Done), 2, 0);
            let done_plain = session("proj-a", Status::Done);
            let band = |s: &Session| rows(&draw(w, h, |f| render_card(f, f.area(), s, &cfg, false, 3)))[1..=usize::from(band_rows)].to_vec();
            assert_eq!(band(&done), band(&done_plain), "{layout:?}: no dots when done");
        }
        // the dot colour is the subagent model's element
        let cfg = Config::default();
        let s = Session { subagents: vec![sub("w", "Explore", "x", Some("fable"), true)], ..session("proj-a", Status::Idle) };
        let buf = draw(CARD_W, CARD_H, |f| render_card(f, f.area(), &s, &cfg, false, 0));
        let plain = draw(CARD_W, CARD_H, |f| render_card(f, f.area(), &session("proj-a", Status::Idle), &cfg, false, 0));
        let cell = (1..CARD_W - 1).map(|x| (x, SCENE_ROWS)).find(|&p| buf[p] != plain[p]).expect("a dot");
        let Color::Rgb(r, g, b) = buf[cell].fg else { panic!("rgb") };
        assert!(b > r && b > g, "fable = water = blue: {:?}", buf[cell].fg);
        // unknown model: the parent's element (fire by default)
        let s = Session { subagents: vec![sub("u", "Explore", "x", None, true)], ..session("proj-a", Status::Idle) };
        let buf = draw(CARD_W, CARD_H, |f| render_card(f, f.area(), &s, &cfg, false, 0));
        let Color::Rgb(r, g, b) = buf[cell].fg else { panic!("rgb") };
        assert!(r > g && r > b, "parent's fire: {:?}", buf[cell].fg);
    }

    /// The user's config: sonnet = fire, opus = water, fable/haiku = poison.
    fn user_cfg(layout: CardLayout) -> Config {
        let mut cfg = Config { card_layout: layout, ..Config::default() };
        cfg.model_themes = [("sonnet", Theme::Fire), ("opus", Theme::Water), ("fable", Theme::Poison), ("haiku", Theme::Poison)].into_iter().map(|(k, v)| (k.to_string(), v)).collect();
        cfg
    }

    /// Cells of the bottom band row that differ from the card without subagents.
    fn dot_cells(s: &Session, cfg: &Config, t: u64) -> Vec<(u16, Color)> {
        let (w, h) = card_size(cfg.card_layout);
        let band_rows = if cfg.card_layout == CardLayout::Large { LARGE_SCENE_ROWS } else { SCENE_ROWS };
        let with = draw(w, h, |f| render_card(f, f.area(), s, cfg, false, t));
        let plain = draw(w, h, |f| render_card(f, f.area(), &Session { subagents: Vec::new(), ..s.clone() }, cfg, false, t));
        (1..w - 1).filter(|&x| with[(x, band_rows)] != plain[(x, band_rows)] && with[(x, band_rows)].symbol() != " ").map(|x| (x, with[(x, band_rows)].fg)).collect()
    }

    #[test]
    fn subagent_dots_take_the_users_model_theme_not_the_parents_water() {
        for layout in [CardLayout::Standard, CardLayout::Large] {
            let cfg = user_cfg(layout);
            for parent_model in ["claude-opus-4-5", "claude-sonnet-5-5"] {
                for status in [Status::Running, Status::NeedsInput, Status::Idle] {
                    for t in [0, 7, 21, 40, 63] {
                        let mut s = Session { model: Some(parent_model.into()), ..session("ui refactoring", status) };
                        s.subagents = vec![sub("a", "general-purpose", "x", Some("sonnet"), true), sub("b", "Explore", "y", Some("sonnet"), true)];
                        let cells = dot_cells(&s, &cfg, t);
                        assert_eq!(cells.len(), 2, "{layout:?} {parent_model} {status:?} t={t}: two beads: {cells:?}");
                        for (x, fg) in &cells {
                            let Color::Rgb(r, g, b) = *fg else { panic!("rgb") };
                            assert!(r > g && g > b, "{layout:?} {parent_model} {status:?} t={t}: sonnet = fire at x={x}: {fg:?}");
                        }
                        // a subagent on opus is water here: blue
                        s.subagents = vec![sub("c", "Plan", "z", Some("opus"), true)];
                        for (x, fg) in dot_cells(&s, &cfg, t) {
                            let Color::Rgb(r, g, b) = fg else { panic!("rgb") };
                            assert!(b > r && b > g, "{layout:?} {parent_model} t={t}: opus = water at x={x}: {fg:?}");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn two_running_subagents_draw_two_separate_beads_unlike_the_body() {
        for layout in [CardLayout::Standard, CardLayout::Large] {
            let cfg = user_cfg(layout);
            let band_rows = if layout == CardLayout::Large { LARGE_SCENE_ROWS } else { SCENE_ROWS };
            let (w, h) = card_size(layout);
            for t in [0, 9, 33] {
                // parent on opus (water body), subagents on sonnet (fire) and haiku (poison)
                let mut s = Session { model: Some("claude-opus-4-5".into()), ..session("proj-a", Status::Idle) };
                s.subagents = vec![sub("a", "Explore", "x", Some("sonnet"), true), sub("b", "Explore", "y", Some("haiku"), true)];
                let buf = draw(w, h, |f| render_card(f, f.area(), &s, &cfg, false, t));
                let cells = dot_cells(&s, &cfg, t);
                let xs: Vec<u16> = cells.iter().map(|c| c.0).collect();
                assert_eq!(xs.len(), 2, "{layout:?} t={t}: two beads, one cell each: {xs:?}");
                assert_eq!(xs[1], xs[0] + 2, "{layout:?}: {xs:?}");
                assert_eq!(buf[(xs[0] + 1, band_rows)].symbol(), " ", "{layout:?}: an empty column between the beads");
                // body colours: every lit band cell left of the first bead
                let body: Vec<Color> = (1..=band_rows).flat_map(|y| (1..xs[0]).map(move |x| (x, y))).filter(|&p| buf[p].symbol() != " ").map(|p| buf[p].fg).collect();
                assert!(!body.is_empty());
                for (x, fg) in &cells {
                    assert!(!body.contains(fg), "{layout:?}: bead at {x} differs from the body: {fg:?}");
                }
            }
        }
    }

    /// Look of the standard and large cards with 1, 2, 4 and 5 running
    /// subagents (sonnet = fire, opus = water, haiku/fable = poison), as
    /// plain text and with ANSI colours, plus the colour of each bead.
    /// Run with `cargo test dump_subagent_cards -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn dump_subagent_cards() {
        let fams = ["sonnet", "opus", "haiku", "sonnet", "fable"];
        for layout in [CardLayout::Standard, CardLayout::Large] {
            let cfg = user_cfg(layout);
            let (w, h) = card_size(layout);
            let band = if layout == CardLayout::Large { LARGE_SCENE_ROWS } else { SCENE_ROWS };
            for n in [1, 2, 4, 5] {
                let mut s = Session { model: Some("claude-opus-4-5".into()), ctx_percent: Some(35.0), ..session("ui refactoring", Status::Running) };
                s.subagents = (0..n).map(|i| sub(&format!("a{i}"), "Explore", &format!("task {i}"), Some(fams[i]), true)).collect();
                let buf = draw(w, h, |f| render_card(f, f.area(), &s, &cfg, false, 30));
                println!("{layout:?}, {n} running:");
                for r in rows(&buf) {
                    println!("{r}");
                }
                for y in 0..h {
                    let line: String = (0..w)
                        .map(|x| match buf[(x, y)].fg {
                            Color::Rgb(r, g, b) => format!("\x1b[38;2;{r};{g};{b}m{}\x1b[0m", buf[(x, y)].symbol()),
                            _ => buf[(x, y)].symbol().to_string(),
                        })
                        .collect();
                    println!("{line}");
                }
                let beads: Vec<String> = (1..w - 1).filter(|&x| matches!(buf[(x, band)].symbol(), "⠿" | "⣶")).map(|x| format!("x={x} {:?}", buf[(x, band)].fg)).collect();
                println!("beads: {beads:?}\n");
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
