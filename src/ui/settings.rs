//! The settings window: a game "menu" with a blinking `▶` cursor.
//!
//! Pure pieces live here ([`toggle`], [`item_label`], [`item_value`],
//! [`render_settings`]); applying a change (shared config, save, PR poller)
//! is `App`'s job.

use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use crate::config::{Config, ViewMode};
use crate::ui::style::{self, cursor_visible, window_block, CURSOR};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsItem {
    View,
    ExtraBranch,
    ExtraElapsed,
    ExtraLastActivity,
    ExtraSummary,
    ExtraPr,
    GuessWorktree,
}

pub const ALL_ITEMS: [SettingsItem; 7] = [
    SettingsItem::View,
    SettingsItem::ExtraBranch,
    SettingsItem::ExtraElapsed,
    SettingsItem::ExtraLastActivity,
    SettingsItem::ExtraSummary,
    SettingsItem::ExtraPr,
    SettingsItem::GuessWorktree,
];

pub const HELP: &str = "j/k 이동  Enter/Space 변경  Esc 닫기";
pub const PR_UNAVAILABLE: &str = "사용 불가";
const TITLE: &str = " ⚙ SETTINGS ";

/// View cycles `A -> B -> Auto -> A`; every other item flips its flag.
pub fn toggle(cfg: &mut Config, item: SettingsItem) {
    match item {
        SettingsItem::View => {
            cfg.view = match cfg.view {
                ViewMode::A => ViewMode::B,
                ViewMode::B => ViewMode::Auto,
                ViewMode::Auto => ViewMode::A,
            }
        }
        SettingsItem::ExtraBranch => cfg.extras.branch = !cfg.extras.branch,
        SettingsItem::ExtraElapsed => cfg.extras.elapsed = !cfg.extras.elapsed,
        SettingsItem::ExtraLastActivity => cfg.extras.last_activity = !cfg.extras.last_activity,
        SettingsItem::ExtraSummary => cfg.extras.summary = !cfg.extras.summary,
        SettingsItem::ExtraPr => cfg.extras.pr = !cfg.extras.pr,
        SettingsItem::GuessWorktree => cfg.guess_worktree = !cfg.guess_worktree,
    }
}

pub fn item_label(item: SettingsItem) -> &'static str {
    match item {
        SettingsItem::View => "뷰 (A/B/자동)",
        SettingsItem::ExtraBranch => "브랜치",
        SettingsItem::ExtraElapsed => "경과 시간",
        SettingsItem::ExtraLastActivity => "마지막 활동",
        SettingsItem::ExtraSummary => "한 줄 요약",
        SettingsItem::ExtraPr => "PR 상태",
        SettingsItem::GuessWorktree => "워크트리 추정(~)",
    }
}

/// Boolean state of a flag item; `None` for [`SettingsItem::View`].
fn flag(cfg: &Config, item: SettingsItem) -> Option<bool> {
    match item {
        SettingsItem::View => None,
        SettingsItem::ExtraBranch => Some(cfg.extras.branch),
        SettingsItem::ExtraElapsed => Some(cfg.extras.elapsed),
        SettingsItem::ExtraLastActivity => Some(cfg.extras.last_activity),
        SettingsItem::ExtraSummary => Some(cfg.extras.summary),
        SettingsItem::ExtraPr => Some(cfg.extras.pr),
        SettingsItem::GuessWorktree => Some(cfg.guess_worktree),
    }
}

pub fn item_value(cfg: &Config, item: SettingsItem) -> String {
    match (item, flag(cfg, item)) {
        (_, Some(true)) => "켜짐".into(),
        (_, Some(false)) => "꺼짐".into(),
        (_, None) => match cfg.view {
            ViewMode::A => "A".into(),
            ViewMode::B => "B".into(),
            ViewMode::Auto => "자동".into(),
        },
    }
}

fn box_glyph(on: Option<bool>) -> &'static str {
    match on {
        Some(true) => "[x]",
        Some(false) => "[ ]",
        None => "   ",
    }
}

