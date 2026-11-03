//! Character-card tiles: thin rounded border, Braille mascot, name, status badge, extras.
//!
//! Minimum sizes (borders included), used by the app to compute columns:
//! - view A (horizontal card): 26×6 — 4×2 mascot top-left, ≥19 text columns right.
//! - view B (vertical card): 24×9 — 6×3 mascot on top, ≥4 text rows below.
//!
//! Text rows in priority order: name, badge line, branch, PR, summary (each
//! extra only when enabled and known). View A always shows at most 4 rows, so
//! PR wins over summary there; view B shows as many as its height allows.
//!
//! Smaller areas degrade to a single name line; empty areas draw nothing.

use std::time::SystemTime;

use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use ratatui::Frame;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::config::Config;
use crate::model::{PrState, Session, Status};
use crate::ui::mascot::{sprite, SpriteSize, TileFx};
use crate::ui::style;

/// Tile layout: A = mascot left / text right, B = mascot top / text below.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewKind {
    A,
    B,
}

impl ViewKind {
    pub fn sprite_size(self) -> SpriteSize {
        match self {
            ViewKind::A => SpriteSize::Small,
            ViewKind::B => SpriteSize::Large,
        }
    }
}

/// Minimum (width, height) of a full tile in `view`, borders included.
///
/// View A (26×6) shows at most 4 text rows: name, badge line (status +
/// elapsed + last activity, dropping last activity then elapsed when too
/// narrow), branch, and ONE of PR / summary — PR first, summary only when the
/// PR row is absent (disabled or no PR). Rows of disabled extras collapse.
/// View B (24×9) has 4 text rows at minimum and uses the same priority order;
/// taller B tiles show every enabled row.
pub fn tile_size(view: ViewKind) -> (u16, u16) {
    match view {
        ViewKind::A => (26, 6),
        ViewKind::B => (24, 9),
    }
}

/// Compact duration: `45s`, `12m`, `3h`, `2d` (rounded down).
pub fn fmt_duration(secs: u64) -> String {
    match secs {
        0..=59 => format!("{secs}s"),
        60..=3599 => format!("{}m", secs / 60),
        3600..=86_399 => format!("{}h", secs / 3600),
        _ => format!("{}d", secs / 86_400),
    }
}

/// Cuts `s` to at most `max` display columns, ending in `…` when cut.
/// Wide (CJK) chars are never split; control chars become spaces.
pub fn truncate_to_width(s: &str, max: usize) -> String {
    let clean: Vec<(char, usize)> = s
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .map(|c| (c, c.width().unwrap_or(0)))
        .collect();
    if clean.iter().map(|(_, w)| w).sum::<usize>() <= max {
        return clean.into_iter().map(|(c, _)| c).collect();
    }
    if max == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut used = 0;
    for (c, w) in clean {
        if used + w > max - 1 {
            break;
        }
        out.push(c);
        used += w;
    }
    out.push('…');
    out
}

/// `area` moved by `shake` (cols, rows), kept fully inside `parent`
/// (cut down to `parent` if larger). Pure i32 math: no u16 under/overflow.
pub fn shake_rect(area: Rect, shake: (i8, i8), parent: Rect) -> Rect {
    let w = area.width.min(parent.width);
    let h = area.height.min(parent.height);
    let shift = |pos: u16, d: i8, len: u16, p_pos: u16, p_len: u16| -> u16 {
        let lo = i32::from(p_pos);
        let hi = (i32::from(p_pos) + i32::from(p_len) - i32::from(len)).max(lo);
        (i32::from(pos) + i32::from(d)).clamp(lo, hi) as u16
    };
    Rect::new(
        shift(area.x, shake.0, w, parent.x, parent.width),
        shift(area.y, shake.1, h, parent.y, parent.height),
        w,
        h,
    )
}

