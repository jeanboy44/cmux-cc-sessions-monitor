//! Shared game-style look: palette, thin rounded border, blinking cursor.
//!
//! Tiles and the settings "game menu" window use the same border set, cursor
//! glyph and blink rhythm so the whole UI reads as one game. The frame is a
//! thin one-cell line so the mascot and text stay the focus.

use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols::border;
use ratatui::widgets::Block;

use crate::model::Status;

/// Thin rounded line frame: `╭─╮ │ │ ╰─╯`.
pub const BORDER: border::Set = border::ROUNDED;

/// Menu / selection cursor glyph.
pub const CURSOR: &str = "▶";

/// Cursor blink period in 12 fps ticks (visible for the first half).
pub const CURSOR_PERIOD: u64 = 12;

/// Animation frame rate the tick counter is based on.
pub const FPS: u64 = 12;

/// Selection colour (selected tile border, `▶` cursor, settings window,
/// scroll hints): "cmux blue" `#4C8DFF`, the colour cmux itself gives agent
/// status in its sidebar (`cmux list-status`: `claude_code=Running …
/// color=#4C8DFF`), which is the default `app.accentColor: "cmux"` ("cmux
/// blue") of cmux's settings schema. No status colour is blue-dominant, so
/// blue always means "selected".
pub const SELECTED: Color = Color::Rgb(0x4C, 0x8D, 0xFF);
/// Background of the HUD's ` ◆ SESSIONS n ` badge (the original game yellow).
pub const HUD_BADGE: Color = Color::Rgb(255, 236, 64);
pub const FLASH: Color = Color::Rgb(255, 255, 255);
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

/// Main (body) color of a status; also the tile border color.
pub fn status_color(status: Status) -> Color {
    match status {
        Status::Running => RUNNING,
        Status::NeedsInput => NEEDS_INPUT,
        Status::Idle => IDLE,
        Status::Done => DONE,
        Status::Unknown => UNKNOWN,
    }
}

/// Short status tag shown on tiles.
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

/// Thin-bordered window in `color`; `selected`/`flash` brighten it.
pub fn window_block(color: Color, selected: bool, flash: bool) -> Block<'static> {
    let style = if flash {
        Style::new().fg(FLASH).add_modifier(Modifier::BOLD)
    } else if selected {
        Style::new().fg(SELECTED).add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(color)
    };
    Block::bordered().border_set(BORDER).border_style(style)
}

#[cfg(test)]
mod tests {
    use super::*;

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
