//! Subagent beads: a small grid of one bead per running subagent, drawn on
//! its own under the spirit. Beads fill it left to right, top to bottom, so
//! the grid grows one bead at a time and the count reads at a glance.
//!
//! Each bead is EXACTLY ONE braille cell holding a 2 x 2 square of
//! sub-pixels (`⠶`). The beads stay still, so the grid stays tidy; the state
//! shows in a soft shimmer only.
//!
//! Colour: the subagent's element (bright base tone with its lighter core
//! blended in, as a braille cell has one colour), never white. Running: each
//! bead twinkles on its own phase, as each subagent works on its own.
//! Needs input: all pulse together, with the body. Idle: dimmed like the
//! body, breathing slowly and faintly. Unknown: dimmed and steady. Done: gone.
//!
//! Beads sit on every other column: a cell is about twice as tall as wide,
//! so the grid is square on screen. At most [`MAX_DOTS`], and what the rows
//! given hold; the text counts the rest.

use super::canvas::{lerp, mul, Rgb};
use super::theme::Theme;
use super::{element, state, Cell, Ctx, SpiritGrid, SpiritParams};
use crate::model::Status;

/// Beads per row of the grid.
const BEAD_COLS: u16 = 4;
/// At most this many dots (4 x 4); further running subagents are only counted in the text.
pub const MAX_DOTS: usize = 16;
/// Columns from one bead to the next.
const BEAD_STEP: u16 = 2;
/// The bead: a 2 x 2 square of sub-pixels on the middle rows of its cell.
const BEAD: char = '⠶';
/// Phase between neighbouring running beads (radians): the golden angle, so
/// no two read as in step and no wave runs along the row.
const TWINKLE_PHASE: f32 = 2.4;
/// Rate and depth of the running twinkle.
const TWINKLE_RATE: f32 = 0.35;
const TWINKLE_DEPTH: f32 = 0.2;
/// Rate and depth of the idle breath.
const BREATH_RATE: f32 = 0.08;
const BREATH_DEPTH: f32 = 0.1;

/// One running subagent, drawn in its element's colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SubagentDot {
    pub theme: Theme,
}

/// Cell (column, row) of bead `i`: left to right, then the next row.
fn bead_cell(i: usize) -> (i32, i32) {
    let (cols, step) = (i32::from(BEAD_COLS), i32::from(BEAD_STEP));
    let i = i as i32;
    (i % cols * step, i / cols)
}

/// Colour of bead `i` of element `theme`: rim and a lighter core, averaged,
/// with the shimmer of the parent's state.
fn colour(ctx: &Ctx, theme: Theme, i: usize) -> Rgb {
    let ramp = element::ramp(theme);
    let twinkle = match ctx.state {
        Status::Running => TWINKLE_DEPTH * (ctx.tf * TWINKLE_RATE + i as f32 * TWINKLE_PHASE).sin(),
        _ => 0.0,
    };
    let rim = lerp(ramp[1], ramp[2], 0.5 + twinkle);
    let core = lerp(rim, ramp[3], 0.35);
    let avg = [(3.0 * rim[0] + core[0]) / 4.0, (3.0 * rim[1] + core[1]) / 4.0, (3.0 * rim[2] + core[2]) / 4.0];
    let breath = match ctx.state {
        Status::Idle => 1.0 + BREATH_DEPTH * (ctx.tf * BREATH_RATE).sin(),
        _ => 1.0,
    };
    match ctx.state {
        Status::Running => avg,
        s => mul(avg, state::brightness(s, state::pulse(s, ctx.tf)) * breath),
    }
}

/// The bead grid of `dots` at most `rows` rows tall (no beads when the
/// parent is done).
pub fn render_beads(p: &SpiritParams, dots: &[SubagentDot], rows: u16) -> SpiritGrid {
    let cols = (BEAD_COLS - 1) * BEAD_STEP + 1;
    let blank = Cell { ch: ' ', fg: None };
    let mut grid = SpiritGrid { cols, rows, cells: vec![blank; usize::from(cols) * usize::from(rows)] };
    if p.status == Status::Done {
        return grid;
    }
    let ctx = Ctx::new(p);
    let fit = usize::from(BEAD_COLS) * usize::from(rows);
    for (i, d) in dots.iter().take(fit.min(MAX_DOTS)).enumerate() {
        let (col, row) = bead_cell(i);
        grid.put(col, row, BEAD, colour(&ctx, d.theme, i));
    }
    grid
}

#[cfg(test)]
mod tests {
    use super::super::canvas::BRAILLE_BITS;
    use super::super::*;
    use super::*;
    use crate::model::Status;
    use std::collections::BTreeSet;

    const STATUSES: [Status; 5] = [Status::Running, Status::NeedsInput, Status::Idle, Status::Done, Status::Unknown];
    /// Bead rows of the standard and large cards.
    const ROWS: [u16; 2] = [3, 4];