/// Draws the centered window; clamps to `area` and never panics on tiny areas.
/// `pr_tools_note` is shown next to the PR item, e.g. [`PR_UNAVAILABLE`] or
/// "az 없음" when the CLI the sessions' remotes need is missing.
pub fn render_settings(f: &mut Frame, area: Rect, cfg: &Config, cursor: usize, tick: u64, pr_tools_note: Option<&str>) {
    let label_w = ALL_ITEMS.iter().map(|i| item_label(*i).width()).max().unwrap_or(0);
    let value_w = ALL_ITEMS.iter().map(|i| item_value(cfg, *i).width()).max().unwrap_or(0);
    let note_w = pr_tools_note.map_or(0, |n| n.width() + 1);
    // cursor(2) + box(3) + space + label + gap(2) + value + note
    let row_w = 2 + 3 + 1 + label_w + 2 + value_w + note_w;
    let inner_w = row_w.max(HELP.width());
    let w = ((inner_w + 4) as u16).min(area.width);
    let h = ((ALL_ITEMS.len() + 4) as u16).min(area.height);
    if w < 3 || h < 3 {
        return;
    }
    let rect = Rect::new(area.x + (area.width - w) / 2, area.y + (area.height - h) / 2, w, h);
    let block = window_block(style::SELECTED, false, false)
        .title(Line::styled(TITLE, Style::new().fg(style::SELECTED).add_modifier(Modifier::BOLD)));
    let inner = block.inner(rect);
    f.render_widget(Clear, rect);
    f.render_widget(block, rect);

    let text = Style::new().fg(style::TEXT);
    let dim = Style::new().fg(style::TEXT_DIM);
    let mut lines = Vec::with_capacity(ALL_ITEMS.len() + 2);
    for (i, item) in ALL_ITEMS.iter().enumerate() {
        let selected = i == cursor;
        let mark = if selected && cursor_visible(tick) { CURSOR } else { " " };
        let on = flag(cfg, *item);
        let label = item_label(*item);
        let base = if selected { text.add_modifier(Modifier::BOLD) } else { text };
        let mut value = vec![Span::styled(item_value(cfg, *item), if on == Some(false) { dim } else { base })];
        if let Some(note) = pr_tools_note.filter(|_| *item == SettingsItem::ExtraPr) {
            value.push(Span::styled(format!(" {note}"), dim));
        }
        let left = format!("{} {label}", box_glyph(on));
        let used = 2 + left.width() + value.iter().map(|s| s.content.width()).sum::<usize>();
        let mut spans = vec![
            Span::styled(format!("{mark} "), Style::new().fg(style::SELECTED).add_modifier(Modifier::BOLD)),
            Span::styled(left, base),
            Span::raw(" ".repeat(inner_w.saturating_sub(used))),
        ];
        spans.extend(value);
        lines.push(Line::from(spans));
    }
    lines.push(Line::raw(""));
    lines.push(Line::styled(HELP, dim).alignment(Alignment::Center));
    f.render_widget(Paragraph::new(lines), inner.inner(ratatui::layout::Margin::new(1, 0)));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::test_util::{draw, rows};

    #[test]
    fn toggle_view_cycles_a_b_auto() {
        let mut c = Config::default();
        c.view = ViewMode::A;
        toggle(&mut c, SettingsItem::View);
        assert_eq!(c.view, ViewMode::B);
        toggle(&mut c, SettingsItem::View);
        assert_eq!(c.view, ViewMode::Auto);
        toggle(&mut c, SettingsItem::View);
        assert_eq!(c.view, ViewMode::A);
    }

    #[test]
    fn toggle_extra_flips_flag() {
        let mut c = Config::default();
        for item in ALL_ITEMS.into_iter().filter(|i| *i != SettingsItem::View) {
            let before = flag(&c, item).unwrap();
            toggle(&mut c, item);
            assert_eq!(flag(&c, item), Some(!before), "{item:?}");
            toggle(&mut c, item);
            assert_eq!(flag(&c, item), Some(before), "{item:?}");
        }
        assert_eq!(c, Config::default());
        toggle(&mut c, SettingsItem::ExtraPr);
        assert!(c.extras.pr);
        toggle(&mut c, SettingsItem::GuessWorktree);
        assert!(!c.guess_worktree);
    }

    #[test]
    fn settings_render_tiny_area_does_not_panic() {
        let cfg = Config::default();
        for (w, h) in [(0, 0), (1, 1), (2, 2), (10, 3), (30, 5), (200, 3)] {
            for note in [None, Some(PR_UNAVAILABLE), Some("az 없음")] {
                let buf = draw(w, h, |f| render_settings(f, f.area(), &cfg, 99, 0, note));
                assert_eq!(rows(&buf).len(), h as usize);
            }
        }
    }

    fn screen(cfg: &Config, cursor: usize, tick: u64, note: Option<&str>) -> Vec<String> {
        rows(&draw(60, 16, |f| render_settings(f, f.area(), cfg, cursor, tick, note)))
    }

    #[test]
    fn settings_render_shows_cursor_and_values() {
        let mut cfg = Config::default();
        cfg.view = ViewMode::B;
        let text = screen(&cfg, 2, 0, None);
        let all = text.join("\n");
        for item in ALL_ITEMS {
            assert!(all.contains(item_label(item)), "{item:?}\n{all}");
        }
        assert!(all.contains("SETTINGS") && all.contains(HELP), "{all}");
        let row = |label: &str| text.iter().find(|l| l.contains(label)).unwrap().clone();
        assert!(row("경과 시간").contains(CURSOR), "cursor on the selected row");
        assert_eq!(all.matches(CURSOR).count(), 1);
        assert!(row("경과 시간").contains("[x]") && row("경과 시간").trim_end_matches(['│', ' ']).ends_with("켜짐"));
        assert!(row("마지막 활동").contains("[ ]") && row("마지막 활동").contains("꺼짐"));
        assert!(row("뷰").contains('B'));
        assert!(row("PR 상태").contains("꺼짐"));
        // hidden half of the blink: no cursor
        let hidden = screen(&cfg, 2, 6, None).join("\n");
        assert!(!hidden.contains(CURSOR), "{hidden}");
        assert!(hidden.contains("경과 시간"));
        // v2-only extras do not exist
        for v2 in ["토큰", "CPU", "도구 호출", "메모리"] {
            assert!(!all.contains(v2), "{v2}");
        }
    }

    #[test]
    fn settings_shows_pr_unavailable_note_when_tools_missing() {
        let cfg = Config::default();
        let with = screen(&cfg, 0, 0, Some(PR_UNAVAILABLE));
        let pr = with.iter().find(|l| l.contains("PR 상태")).unwrap();
        assert!(pr.contains(PR_UNAVAILABLE), "{pr}");
        assert_eq!(with.join("\n").matches(PR_UNAVAILABLE).count(), 1);
        assert!(!screen(&cfg, 0, 0, None).join("\n").contains(PR_UNAVAILABLE));
        // remote-aware note names the missing tool
        let az = screen(&cfg, 0, 0, Some("az 없음"));
        assert!(az.iter().find(|l| l.contains("PR 상태")).unwrap().contains("az 없음"), "{az:?}");
    }
}