/// Draws one session tile with its live mascot animation.
#[allow(clippy::too_many_arguments)]
pub fn render_tile(
    f: &mut Frame,
    area: Rect,
    s: &Session,
    cfg: &Config,
    selected: bool,
    tick: u64,
    fx: TileFx,
    view: ViewKind,
) {
    let frame = sprite(s.status, view.sprite_size(), tick);
    render_tile_with_sprite(f, area, s, cfg, selected, tick, fx, view, frame);
}

/// Like [`render_tile`] but with a caller-chosen mascot frame, e.g.
/// `sprite_oneshot_done` while a freshly finished session celebrates.
#[allow(clippy::too_many_arguments)]
pub fn render_tile_with_sprite(
    f: &mut Frame,
    area: Rect,
    s: &Session,
    cfg: &Config,
    selected: bool,
    tick: u64,
    fx: TileFx,
    view: ViewKind,
    sprite_lines: Vec<Line<'static>>,
) {
    let parent = f.area();
    let base = area.intersection(parent);
    if base.is_empty() {
        return;
    }
    let (min_w, min_h) = tile_size(view);
    if base.width < min_w || base.height < min_h {
        render_name_only(f, base, s, selected);
        return;
    }
    let rect = shake_rect(base, fx.shake, parent);
    let done = s.status == Status::Done;
    let mut block = style::window_block(style::status_color(s.status), selected, fx.flash);
    if done && !selected && !fx.flash {
        block = block.border_style(Style::new().fg(style::DONE).add_modifier(Modifier::DIM));
    }
    let inner = block.inner(rect);
    f.render_widget(Clear, rect);
    f.render_widget(block, rect);

    let (sw, sh) = view.sprite_size().cells();
    let (sprite_area, text_area, align) = match view {
        ViewKind::A => {
            // Top-aligned with the name row.
            let sprite_area = Rect::new(inner.x, inner.y, sw.min(inner.width), sh.min(inner.height));
            let gap = sw + 1;
            let text = Rect::new(inner.x + gap, inner.y, inner.width.saturating_sub(gap), inner.height);
            (sprite_area, text, Alignment::Left)
        }
        ViewKind::B => {
            let sprite_area = Rect::new(inner.x + inner.width.saturating_sub(sw) / 2, inner.y, sw.min(inner.width), sh.min(inner.height));
            let text = Rect::new(inner.x, inner.y + sh, inner.width, inner.height.saturating_sub(sh));
            (sprite_area, text, Alignment::Center)
        }
    };
    f.render_widget(Paragraph::new(sprite_lines), sprite_area);
    let mut lines = text_lines(s, cfg, text_area.width as usize, SystemTime::now());
    let max_rows = match view {
        ViewKind::A => A_TEXT_ROWS,
        ViewKind::B => text_area.height as usize,
    };
    lines.truncate(max_rows);
    let mut para = Paragraph::new(lines).alignment(align);
    if done {
        para = para.style(Style::new().add_modifier(Modifier::DIM));
    }
    f.render_widget(para, text_area);

    if selected && style::cursor_visible(tick) && !text_area.is_empty() {
        // The cursor replaces the left border cell on the name row.
        f.buffer_mut()[(rect.x, text_area.y)]
            .set_symbol(style::CURSOR)
            .set_style(Style::new().fg(style::SELECTED).add_modifier(Modifier::BOLD));
    }
}

fn render_name_only(f: &mut Frame, area: Rect, s: &Session, selected: bool) {
    let mut st = Style::new().fg(style::status_color(s.status)).add_modifier(Modifier::BOLD);
    if selected {
        st = st.fg(style::SELECTED);
    }
    let name = truncate_to_width(&s.name, area.width as usize);
    f.render_widget(Paragraph::new(Line::styled(name, st)), Rect { height: 1, ..area });
}

