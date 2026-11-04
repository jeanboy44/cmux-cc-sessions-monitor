//! Subagent dots: one small round dot per running subagent at the body's
//! lower right, on the band's bottom row (never the eye row), in a short
//! line going right. Each dot takes its subagent's element colour (the base
//! body tone of that element), bobs with the main spirit's bob (same stage
//! and state, a little behind it, each dot further behind), pulses brighter
//! while the parent runs, is dimmed like the body when the parent is idle or
//! unknown, and is gone when the parent is done (see `render_frame`). One
//! dot per braille cell, so dots never mix colours; foreground only, never
//! white.

use super::canvas::{lerp, mul, Canvas, Geom};
use super::theme::Theme;
use super::{element, state, Ctx, SpiritGrid, BASE_R};
use crate::model::Status;

/// At most this many dots; further running subagents are only counted in the text.
pub const MAX_DOTS: usize = 4;
/// Over the body and ambient bits (1), under the eye catchlight.
const DOT_PRIO: u8 = 2;
/// Ticks each dot lags behind the body's bob (times its index + 1).
const LAG: f32 = 4.0;

/// One running subagent, drawn in its element's colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SubagentDot {
    pub theme: Theme,
}

/// First dot column: the cell right of where the round body meets the top
/// of the bottom cell row.
fn first_col(g: &Geom) -> i32 {
    let r = BASE_R * g.ppu;
    let dy = (g.h as f32 - 4.0 - g.rcy).max(0.0);
    let dx = (r * r - dy * dy).max(0.0).sqrt();
    ((g.rcx + dx) / 2.0).floor() as i32 + 1
}

/// Canvas width (sub-pixels) that holds `n` dots.
pub(super) fn width_for(g: &Geom, n: usize) -> usize {
    ((first_col(g) + n as i32) * 2).max(0) as usize
}

/// Grid columns of the first `n` dots of a spirit of `size`.
#[cfg(test)]
pub(super) fn dot_cols(size: super::Size, n: usize) -> Vec<u16> {
    let c0 = first_col(&size.geom());
    (0..n.min(MAX_DOTS)).map(|i| (c0 + i as i32) as u16).collect()
}

/// Paints the dots (none when the parent is done).
pub(super) fn draw(cv: &mut Canvas, ctx: &Ctx, dots: &[SubagentDot]) {
    if ctx.state == Status::Done || dots.is_empty() {
        return;
    }
    let g = cv.g;
    let (c0, base_y) = (first_col(&g), g.h as i32 - 3);
    let bright = state::brightness(ctx.state, state::pulse(ctx.state, ctx.tf));
    for (i, d) in dots.iter().take(MAX_DOTS).enumerate() {
        let k = i as f32;
        let bob = state::bob_at(ctx.stage, ctx.state, ctx.seed, ctx.tf - LAG * (k + 1.0));
        let dy = (bob * 1.5).round().clamp(-1.0, 1.0) as i32;
        let ramp = element::ramp(d.theme);
        let c = match ctx.state {
            Status::Running => lerp(ramp[1], ramp[2], 0.5 + 0.5 * (ctx.tf * 0.35 + k * 1.3).sin()),
            _ => mul(ramp[1], bright),
        };
        let (x, y) = ((c0 + i as i32) * 2, base_y + dy);
        cv.set_px(x, y, c, DOT_PRIO, false);
        cv.set_px(x, y + 1, c, DOT_PRIO, false);
    }
}

/// Columns the canvas grew by for the dots (from `base_cols` on) keep only
/// the dot cells, so nothing else shows where the spirit had no room before.
pub(super) fn keep_only_dots_past(grid: &mut SpiritGrid, g: &Geom, base_cols: u16, n: usize) {
    if n == 0 {
        return;
    }
    let c0 = first_col(g);
    let last = grid.rows.saturating_sub(1);
    for r in 0..grid.rows {
        for c in base_cols..grid.cols {
            let dot = r == last && (c0..c0 + n as i32).contains(&i32::from(c));
            if !dot {
                grid.cells[usize::from(r) * usize::from(grid.cols) + usize::from(c)] = super::Cell { ch: ' ', fg: None };
            }
        }
    }
}


#[cfg(test)]
mod tests {
    use super::super::*;
    use super::*;
    use crate::model::Status;

