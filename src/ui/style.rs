//! Shared game-style look: palette, thin rounded border, blinking cursor,
//! and small text helpers (durations, width-aware truncation).
//!
//! Cards and the settings "game menu" window use the same border set so the
//! whole UI reads as one game. The frame is a thin one-cell line so the
//! scene and text stay the focus.

use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols::border;
use ratatui::widgets::Block;
use unicode_width::UnicodeWidthChar;

use crate::model::Status;

/// Thin rounded line frame: `╭─╮ │ │ ╰─╯`.
pub const BORDER: border::Set = border::ROUNDED;

/// Menu / selection cursor glyph.
pub const CURSOR: &str = "▶";

/// Cursor blink period in 12 fps ticks (visible for the first half).
pub const CURSOR_PERIOD: u64 = 12;

/// Animation frame rate the tick counter is based on.
pub const FPS: u64 = 12;

/// Selection colour (selected card border, `▶` cursor, settings window,
/// scroll hints): "cmux blue" `#4C8DFF`, the colour cmux itself gives agent
/// status in its sidebar (`cmux list-status`: `claude_code=Running …
/// color=#4C8DFF`), which is the default `app.accentColor: "cmux"` ("cmux
/// blue") of cmux's settings schema. No status colour is blue-dominant, so
/// blue always means "selected".
pub const SELECTED: Color = Color::Rgb(0x4C, 0x8D, 0xFF);
/// Background of the HUD's ` ◆ SESSIONS n ` badge (the original game yellow).
pub const HUD_BADGE: Color = Color::Rgb(255, 236, 64);
/// Warning red (PR conflicts, alert glyphs).
pub const FLASH_ALERT: Color = Color::Rgb(255, 84, 60);
pub const TEXT: Color = Color::Rgb(236, 230, 220);
pub const TEXT_DIM: Color = Color::Rgb(150, 142, 132);
pub const HUD_BG: Color = Color::Rgb(30, 26, 44);
pub const HUD_FG: Color = Color::Rgb(236, 230, 220);
pub const BADGE_FG: Color = Color::Rgb(20, 16, 24);

pub const RUNNING: Color = Color::Rgb(242, 140, 40);
pub const NEEDS_INPUT: Color = Color::Rgb(250, 204, 21);
pub const IDLE: Color = Color::Rgb(140, 116, 96);
pub const DONE: Color = Color::Rgb(84, 200, 110);
pub const UNKNOWN: Color = Color::Rgb(140, 140, 140);

/// Main (body) color of a status; also the card border color.
pub fn status_color(status: Status) -> Color {
    match status {
        Status::Running => RUNNING,
        Status::NeedsInput => NEEDS_INPUT,
        Status::Idle => IDLE,
        Status::Done => DONE,
        Status::Unknown => UNKNOWN,
    }
}

/// Short status tag shown on cards.
pub fn status_tag(status: Status) -> &'static str {
    match status {
        Status::Running => "RUN",
        Status::NeedsInput => "INPUT",
        Status::Idle => "IDLE",
        Status::Done => "DONE",
        Status::Unknown => "???",
    }
}

/// One-cell HUD icon per status.
pub fn status_icon(status: Status) -> &'static str {
    match status {
        Status::Running => "✦",
        Status::NeedsInput => "!",
        Status::Idle => "z",
        Status::Done => "✓",
        Status::Unknown => "?",
    }
}

/// Whether the blinking cursor is visible on this tick.
pub fn cursor_visible(tick: u64) -> bool {
    tick % CURSOR_PERIOD < CURSOR_PERIOD / 2
}

/// Thin-bordered window in `color`; `selected` turns it bold cmux blue.
pub fn window_block(color: Color, selected: bool) -> Block<'static> {
    let style = if selected {
        Style::new().fg(SELECTED).add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(color)
    };
    Block::bordered().border_set(BORDER).border_style(style)
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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn cursor_blinks_six_on_six_off() {
        assert!((0..6).all(cursor_visible));
        assert!(!(6..12).any(cursor_visible));
        assert!(cursor_visible(12));
    }

    #[test]
    fn border_is_thin_rounded_line() {
        assert_eq!(BORDER, border::ROUNDED);
        assert_eq!(BORDER.top_left, "╭");
        assert_eq!(BORDER.vertical_left, "│");
    }
}