    fn p(theme: Theme, status: Status, size: Size, tick: u64) -> SpiritParams {
        SpiritParams { theme, status, size, tick, seed: 7, warning: false, stage: Stage::Lively }
    }

    fn dots(themes: &[Theme]) -> Vec<SubagentDot> {
        themes.iter().map(|&theme| SubagentDot { theme }).collect()
    }

    fn cell_at(g: &SpiritGrid, c: u16, r: u16) -> Cell {
        g.cells[usize::from(r) * usize::from(g.cols) + usize::from(c)]
    }

    /// Lit sub-pixels (x 0..2, y 0..4) of a braille cell (empty for a non-braille glyph).
    fn cell_px(ch: char) -> BTreeSet<(i32, i32)> {
        let mut px = BTreeSet::new();
        if (0x2801..=0x28ff).contains(&(ch as u32)) {
            let bits = ch as u32 - 0x2800;
            for (dx, column) in BRAILLE_BITS.iter().enumerate() {
                for (dy, b) in column.iter().enumerate() {
                    if bits & b != 0 {
                        px.insert((dx as i32, dy as i32));
                    }
                }
            }
        }
        px
    }

    /// Cells of the first `n` beads.
    fn cells_of(n: usize) -> Vec<(u16, u16)> {
        (0..n).map(bead_cell).map(|(c, r)| (c as u16, r as u16)).collect()
    }

    /// Cells of `g` with anything drawn on them.
    fn lit(g: &SpiritGrid) -> Vec<(u16, u16)> {
        (0..g.rows).flat_map(|r| (0..g.cols).map(move |c| (c, r))).filter(|&(c, r)| cell_at(g, c, r) != BLANK).collect()
    }

    const BLANK: Cell = Cell { ch: ' ', fg: None };

    #[test]
    fn beads_fill_left_to_right_then_top_to_bottom() {
        let q = p(Theme::Fire, Status::Running, Size::XS, 3);
        let g = render_beads(&q, &dots(&[Theme::Water; 6]), 3);
        assert_eq!((g.cols, g.rows), (7, 3));
        assert_eq!(g.cells.len(), 21);
        let want = vec![(0, 0), (2, 0), (4, 0), (6, 0), (0, 1), (2, 1)];
        assert_eq!(cells_of(6), want);
        assert_eq!(lit(&g), want);
        assert!(want.iter().all(|&(c, r)| cell_at(&g, c, r).ch == BEAD));
    }

    #[test]
    fn a_bead_is_one_filled_cell() {
        assert_eq!(cell_px(BEAD), BTreeSet::from([(0, 1), (1, 1), (0, 2), (1, 2)]), "a 2 x 2 square");
    }