    const STATUSES: [Status; 5] = [Status::Running, Status::NeedsInput, Status::Idle, Status::Done, Status::Unknown];

    fn p(theme: Theme, status: Status, size: Size, tick: u64) -> SpiritParams {
        SpiritParams { theme, status, size, tick, seed: 7, warning: false, magic: Magic::Normal, stage: Stage::Lively, cols: 0 }
    }

    fn dots(themes: &[Theme]) -> Vec<SubagentDot> {
        themes.iter().map(|&theme| SubagentDot { theme }).collect()
    }

    /// Cells (col, row, cell) that differ from the render without dots.
    fn changed(p: &SpiritParams, d: &[SubagentDot]) -> Vec<(u16, u16, Cell)> {
        let (with, without) = (render_dots(p, d), render(p));
        (0..with.rows)
            .flat_map(|r| (0..with.cols).map(move |c| (c, r)))
            .filter_map(|(c, r)| {
                let i = usize::from(r) * usize::from(with.cols) + usize::from(c);
                let old = (c < without.cols).then(|| without.cells[usize::from(r) * usize::from(without.cols) + usize::from(c)]);
                let blank = Cell { ch: ' ', fg: None };
                (old.unwrap_or(blank) != with.cells[i]).then_some((c, r, with.cells[i]))
            })
            .collect()
    }

    fn cell_at(g: &SpiritGrid, c: u16, r: u16) -> Cell {
        g.cells[usize::from(r) * usize::from(g.cols) + usize::from(c)]
    }