fn since(t: Option<SystemTime>, now: SystemTime) -> Option<String> {
    t.map(|t| fmt_duration(now.duration_since(t).map(|d| d.as_secs()).unwrap_or(0)))
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

/// Text block: name, status badge (+ elapsed / last activity), then the
/// enabled extras: branch (`~` when guessed), PR, one-line summary.
/// Text rows shown by view A (see [`tile_size`]).
const A_TEXT_ROWS: usize = 4;

/// Text lines in priority order (callers cut the tail to their row budget).
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
    // A finished session's elapsed time stops at its end.
    let elapsed_end = match (s.status, s.ended_at) {
        (Status::Done, Some(end)) => end,
        _ => now,
    };
    let elapsed = ex.elapsed.then(|| since(s.started_at, elapsed_end)).flatten().map(|d| format!(" {d}"));
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
    use crate::model::{Pr, PrState, Status};
    use crate::ui::style;
    use crate::ui::test_util::{draw, rows};
    use std::path::PathBuf;
    use std::time::{Duration, SystemTime};

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

    /// A Braille mascot cell with at least one dot.
    fn is_pixel(c: char) -> bool {
        ('\u{2801}'..='\u{28ff}').contains(&c)
    }

    fn tile(w: u16, h: u16, s: &Session, cfg: &Config, selected: bool, tick: u64, view: ViewKind) -> Vec<String> {
        let buf = draw(w, h, |f| render_tile(f, f.area(), s, cfg, selected, tick, TileFx::default(), view));
        rows(&buf)
    }

    #[test]
    fn tile_view_a_shows_name_and_status() {
        let rows = tile(40, 8, &session("proj-a", Status::Running), &Config::default(), false, 0, ViewKind::A);
        let text = rows.join("\n");
        assert!(rows[0].starts_with('╭') && rows[0].ends_with('╮'), "thin rounded border: {text}");
        assert!(rows[7].starts_with('╰') && rows[7].ends_with('╯'));
        assert!(rows[1..7].iter().all(|r| r.starts_with('│') && r.ends_with('│')), "{text}");
        let name_row = rows.iter().position(|r| r.contains("proj-a")).expect(&text);
        assert_eq!(name_row, 1, "name on the first inner row");
        let name_col = rows[name_row].chars().position(|c| c == 'p').unwrap();
        assert!(rows.iter().any(|r| r.contains("RUN")), "{text}");
        // The mascot sits left of the text column.
        let sprite_cols: Vec<usize> = rows[1..rows.len() - 1]
            .iter()
            .flat_map(|r| r.chars().enumerate().filter(|(i, c)| *i > 0 && is_pixel(*c)).map(|(i, _)| i))
            .collect();
        assert!(!sprite_cols.is_empty(), "{text}");
        assert!(sprite_cols.iter().all(|c| *c < name_col), "{text}");
        // Default extras: branch, elapsed and summary shown; PR hidden.
        assert!(text.contains("feat/login") && text.contains("3h") && text.contains("fixing the parser"), "{text}");
        assert!(!text.contains("#42"), "{text}");
    }

    #[test]
    fn tile_view_b_shows_mascot_above_text() {
        let rows = tile(30, 14, &session("proj-b", Status::NeedsInput), &Config::default(), false, 0, ViewKind::B);
        let text = rows.join("\n");
        let name_row = rows.iter().position(|r| r.contains("proj-b")).expect(&text);
        let sprite_rows: Vec<usize> = rows
            .iter()
            .enumerate()
            .filter(|(i, r)| *i > 0 && *i < rows.len() - 1 && r.chars().any(is_pixel))
            .map(|(i, _)| i)
            .collect();
        assert!(!sprite_rows.is_empty(), "{text}");
        assert!(sprite_rows.iter().all(|r| *r < name_row), "{text}");
        assert!(rows[name_row + 1].contains("INPUT"), "{text}");
    }

    #[test]
    fn truncate_to_width_respects_display_width() {
        assert_eq!(truncate_to_width("proj-a", 10), "proj-a");
        assert_eq!(truncate_to_width("proj-a", 6), "proj-a");
        assert_eq!(truncate_to_width("proj-a", 5), "proj…");
        assert_eq!(truncate_to_width("proj-a", 1), "…");
        assert_eq!(truncate_to_width("proj-a", 0), "");
        assert_eq!(truncate_to_width("프로젝트", 8), "프로젝트");
        assert_eq!(truncate_to_width("프로젝트", 7), "프로젝…");
        assert_eq!(truncate_to_width("프로젝트", 6), "프로…"); // never splits a wide char
        assert_eq!(truncate_to_width("프로젝트", 2), "…");
        assert_eq!(truncate_to_width("a\nb\tc", 10), "a b c", "control chars become spaces");
    }

    #[test]
    fn tile_truncates_wide_korean_name_without_panic() {
        let s = session("프로젝트-아주-아주-긴-이름-입니다-정말로", Status::Running);
        for w in 26..44 {
            let rows = tile(w, 6, &s, &Config::default(), false, 0, ViewKind::A);
            let row = &rows[1];
            assert!(row.contains("프로젝트"), "{row}");
            assert!(row.contains('…'), "{row}");
            assert!(row.ends_with('│'), "border intact: {row:?}");
        }
        for w in 24..36 {
            let rows = tile(w, 9, &s, &Config::default(), true, 0, ViewKind::B);
            assert!(rows.iter().any(|r| r.contains("프로") && r.contains('…')));
        }
    }

    #[test]
    fn tile_tiny_area_does_not_panic() {
        let s = session("proj-a", Status::Idle);
        for view in [ViewKind::A, ViewKind::B] {
            for (w, h) in [(20, 6), (5, 2), (1, 1), (0, 0), (2, 40), (23, 11), (25, 5), (60, 5)] {
                let rows = tile(w, h, &s, &Config::default(), true, 0, view);
                if w == 0 || h == 0 {
                    continue;
                }
                // Too small for a card: just the name line, no border.
                assert!(!rows.join("").contains('╭'), "{view:?} {w}x{h}");
                assert_eq!(rows[0], truncate_to_width("proj-a", w as usize) + &" ".repeat(w as usize - truncate_to_width("proj-a", w as usize).chars().count()));
            }
        }
        let rows = tile(5, 2, &s, &Config::default(), false, 0, ViewKind::A);
        assert_eq!(rows[0], "proj…");
        assert_eq!(rows[1], "     ");
        let rows = tile(1, 1, &s, &Config::default(), false, 0, ViewKind::A);
        assert_eq!(rows[0], "…");
    }

    #[test]
    fn tile_hides_disabled_extras() {
        let mut s = session("proj-a", Status::Running);
        s.summary = Some("summary-text".into());
        let shown = tile(30, 16, &s, &all_extras(), false, 0, ViewKind::B).join("\n");
        for want in ["feat/login", "summary-text", "#42 approved", "3h", "2m"] {
            assert!(shown.contains(want), "missing {want}: {shown}");
        }
        let hidden = tile(44, 10, &s, &no_extras(), false, 0, ViewKind::A).join("\n");
        assert!(hidden.contains("proj-a") && hidden.contains("RUN"), "{hidden}");
        for gone in ["feat/login", "summary-text", "#42", "3h", "2m"] {
            assert!(!hidden.contains(gone), "{gone} should be hidden: {hidden}");
        }
    }

    /// Text column of a view A row: after the border, 4 mascot cells and the gap.
    fn a_text(row: &str) -> String {
        row.chars().skip(6).collect::<String>().trim_end_matches('│').to_string()
    }

    #[test]
    fn view_a_min_size_shows_name_badge_branch_and_pr_over_summary() {
        let s = session("proj-a", Status::Running);
        let (w, h) = tile_size(ViewKind::A);
        let rows = tile(w, h, &s, &all_extras(), false, 0, ViewKind::A);
        let text: Vec<String> = rows[1..5].iter().map(|r| a_text(r)).collect();
        assert_eq!(
            text,
            vec![
                format!("{:<19}", "proj-a"),
                format!("{:<19}", " RUN  3h ·2m ago"),
                format!("{:<19}", "⎇ feat/login"),
                format!("{:<19}", "#42 approved"),
            ]
        );
        // Summary takes the 4th row when PR is off; taller A tiles still show 4 rows.
        let mut cfg = all_extras();
        cfg.extras.pr = false;
        let rows = tile(w, h + 3, &s, &cfg, false, 0, ViewKind::A);
        let all = rows.join("\n");
        assert!(all.contains("fixing the parser") && !all.contains("#42"), "{all}");
        let rows = tile(w, h + 3, &s, &all_extras(), false, 0, ViewKind::A).join("\n");
        assert!(rows.contains("#42 approved") && !rows.contains("fixing the parser"), "{rows}");
    }

    #[test]
    fn badge_line_drops_last_activity_then_elapsed_instead_of_cutting() {
        let now = SystemTime::now();
        let mut s = session("proj-a", Status::NeedsInput);
        s.started_at = Some(now - Duration::from_secs(400 * 86_400 + 5));
        // " INPUT " (7) + " 400d" (5) + " ·2m ago" (8) = 20 > 19 → last activity dropped.
        let rows = tile(26, 6, &s, &all_extras(), false, 0, ViewKind::A);
        assert_eq!(a_text(&rows[2]), format!("{:<19}", " INPUT  400d"));
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
    fn tile_shows_dim_pr_question_mark_when_lookup_unavailable() {
        let mut s = session("proj-a", Status::Running);
        s.pr = None;
        s.pr_unavailable = true;
        let buf = draw(30, 16, |f| render_tile(f, f.area(), &s, &all_extras(), false, 0, TileFx::default(), ViewKind::B));
        let text = rows(&buf);
        let y = text.iter().position(|r| r.contains("PR ?")).expect(&text.join("\n"));
        let x = text[y].find("PR ?").map(|b| text[y][..b].chars().count()).unwrap() as u16;
        assert_eq!(buf[(x, y as u16)].fg, style::TEXT_DIM);
        // no PR (lookup said none): nothing
        s.pr_unavailable = false;
        let none = tile(30, 16, &s, &all_extras(), false, 0, ViewKind::B).join("\n");
        assert!(!none.contains("PR ?") && !none.contains('#'), "{none}");
        // PR extra off: nothing either
        s.pr_unavailable = true;
        let off = tile(30, 16, &s, &Config::default(), false, 0, ViewKind::B).join("\n");
        assert!(!off.contains("PR ?"), "{off}");
    }

    #[test]
    fn done_tile_elapsed_freezes_at_end_time() {
        let now = SystemTime::now();
        let mut s = session("proj-a", Status::Done);
        s.started_at = Some(now - Duration::from_secs(3 * 3600 + 10));
        s.ended_at = Some(now - Duration::from_secs(2 * 3600));
        let badge = |s: &Session, at: SystemTime| -> String {
            text_lines(s, &Config::default(), 40, at)[1].spans.iter().map(|sp| sp.content.as_ref()).collect()
        };
        assert_eq!(badge(&s, now), " DONE  1h");
        assert_eq!(badge(&s, now + Duration::from_secs(5 * 3600)), " DONE  1h", "does not keep growing");
        // a live session still counts up to now
        s.status = Status::Running;
        assert_eq!(badge(&s, now), " RUN  3h");
    }

    #[test]
    fn tile_marks_guessed_branch_with_tilde() {
        let mut s = session("proj-a", Status::Running);
        let exact = tile(40, 8, &s, &Config::default(), false, 0, ViewKind::A).join("\n");
        assert!(exact.contains("feat/login") && !exact.contains("~feat/login"), "{exact}");
        s.branch_guessed = true;
        let guessed = tile(40, 8, &s, &Config::default(), false, 0, ViewKind::A).join("\n");
        assert!(guessed.contains("~feat/login"), "{guessed}");
    }

    #[test]
    fn selected_tile_shows_blinking_cursor_arrow() {
        let s = session("proj-a", Status::Running);
        for view in [ViewKind::A, ViewKind::B] {
            let (w, h) = tile_size(view);
            let on = tile(w, h, &s, &Config::default(), true, 0, view).join("\n");
            let off = tile(w, h, &s, &Config::default(), true, 6, view).join("\n");
            let on_again = tile(w, h, &s, &Config::default(), true, 12, view).join("\n");
            let unselected = tile(w, h, &s, &Config::default(), false, 0, view).join("\n");
            assert!(on.contains('▶'), "{view:?}: {on}");
            assert!(!off.contains('▶'), "{view:?}: {off}");
            assert!(on_again.contains('▶'));
            assert!(!unselected.contains('▶'));
            // The cursor takes the thin left border cell of the name row.
            let name_row = |t: &str| t.lines().find(|r| r.contains("proj-a")).unwrap().to_string();
            assert!(name_row(&on).starts_with('▶'), "{view:?}: {on}");
            assert!(name_row(&off).starts_with('│'), "{view:?}: {off}");
        }
    }

    #[test]
    fn selected_tile_border_is_yellow() {
        use crate::ui::style;
        let s = session("proj-a", Status::Running);
        let buf = draw(30, 6, |f| render_tile(f, f.area(), &s, &Config::default(), true, 6, TileFx::default(), ViewKind::A));
        assert_eq!(buf[(0, 0)].fg, style::SELECTED);
        let buf = draw(30, 6, |f| render_tile(f, f.area(), &s, &Config::default(), false, 6, TileFx::default(), ViewKind::A));
        assert_eq!(buf[(0, 0)].fg, style::RUNNING);
        let flash = TileFx { shake: (0, 0), flash: true };
        let buf = draw(30, 6, |f| render_tile(f, f.area(), &s, &Config::default(), false, 6, flash, ViewKind::A));
        assert_eq!(buf[(0, 0)].fg, style::FLASH);
    }

    #[test]
    fn done_tile_is_dimmed() {
        use ratatui::style::Modifier;
        let s = session("proj-a", Status::Done);
        let buf = draw(30, 6, |f| render_tile(f, f.area(), &s, &Config::default(), false, 0, TileFx::default(), ViewKind::A));
        assert!(buf[(0, 0)].modifier.contains(Modifier::DIM));
        let row1 = rows(&buf)[1].clone();
        let col = row1.chars().position(|c| c == 'p').unwrap() as u16;
        assert!(buf[(col, 1)].modifier.contains(Modifier::DIM));
    }

    #[test]
    fn fmt_duration_seconds_minutes_hours_days() {
        assert_eq!(fmt_duration(0), "0s");
        assert_eq!(fmt_duration(45), "45s");
        assert_eq!(fmt_duration(59), "59s");
        assert_eq!(fmt_duration(60), "1m");
        assert_eq!(fmt_duration(12 * 60 + 59), "12m");
        assert_eq!(fmt_duration(3599), "59m");
        assert_eq!(fmt_duration(3600), "1h");
        assert_eq!(fmt_duration(3 * 3600 + 1800), "3h");
        assert_eq!(fmt_duration(86_399), "23h");
        assert_eq!(fmt_duration(2 * 86_400), "2d");
        assert_eq!(fmt_duration(u64::MAX), format!("{}d", u64::MAX / 86_400));
    }

    #[test]
    fn tile_size_is_documented_minimum() {
        assert_eq!(tile_size(ViewKind::A), (26, 6));
        assert_eq!(tile_size(ViewKind::B), (24, 9));
    }

    #[test]
    fn shake_stays_inside_parent_area() {
        let parent = Rect::new(0, 0, 100, 40);
        let area = Rect::new(10, 5, 30, 6);
        assert_eq!(shake_rect(area, (0, 0), parent), area);
        assert_eq!(shake_rect(area, (2, -1), parent), Rect::new(12, 4, 30, 6));
        assert_eq!(shake_rect(area, (-2, 1), parent), Rect::new(8, 6, 30, 6));
        // Near the edges the offset is clamped, never outside the parent.
        let edge = Rect::new(0, 0, 30, 6);
        assert_eq!(shake_rect(edge, (-2, -1), parent), edge);
        let far = Rect::new(70, 34, 30, 6);
        assert_eq!(shake_rect(far, (2, 1), parent), far);
        for dx in [-128i8, -3, 0, 3, 127] {
            for dy in [-128i8, -1, 0, 1, 127] {
                let r = shake_rect(area, (dx, dy), parent);
                assert_eq!(r.intersection(parent), r);
                assert_eq!((r.width, r.height), (30, 6));
            }
        }
        // An area bigger than the parent is cut down to it.
        let big = shake_rect(Rect::new(0, 0, 200, 80), (5, 5), parent);
        assert_eq!(big, parent);
    }

    #[test]
    fn render_tile_with_extreme_shake_does_not_underflow() {
        let s = session("proj-a", Status::NeedsInput);
        for shake in [(i8::MIN, i8::MIN), (i8::MAX, i8::MAX), (i8::MIN, i8::MAX), (-1, -1)] {
            let fx = TileFx { shake, flash: true };
            for view in [ViewKind::A, ViewKind::B] {
                draw(40, 14, |f| render_tile(f, Rect::new(0, 0, 40, 14), &s, &Config::default(), true, 3, fx, view));
                draw(40, 14, |f| render_tile(f, Rect::new(39, 13, 30, 12), &s, &Config::default(), true, 3, fx, view));
                draw(40, 14, |f| render_tile(f, Rect::new(u16::MAX - 5, u16::MAX - 5, 30, 12), &s, &Config::default(), true, 3, fx, view));
            }
        }
        // The shifted card is fully drawn inside the frame.
        let fx = TileFx { shake: (2, 1), flash: false };
        let buf = draw(34, 8, |f| render_tile(f, Rect::new(0, 0, 30, 6), &s, &Config::default(), false, 0, fx, ViewKind::A));
        let rows = rows(&buf);
        assert!(rows[1].chars().nth(2) == Some('╭'), "{}", rows.join("\n"));
    }

    /// `text` in 24-bit ANSI foreground `color` (other colors: plain).
    fn ansi(text: &str, color: Option<ratatui::style::Color>) -> String {
        match color {
            Some(ratatui::style::Color::Rgb(r, g, b)) => format!("\x1b[38;2;{r};{g};{b}m{text}\x1b[0m"),
            _ => text.to_string(),
        }
    }

    /// Colored gallery for a real terminal: every status in both sizes over
    /// a few ticks, the Done one-shot, and one tile per view.
    /// Run with `cargo test dump_color_gallery -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn dump_color_gallery() {
        use crate::ui::mascot::{sprite, sprite_oneshot_done, SpriteSize};
        let colored = |lines: &[Line]| -> Vec<String> {
            lines.iter().map(|l| l.spans.iter().map(|s| ansi(&s.content, s.style.fg)).collect()).collect()
        };
        let row = |frames: Vec<Vec<String>>| {
            for r in 0..frames[0].len() {
                println!("  {}", frames.iter().map(|f| f[r].clone()).collect::<Vec<_>>().join("   "));
            }
        };
        let statuses = [Status::Running, Status::NeedsInput, Status::Idle, Status::Done, Status::Unknown];
        for size in [SpriteSize::Small, SpriteSize::Large] {
            for st in statuses {
                let len = crate::ui::mascot::loop_len(st);
                let ticks: Vec<u64> = (0..len).step_by((len as usize / 6).max(1)).take(6).collect();
                println!("{st:?} {size:?} ticks {ticks:?}");
                row(ticks.iter().map(|t| colored(&sprite(st, size, *t))).collect());
            }
            println!("Done one-shot {size:?} ticks 0,2,4,6,8,12,15");
            row([0, 2, 4, 6, 8, 12, 15].iter().map(|t| colored(&sprite_oneshot_done(size, *t))).collect());
        }
        for view in [ViewKind::A, ViewKind::B] {
            let (w, h) = tile_size(view);
            println!("tile view {view:?} {w}x{h}");
            let buf = draw(w, h, |f| {
                render_tile(f, f.area(), &session("proj-a", Status::Running), &Config::default(), false, 0, TileFx::default(), view)
            });
            for y in 0..h {
                let mut line = String::new();
                let mut x = 0;
                while x < w {
                    let cell = &buf[(x, y)];
                    line.push_str(&ansi(cell.symbol(), Some(cell.fg)));
                    x += unicode_width::UnicodeWidthStr::width(cell.symbol()).max(1) as u16;
                }
                println!("  {line}");
            }
        }
    }

    /// Plain-text gallery of every sprite and both tile views.
    /// Run with `cargo test dump_gallery -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn dump_gallery() {
        use crate::ui::mascot::{gallery_pixels, sprite, sprite_oneshot_done, SpriteSize};
        let statuses = [Status::Running, Status::NeedsInput, Status::Idle, Status::Done, Status::Unknown];
        let plain = |lines: &[Line]| -> Vec<String> {
            lines.iter().map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect()).collect()
        };
        for size in [SpriteSize::Large, SpriteSize::Small] {
            for st in statuses {
                let ticks: Vec<u64> = match st {
                    Status::Done => vec![0],
                    _ => {
                        let len = crate::ui::mascot::loop_len(st);
                        (0..len).step_by((len as usize / 6).max(2)).collect()
                    }
                };
                println!("== {st:?} {size:?} ticks {ticks:?} (pixels: o body = shade l shine # dark * accent; then Braille cells)");
                let pix: Vec<Vec<String>> = ticks.iter().map(|t| gallery_pixels(st, size, *t)).collect();
                for r in 0..pix[0].len() {
                    println!("{}", pix.iter().map(|p| p[r].clone()).collect::<Vec<_>>().join("   "));
                }
                let cells: Vec<Vec<String>> = ticks.iter().map(|t| plain(&sprite(st, size, *t))).collect();
                for r in 0..cells[0].len() {
                    println!("{}", cells.iter().map(|p| p[r].clone()).collect::<Vec<_>>().join("   "));
                }
            }
            println!("== Done one-shot {size:?} ticks 0,2,4,6,8,12,14,15 (14+ = held pose)");
            let cells: Vec<Vec<String>> = [0, 2, 4, 6, 8, 12, 14, 15].iter().map(|t| plain(&sprite_oneshot_done(size, *t))).collect();
            for r in 0..cells[0].len() {
                println!("{}", cells.iter().map(|p| p[r].clone()).collect::<Vec<_>>().join("   "));
            }
        }
        let mut s = session("프로젝트-alpha", Status::Running);
        s.branch_guessed = true;
        for view in [ViewKind::A, ViewKind::B] {
            let (w, h) = tile_size(view);
            println!("== tile view {view:?} {w}x{h} (selected, tick 0, all extras)");
            for r in tile(w, h, &s, &all_extras(), true, 0, view) {
                println!("{r}");
            }
            println!("== tile view {view:?} {}x{} (not selected, default extras)", w + 6, h + 2);
            for r in tile(w + 6, h + 2, &session("proj-a", Status::Idle), &Config::default(), false, 0, view) {
                println!("{r}");
            }
        }
        println!("== HUD");
        let counts = crate::ui::hud::StatusCounts { running: 3, needs_input: 1, idle: 2, done: 1, unknown: 0 };
        let buf = draw(90, 1, |f| crate::ui::hud::render_hud(f, f.area(), &counts));
        println!("{}", rows(&buf)[0]);
    }
}
