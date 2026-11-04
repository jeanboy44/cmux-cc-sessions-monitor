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

use crate::config::{CardLayout, Config};
use crate::model::ModelFamily;
use crate::spirit::theme::Theme;
use crate::ui::style::{self, cursor_visible, window_block, CURSOR};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsItem {
    /// Standard or large cards (also `v`).
    CardLayout,
    Theme,
    /// Spirit element of one model family.
    ModelTheme(ModelFamily),
    ExtraBranch,
    ExtraElapsed,
    ExtraLastActivity,
    ExtraSummary,
    ExtraPr,
    GuessWorktree,
}

pub const ALL_ITEMS: [SettingsItem; 12] = [
    SettingsItem::CardLayout,
    SettingsItem::Theme,
    SettingsItem::ModelTheme(ModelFamily::Fable),
    SettingsItem::ModelTheme(ModelFamily::Opus),
    SettingsItem::ModelTheme(ModelFamily::Sonnet),
    SettingsItem::ModelTheme(ModelFamily::Haiku),
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

/// Theme items cycle (see [`Theme::next`]), the card layout switches
/// between its two values; every other item flips its flag.
/// A model row cycles from the family's current element, so a family missing
/// from a hand-edited map starts from its default.
pub fn toggle(cfg: &mut Config, item: SettingsItem) {
    match item {
        SettingsItem::CardLayout => cfg.card_layout = cfg.card_layout.next(),
        SettingsItem::Theme => cfg.theme = cfg.theme.next(),
        SettingsItem::ModelTheme(f) => {
            let next = cfg.model_theme(f).next();
            cfg.model_themes.insert(f.as_key().to_string(), next);
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
        SettingsItem::CardLayout => "카드 레이아웃",
        SettingsItem::Theme => "기본 테마",
        SettingsItem::ModelTheme(ModelFamily::Fable) => "fable 모델",
        SettingsItem::ModelTheme(ModelFamily::Opus) => "opus 모델",
        SettingsItem::ModelTheme(ModelFamily::Sonnet) => "sonnet 모델",
        SettingsItem::ModelTheme(ModelFamily::Haiku) => "haiku 모델",
        SettingsItem::ExtraBranch => "브랜치",
        SettingsItem::ExtraElapsed => "경과 시간",
        SettingsItem::ExtraLastActivity => "마지막 활동",
        SettingsItem::ExtraSummary => "한 줄 요약",
        SettingsItem::ExtraPr => "PR 상태",
        SettingsItem::GuessWorktree => "워크트리 추정(~)",
    }
}

/// Boolean state of a flag item; `None` for the theme and layout items.
fn flag(cfg: &Config, item: SettingsItem) -> Option<bool> {
    match item {
        SettingsItem::CardLayout | SettingsItem::Theme | SettingsItem::ModelTheme(_) => None,
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
        (SettingsItem::CardLayout, None) => layout_label(cfg.card_layout).into(),
        (SettingsItem::ModelTheme(f), None) => theme_label(cfg.model_theme(f)).into(),
        (_, None) => theme_label(cfg.theme).into(),
    }
}

/// Korean name of a card layout for the settings window.
pub fn layout_label(layout: CardLayout) -> &'static str {
    match layout {
        CardLayout::Standard => "기본",
        CardLayout::Large => "크게",
    }
}

/// Korean name of a theme for the settings window.
pub fn theme_label(theme: Theme) -> &'static str {
    match theme {
        Theme::Fire => "불",
        Theme::Water => "물",
        Theme::Poison => "독",
        Theme::Lightning => "번개",
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
    let block = window_block(style::SELECTED, false)
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
    fn toggle_theme_cycles_all_themes() {
        let mut c = Config::default();
        assert_eq!(c.theme, Theme::Fire);
        let mut seen = Vec::new();
        for _ in 0..4 {
            toggle(&mut c, SettingsItem::Theme);
            seen.push(c.theme);
        }
        assert_eq!(seen, [Theme::Water, Theme::Poison, Theme::Lightning, Theme::Fire]);
        assert_eq!(item_value(&c, SettingsItem::Theme), "불");
        c.theme = Theme::Lightning;
        assert_eq!(item_value(&c, SettingsItem::Theme), "번개");
    }

    #[test]
    fn toggle_extra_flips_flag() {
        let mut c = Config::default();
        for item in ALL_ITEMS.into_iter().filter(|i| flag(&Config::default(), *i).is_some()) {
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
        cfg.theme = Theme::Poison;
        let elapsed = ALL_ITEMS.iter().position(|i| *i == SettingsItem::ExtraElapsed).unwrap();
        let text = screen(&cfg, elapsed, 0, None);
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
        assert!(row("기본 테마").trim_end_matches(['│', ' ']).ends_with("독"), "{all}");
        assert!(!all.contains("뷰"), "the A/B view row is gone");
        assert!(row("PR 상태").contains("꺼짐"));
        // hidden half of the blink: no cursor
        let hidden = screen(&cfg, elapsed, 6, None).join("\n");
        assert!(!hidden.contains(CURSOR), "{hidden}");
        assert!(hidden.contains("경과 시간"));
        // v2-only extras do not exist
        for v2 in ["토큰", "CPU", "도구 호출", "메모리"] {
            assert!(!all.contains(v2), "{v2}");
        }
    }

    #[test]
    fn model_rows_cycle_each_family_independently() {
        use crate::model::ModelFamily;
        let mut c = Config::default();
        let rows: Vec<SettingsItem> = ModelFamily::ALL.into_iter().map(SettingsItem::ModelTheme).collect();
        for item in &rows {
            assert!(ALL_ITEMS.contains(item), "{item:?} is a settings row");
        }
        let labels: Vec<&str> = rows.iter().map(|i| item_label(*i)).collect();
        assert_eq!(labels, ["fable 모델", "opus 모델", "sonnet 모델", "haiku 모델"]);
        let values: Vec<String> = rows.iter().map(|i| item_value(&c, *i)).collect();
        assert_eq!(values, ["물", "불", "번개", "독"], "default mapping shown");
        // opus cycles through all four elements and back; others stay put
        let mut seen = Vec::new();
        for _ in 0..4 {
            toggle(&mut c, SettingsItem::ModelTheme(ModelFamily::Opus));
            seen.push(c.model_theme(ModelFamily::Opus));
        }
        assert_eq!(seen, [Theme::Water, Theme::Poison, Theme::Lightning, Theme::Fire]);
        for f in ModelFamily::ALL {
            toggle(&mut c, SettingsItem::ModelTheme(f));
        }
        assert_eq!(
            ModelFamily::ALL.map(|f| c.model_theme(f)),
            [Theme::Poison, Theme::Water, Theme::Fire, Theme::Lightning],
            "each row cycles its own family from its current element"
        );
        assert_eq!(c.theme, Theme::Fire, "the default theme row is untouched");
        // a family missing from a hand-edited map cycles from its default
        c.model_themes.clear();
        toggle(&mut c, SettingsItem::ModelTheme(ModelFamily::Haiku));
        assert_eq!(c.model_themes.get("haiku"), Some(&Theme::Lightning));
    }

    #[test]
    fn card_layout_row_cycles_standard_and_large() {
        use crate::config::CardLayout;
        assert_eq!(ALL_ITEMS[0], SettingsItem::CardLayout, "the first row");
        assert_eq!(item_label(SettingsItem::CardLayout), "카드 레이아웃");
        let mut c = Config::default();
        assert_eq!(item_value(&c, SettingsItem::CardLayout), "기본");
        toggle(&mut c, SettingsItem::CardLayout);
        assert_eq!(c.card_layout, CardLayout::Large);
        assert_eq!(item_value(&c, SettingsItem::CardLayout), "크게");
        toggle(&mut c, SettingsItem::CardLayout);
        assert_eq!(c, Config::default());
        let text = screen(&Config { card_layout: CardLayout::Large, ..Config::default() }, 0, 0, None);
        let row = text.iter().find(|l| l.contains("카드 레이아웃")).expect("row drawn");
        assert!(row.contains(CURSOR) && row.trim_end_matches(['│', ' ']).ends_with("크게"), "{row}");
    }

    #[test]
    fn settings_render_shows_model_rows() {
        let mut cfg = Config::default();
        cfg.model_themes.insert("sonnet".into(), Theme::Fire);
        let text = screen(&cfg, 0, 0, None);
        let row = |label: &str| text.iter().find(|l| l.contains(label)).unwrap_or_else(|| panic!("{label}: {text:?}")).clone();
        let end = |label: &str| row(label).trim_end_matches(['│', ' ']).to_string();
        assert!(end("fable 모델").ends_with("물"));
        assert!(end("opus 모델").ends_with("불"));
        assert!(end("sonnet 모델").ends_with("불"));
        assert!(end("haiku 모델").ends_with("독"));
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