    #[test]
    fn no_dots_render_exactly_as_before() {
        for theme in Theme::ALL {
            for status in STATUSES {
                for size in [Size::XS, Size::S, Size::M, Size::L] {
                    for tick in [0, 37, 500, u64::MAX] {
                        for magic in Magic::ALL {
                            let q = SpiritParams { magic, cols: 17, ..p(theme, status, size, tick) };
                            assert_eq!(render_dots(&q, &[]), render(&q), "{q:?}");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn one_dot_per_running_subagent_right_of_the_body_in_the_lower_half() {
        for size in [Size::XS, Size::S] {
            for status in [Status::Idle, Status::NeedsInput, Status::Unknown] {
                for n in 1..=MAX_DOTS {
                    for tick in [0, 11, 29] {
                        let q = p(Theme::Fire, status, size, tick);
                        let d = dots(&vec![Theme::Water; n]);
                        let cells = changed(&q, &d);
                        assert_eq!(cells.len(), n, "{size:?} {status:?} n={n} t={tick}: {cells:?}");
                        let g = render_dots(&q, &d);
                        for (c, r, cell) in &cells {
                            assert!(f32::from(*c) > body_center_x(size), "{size:?}: right of the body centre ({c})");
                            assert!(*r >= g.rows / 2, "{size:?}: lower half ({r})");
                            assert_eq!(*r, g.rows - 1, "{size:?}: the bottom row, never the eye row");
                            assert!(cell.ch != ' ' && cell.fg.is_some());
                        }
                        let cols: Vec<u16> = cells.iter().map(|c| c.0).collect();
                        assert_eq!(cols, dot_cols(size, n));
                    }
                }
            }
        }
        // more than four: four dots (the text counts the rest)
        let q = p(Theme::Fire, Status::Idle, Size::S, 0);
        assert_eq!(changed(&q, &dots(&[Theme::Water; 7])).len(), MAX_DOTS);
    }

    fn dominant(c: (u8, u8, u8)) -> &'static str {
        let (r, g, b) = (u32::from(c.0), u32::from(c.1), u32::from(c.2));
        if b > r && b > g {
            "blue"
        } else if g > r && g > b {
            "green"
        } else if r * 10 > g * 14 && r > b {
            "red"
        } else if r > b * 2 && g > b * 2 {
            "yellow"
        } else {
            "?"
        }
    }

    #[test]
    fn dot_colours_follow_each_subagents_element_and_are_never_white() {
        let want = [(Theme::Water, "blue"), (Theme::Poison, "green"), (Theme::Fire, "red"), (Theme::Lightning, "yellow")];
        for size in [Size::XS, Size::S] {
            for status in [Status::Running, Status::NeedsInput, Status::Idle, Status::Unknown] {
                for tick in [0, 9, 44] {
                    let q = p(Theme::Fire, status, size, tick);
                    let themes: Vec<Theme> = want.iter().map(|w| w.0).collect();
                    let g = render_dots(&q, &dots(&themes));
                    for (i, (theme, colour)) in want.iter().enumerate() {
                        let fg = cell_at(&g, dot_cols(size, 4)[i], g.rows - 1).fg.expect("painted");
                        assert_ne!(fg, (255, 255, 255), "never white");
                        if status != Status::Unknown {
                            assert_eq!(dominant(fg), *colour, "{size:?} {status:?} {theme:?} t={tick}: {fg:?}");
                        }
                    }
                }
            }
        }
    }

    fn dot_level(status: Status, tick: u64) -> u8 {
        let q = p(Theme::Water, status, Size::S, tick);
        let g = render_dots(&q, &dots(&[Theme::Water]));
        let fg = cell_at(&g, dot_cols(Size::S, 1)[0], g.rows - 1).fg.unwrap();
        fg.0.max(fg.1).max(fg.2)
    }

    #[test]
    fn dots_dim_with_an_idle_parent_and_vanish_when_done() {
        for t in [0, 13, 40] {
            assert!(dot_level(Status::Idle, t) < dot_level(Status::NeedsInput, t), "t={t}");
            assert!(dot_level(Status::Unknown, t) < dot_level(Status::Running, t), "t={t}");
            assert!(dot_level(Status::Idle, t) < dot_level(Status::Running, t), "t={t}");
        }
        for size in [Size::XS, Size::S] {
            for t in [0, 5, 77] {
                let q = p(Theme::Poison, Status::Done, size, t);
                assert_eq!(render_dots(&q, &dots(&[Theme::Water, Theme::Fire])), render(&q), "{size:?}: no dots when done");
            }
        }
        // running pulses: the brightness changes over ticks
        let levels: std::collections::BTreeSet<u8> = (0..40).map(|t| dot_level(Status::Running, t)).collect();
        assert!(levels.len() > 3, "{levels:?}");
    }

    #[test]
    fn dots_bob_offset_from_each_other_and_follow_the_stage() {
        let glyphs = |stage: Stage, status: Status| -> Vec<Vec<char>> {
            (0..120)
                .map(|t| {
                    let q = SpiritParams { stage, ..p(Theme::Fire, status, Size::S, t) };
                    let g = render_dots(&q, &dots(&[Theme::Water; 3]));
                    dot_cols(Size::S, 3).iter().map(|&c| cell_at(&g, c, g.rows - 1).ch).collect()
                })
                .collect()
        };
        let lively = glyphs(Stage::Lively, Status::NeedsInput);
        let distinct: std::collections::BTreeSet<char> = lively.iter().map(|f| f[0]).collect();
        assert!(distinct.len() >= 2, "the dot bobs: {distinct:?}");
        assert!(lively.iter().any(|f| f[0] != f[1] || f[1] != f[2]), "dots are offset from each other");
        // struggling moves more often than relaxed
        let moves = |frames: &[Vec<char>]| frames.windows(2).filter(|w| w[0] != w[1]).count();
        let (relaxed, struggling) = (glyphs(Stage::Relaxed, Status::NeedsInput), glyphs(Stage::Struggling, Status::NeedsInput));
        assert!(moves(&relaxed) < moves(&struggling), "{} < {}", moves(&relaxed), moves(&struggling));
    }

    #[test]
    fn dots_are_deterministic_and_never_panic() {
        for size in [Size::XS, Size::S, Size::M, Size::L] {
            for status in STATUSES {
                for tick in [0, 1, u64::MAX, u64::MAX - 1, 1 << 40] {
                    for magic in Magic::ALL {
                        for stage in [Stage::Relaxed, Stage::Lively, Stage::Struggling] {
                            let q = SpiritParams { magic, stage, cols: 17, warning: true, ..p(Theme::Lightning, status, size, tick) };
                            let d = dots(&[Theme::Water, Theme::Poison, Theme::Fire, Theme::Lightning, Theme::Water]);
                            let a = render_dots(&q, &d);
                            assert_eq!(a, render_dots(&q, &d));
                            assert_eq!(a.cells.len(), usize::from(a.cols) * usize::from(a.rows));
                        }
                    }
                }
            }
        }
    }
}
