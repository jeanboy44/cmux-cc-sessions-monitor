//! Top game-HUD bar: party size, per-status counters and an "active" gauge.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::model::Status;
use crate::ui::style;

/// Number of sessions per status.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StatusCounts {
    pub running: usize,
    pub needs_input: usize,
    pub idle: usize,
    pub done: usize,
    pub unknown: usize,
}

impl StatusCounts {
    pub fn from_statuses(statuses: impl IntoIterator<Item = Status>) -> Self {
        let mut c = Self::default();
        for st in statuses {
            *c.slot(st) += 1;
        }
        c
    }

    pub fn total(&self) -> usize {
        self.running + self.needs_input + self.idle + self.done + self.unknown
    }

    pub fn get(&self, status: Status) -> usize {
        match status {
            Status::Running => self.running,
            Status::NeedsInput => self.needs_input,
            Status::Idle => self.idle,
            Status::Done => self.done,
            Status::Unknown => self.unknown,
        }
    }

    fn slot(&mut self, status: Status) -> &mut usize {
        match status {
            Status::Running => &mut self.running,
            Status::NeedsInput => &mut self.needs_input,
            Status::Idle => &mut self.idle,
            Status::Done => &mut self.done,
            Status::Unknown => &mut self.unknown,
        }
    }
}

/// HP-bar style gauge of exactly `width` cells: `█` full, `▓` partial, `░` empty.
/// `value` is clamped to 0..=1; NaN counts as 0.
pub fn hud_gauge(value: f32, width: u16) -> String {
    let v = if value.is_nan() { 0.0 } else { value.clamp(0.0, 1.0) };
    let filled = v * f32::from(width);
    let full = (filled.floor() as u16).min(width);
    let partial = u16::from(full < width && filled - f32::from(full) > 0.0);
    let mut out = String::with_capacity(width as usize * 3);
    out.extend(std::iter::repeat_n('█', full as usize));
    out.extend(std::iter::repeat_n('▓', partial as usize));
    out.extend(std::iter::repeat_n('░', (width - full - partial) as usize));
    out
}

const HUD_ORDER: [(Status, &str); 5] = [
    (Status::Running, "RUN"),
    (Status::NeedsInput, "INPUT"),
    (Status::Idle, "IDLE"),
    (Status::Done, "DONE"),
    (Status::Unknown, "UNK"),
];

/// Draws the one-line HUD: `◆ SESSIONS n  ✦ RUN n  ! INPUT n  …  ACT ███░░`.
/// Clips silently on narrow or empty areas.
pub fn render_hud(f: &mut Frame, area: Rect, counts: &StatusCounts) {
    let area = area.intersection(f.area());
    if area.is_empty() {
        return;
    }
    let base = Style::new().fg(style::HUD_FG).bg(style::HUD_BG);
    let mut spans = vec![
        Span::styled(
            format!(" ◆ SESSIONS {} ", counts.total()),
            Style::new().fg(style::BADGE_FG).bg(style::SELECTED).add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
    ];
    for (st, label) in HUD_ORDER {
        let n = counts.get(st);
        let mut icon = Style::new().fg(style::status_color(st)).add_modifier(Modifier::BOLD);
        if n == 0 {
            icon = icon.add_modifier(Modifier::DIM);
        }
        spans.push(Span::styled(style::status_icon(st), icon));
        spans.push(Span::raw(format!(" {label} {n}  ")));
    }
    let active = counts.running + counts.needs_input;
    let ratio = if counts.total() == 0 { 0.0 } else { active as f32 / counts.total() as f32 };
    spans.push(Span::raw("ACT "));
    spans.push(Span::styled(hud_gauge(ratio, 8), Style::new().fg(style::RUNNING)));
    f.render_widget(Paragraph::new(Line::from(spans)).style(base), area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Status;
    use crate::ui::test_util::{draw, rows};

    #[test]
    fn hud_gauge_clamps_and_zero_width_ok() {
        assert_eq!(hud_gauge(0.5, 0), "");
        assert_eq!(hud_gauge(1.0, 4), "████");
        assert_eq!(hud_gauge(7.5, 4), "████");
        assert_eq!(hud_gauge(0.0, 4), "░░░░");
        assert_eq!(hud_gauge(-3.0, 4), "░░░░");
        assert_eq!(hud_gauge(f32::NAN, 4), "░░░░");
        assert_eq!(hud_gauge(f32::INFINITY, 3), "███");
        assert_eq!(hud_gauge(0.5, 4), "██░░");
        assert_eq!(hud_gauge(0.6, 4), "██▓░");
        for w in 0..20u16 {
            assert_eq!(hud_gauge(0.37, w).chars().count(), w as usize);
        }
    }

    #[test]
    fn status_counts_from_statuses() {
        let c = StatusCounts::from_statuses([
            Status::Running,
            Status::Running,
            Status::NeedsInput,
            Status::Done,
            Status::Unknown,
        ]);
        assert_eq!(
            c,
            StatusCounts { running: 2, needs_input: 1, idle: 0, done: 1, unknown: 1 }
        );
        assert_eq!(c.total(), 5);
    }

    #[test]
    fn hud_shows_counts_per_status() {
        let counts = StatusCounts { running: 2, needs_input: 1, idle: 3, done: 0, unknown: 1 };
        let buf = draw(100, 1, |f| render_hud(f, f.area(), &counts));
        let row = &rows(&buf)[0];
        for want in ["SESSIONS 7", "✦ RUN 2", "! INPUT 1", "z IDLE 3", "✓ DONE 0", "? UNK 1"] {
            assert!(row.contains(want), "missing {want:?} in {row:?}");
        }
    }

    #[test]
    fn hud_tiny_areas_do_not_panic() {
        let counts = StatusCounts { running: 99, ..Default::default() };
        for (w, h) in [(0, 0), (1, 1), (5, 1), (10, 3)] {
            draw(w, h, |f| render_hud(f, f.area(), &counts));
        }
        // Area outside the frame is ignored.
        draw(10, 2, |f| render_hud(f, Rect::new(50, 50, 20, 1), &counts));
    }
}