    #[test]
    fn each_subagent_adds_one_bead_up_to_what_the_rows_hold() {
        assert_eq!(MAX_DOTS, 16);
        for rows in ROWS {
            for status in [Status::Running, Status::NeedsInput, Status::Idle, Status::Unknown] {
                for tick in [0, 11, 29, 300] {
                    let q = p(Theme::Fire, status, Size::XS, tick);
                    for n in 0..=20 {
                        let g = render_beads(&q, &dots(&vec![Theme::Water; n]), rows);
                        let cells = cells_of(n.min(usize::from(rows) * 4));
                        assert_eq!(lit(&g), cells, "rows={rows} {status:?} n={n} t={tick}");
                        assert!(cells.iter().all(|&(c, r)| cell_at(&g, c, r).ch == BEAD), "{status:?} n={n} t={tick}");
                        let fgs: BTreeSet<_> = cells.iter().map(|&(c, r)| cell_at(&g, c, r).fg).collect();
                        assert!(status == Status::Running || fgs.len() <= 1, "same brightness: {fgs:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn no_more_than_max_dots_however_many_rows() {
        let q = p(Theme::Fire, Status::Running, Size::S, 3);
        let g = render_beads(&q, &dots(&[Theme::Water; 30]), 9);
        assert_eq!(lit(&g), cells_of(MAX_DOTS));
        assert!(lit(&render_beads(&q, &dots(&[Theme::Water; 3]), 0)).is_empty());
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

    const WANT: [(Theme, &str); 4] = [(Theme::Water, "blue"), (Theme::Poison, "green"), (Theme::Fire, "red"), (Theme::Lightning, "yellow")];

    #[test]
    fn bead_colours_follow_each_subagents_element_and_are_never_white() {
        for parent in Theme::ALL {
            for status in [Status::Running, Status::NeedsInput, Status::Idle, Status::Unknown] {
                for tick in [0, 9, 44, 123] {
                    let q = p(parent, status, Size::XS, tick);
                    let themes: Vec<Theme> = WANT.iter().map(|w| w.0).collect();
                    let g = render_beads(&q, &dots(&themes), 3);
                    for (i, (theme, colour)) in WANT.iter().enumerate() {
                        let (c, r) = cells_of(4)[i];
                        let fg = cell_at(&g, c, r).fg.expect("painted");
                        assert!(fg.0.max(fg.1).max(fg.2) <= 250, "never white: {fg:?}");
                        assert_eq!(dominant(fg), *colour, "{parent:?} {status:?} {theme:?} t={tick}: {fg:?}");
                    }
                }
            }
        }
    }

    fn dot_level(status: Status, tick: u64) -> u8 {
        let q = p(Theme::Water, status, Size::S, tick);
        let g = render_beads(&q, &dots(&[Theme::Water]), 3);
        let (c, r) = cells_of(1)[0];
        let fg = cell_at(&g, c, r).fg.unwrap();
        fg.0.max(fg.1).max(fg.2)
    }

    #[test]
    fn dots_dim_with_an_idle_parent_and_vanish_when_done() {
        for t in [0, 13, 40] {
            assert!(dot_level(Status::Idle, t) < dot_level(Status::NeedsInput, t), "t={t}");
            assert!(dot_level(Status::Unknown, t) < dot_level(Status::Running, t), "t={t}");
            assert!(dot_level(Status::Idle, t) < dot_level(Status::Running, t), "t={t}");
        }
        for rows in ROWS {
            for t in [0, 5, 77] {
                let q = p(Theme::Poison, Status::Done, Size::XS, t);
                assert!(lit(&render_beads(&q, &dots(&[Theme::Water, Theme::Fire]), rows)).is_empty(), "no dots when done");
            }
        }
        // running pulses: the brightness changes over ticks
        let levels: BTreeSet<u8> = (0..40).map(|t| dot_level(Status::Running, t)).collect();
        assert!(levels.len() > 3, "{levels:?}");
    }

    /// Colours of three same-element beads at `tick`.
    fn fgs_at(status: Status, tick: u64) -> Vec<Option<(u8, u8, u8)>> {
        let g = render_beads(&p(Theme::Fire, status, Size::XS, tick), &dots(&[Theme::Water; 3]), 3);
        cells_of(3).iter().map(|&(c, r)| cell_at(&g, c, r).fg).collect()
    }

    #[test]
    fn beads_stay_still_at_every_status_and_tick() {
        for status in [Status::Running, Status::NeedsInput, Status::Idle, Status::Unknown] {
            for tick in 0..200 {
                let g = render_beads(&p(Theme::Fire, status, Size::XS, tick), &dots(&[Theme::Water; 3]), 3);
                for (c, r) in cells_of(3) {
                    assert_eq!(cell_at(&g, c, r).ch, '⠶', "{status:?} t={tick}");
                }
            }
        }
    }

    #[test]
    fn running_beads_twinkle_each_on_its_own() {
        assert!((0..60).any(|t| {
            let f = fgs_at(Status::Running, t);
            f[0] != f[1] && f[1] != f[2]
        }));
        for i in 0..3 {
            let seen: BTreeSet<_> = (0..60).map(|t| fgs_at(Status::Running, t)[i]).collect();
            assert!(seen.len() > 3, "bead {i}: {seen:?}");
        }
    }

    #[test]
    fn waiting_beads_pulse_together() {
        for status in [Status::NeedsInput, Status::Idle] {
            for t in 0..120 {
                let f = fgs_at(status, t);
                assert!(f[0] == f[1] && f[1] == f[2], "{status:?} t={t}: {f:?}");
            }
            let seen: BTreeSet<_> = (0..120).map(|t| fgs_at(status, t)[0]).collect();
            assert!(seen.len() > 2, "{status:?}: {seen:?}");
        }
    }

    #[test]
    fn idle_shimmer_is_faint_and_unknown_is_steady() {
        let levels: Vec<u8> = (0..200).map(|t| dot_level(Status::Idle, t)).collect();
        let (lo, hi) = (levels.iter().min().unwrap(), levels.iter().max().unwrap());
        assert!(f32::from(*hi) / f32::from(*lo) <= 1.3, "{lo}..{hi}");
        let seen: BTreeSet<_> = (0..200).map(|t| fgs_at(Status::Unknown, t)[0]).collect();
        assert_eq!(seen.len(), 1, "{seen:?}");
    }

    #[test]
    fn dots_are_deterministic_and_never_panic() {
        for rows in [0, 1, 3, 4, 9] {
            for status in STATUSES {
                for tick in [0, 1, u64::MAX, u64::MAX - 1, 1 << 40] {
                    for stage in [Stage::Relaxed, Stage::Lively, Stage::Struggling] {
                        let q = SpiritParams { stage, warning: true, ..p(Theme::Lightning, status, Size::S, tick) };
                        let d = dots(&[Theme::Water, Theme::Poison, Theme::Fire, Theme::Lightning, Theme::Water, Theme::Fire]);
                        let a = render_beads(&q, &d, rows);
                        assert_eq!(a, render_beads(&q, &d, rows));
                        assert_eq!(a.cells.len(), usize::from(a.cols) * usize::from(a.rows));
                    }
                }
            }
        }
    }
}
