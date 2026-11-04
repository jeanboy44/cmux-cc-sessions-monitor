//! Subagent dots: one bead per running subagent in the spirit band, in
//! ONE row on the band's bottom row (never the eye row), starting at the
//! body's lower right and growing to the right: bead 1 nearest the body.
//!
//! Each bead is EXACTLY ONE braille cell holding a 2 x 2
//! square of sub-pixels (`⠶`, or `⣤` one sub-pixel lower while it bobs), and two beads are
//! separated by exactly one empty cell, so N beads span 2N-1 cells and a
//! count is never ambiguous (two beads are two separate cells, never one
//! wider shape). The cells between the body and the first bead and after
//! the last bead are kept empty too.
//!
//! Colour: the subagent's element (bright base tone with its lighter core
//! blended in, as a braille cell has one colour), never white; all beads
//! share the same brightness: in sync pulsing while the parent runs, dimmed
//! like the body when it is idle or unknown, gone when it is done. The bob
//! only moves a bead one sub-pixel down inside its own cell (never x).
//!
//! The beads are painted on the finished cell grid, so they always win their
//! cells (and the gap cells) over the body, the water ripple and the magic.
//! At most [`MAX_DOTS`], and only as many as fit in the card's columns
//! ([`fit`]); the text counts the rest.

use super::canvas::{lerp, mul, Geom, Rgb, BRAILLE_BITS};
use super::theme::Theme;
use super::{element, state, Cell, Ctx, SpiritGrid, BASE_R};
use crate::model::Status;

/// At most this many dots; further running subagents are only counted in the text.
pub const MAX_DOTS: usize = 5;
/// Columns from one bead to the next: the bead cell and one empty cell.
const PITCH: i32 = 2;
/// Ticks each dot lags behind the body's bob (times its index + 1).
const LAG: f32 = 4.0;
/// The bead's sub-pixels inside its cell (x 0..2, y 0..3): a 2 x 2 square on rows 1..3.
const BEAD: [(i32, i32); 4] = [(0, 1), (1, 1), (0, 2), (1, 2)];

/// One running subagent, drawn in its element's colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SubagentDot {
    pub theme: Theme,
}

/// The cell right of where the round body meets the top of the bottom cell
/// row (kept empty; the first bead sits right of it).
fn first_col(g: &Geom) -> i32 {
    let r = BASE_R * g.ppu;
    let dy = (g.h as f32 - 4.0 - g.rcy).max(0.0);
    let dx = (r * r - dy * dy).max(0.0).sqrt();
    ((g.rcx + dx) / 2.0).floor() as i32 + 1
}

/// Cell column of bead `i`: one empty cell after the body, then a bead
/// every [`PITCH`] cells.
fn bead_col(g: &Geom, i: usize) -> i32 {
    first_col(g) + 1 + PITCH * i as i32
}

/// How many of `n` dots are drawn: at most [`MAX_DOTS`] and, when the card
/// gives the spirit `cols` columns (0: no limit), as many as fit in them.
pub(super) fn fit(g: &Geom, cols: u16, n: usize) -> usize {
    let n = n.min(MAX_DOTS);
    if cols == 0 {
        return n;
    }
    let room = (i32::from(cols) - first_col(g)).max(0) / PITCH;
    n.min(room as usize)
}

/// Grid columns of the first `n` beads of a spirit of `size`.
#[cfg(test)]
pub(super) fn dot_cols(size: super::Size, n: usize) -> Vec<u16> {
    let g = size.geom();
    (0..n.min(MAX_DOTS)).map(|i| bead_col(&g, i) as u16).collect()
}

/// Sub-pixel row offset of bead `i` inside its cell (0 or 1): the body's
/// bob, `LAG` ticks later per index.
fn bob_dy(ctx: &Ctx, i: usize) -> i32 {
    let bob = state::bob_at(ctx.stage, ctx.state, ctx.seed, ctx.tf - LAG * (i as f32 + 1.0));
    i32::from(bob >= 0.0)
}

/// Colour of a bead of element `theme`: rim and a lighter core, averaged;
/// pulsing (all beads in sync) while running, dimmed otherwise.
fn colour(ctx: &Ctx, theme: Theme) -> Rgb {
    let ramp = element::ramp(theme);
    let rim = match ctx.state {
        Status::Running => lerp(ramp[1], ramp[2], 0.5 + 0.5 * (ctx.tf * 0.35).sin()),
        _ => lerp(ramp[1], ramp[2], 0.5),
    };
    let core = lerp(rim, ramp[3], 0.35);
    let avg = [(3.0 * rim[0] + core[0]) / 4.0, (3.0 * rim[1] + core[1]) / 4.0, (3.0 * rim[2] + core[2]) / 4.0];
    match ctx.state {
        Status::Running => avg,
        s => mul(avg, state::brightness(s, state::pulse(s, ctx.tf))),
    }
}

/// Braille glyph of a bead `dy` sub-pixels down its cell.
fn glyph(dy: i32) -> char {
    let bits = BEAD.iter().fold(0u32, |b, &(x, y)| b | BRAILLE_BITS[x as usize][(y + dy) as usize]);
    char::from_u32(0x2800 + bits).unwrap_or(' ')
}

/// Paints the first `n` dots on the bottom row of the finished grid (none
/// when the parent is done), clearing the cells before and after each bead.
pub(super) fn paint(grid: &mut SpiritGrid, g: &Geom, ctx: &Ctx, dots: &[SubagentDot], n: usize) {
    let n = n.min(dots.len());
    if ctx.state == Status::Done || n == 0 {
        return;
    }
    widen(grid, (bead_col(g, n - 1) + 1).max(0) as u16);
    let row = i32::from(grid.rows) - 1;
    for (i, d) in dots.iter().take(n).enumerate() {
        let col = bead_col(g, i);
        clear(grid, col - 1, row);
        grid.put(col, row, glyph(bob_dy(ctx, i)), colour(ctx, d.theme));
        clear(grid, col + 1, row);
    }
}

/// Pads the grid with empty columns up to `cols` (the frame itself is
/// rendered at its own width, so nothing else moves).
fn widen(grid: &mut SpiritGrid, cols: u16) {
    if cols <= grid.cols {
        return;
    }
    let blank = Cell { ch: ' ', fg: None };
    let mut cells = Vec::with_capacity(usize::from(cols) * usize::from(grid.rows));
    for row in grid.cells.chunks(usize::from(grid.cols).max(1)).take(usize::from(grid.rows)) {
        cells.extend_from_slice(row);
        cells.extend(std::iter::repeat_n(blank, usize::from(cols - grid.cols)));
    }
    grid.cols = cols;
    grid.cells = cells;
}

fn clear(grid: &mut SpiritGrid, col: i32, row: i32) {
    if (0..i32::from(grid.cols)).contains(&col) && (0..i32::from(grid.rows)).contains(&row) {
        grid.cells[row as usize * usize::from(grid.cols) + col as usize] = Cell { ch: ' ', fg: None };
    }
}

#[cfg(test)]
mod tests {
    use super::super::*;
    use super::*;
    use crate::model::Status;
    use std::collections::BTreeSet;

    const STATUSES: [Status; 5] = [Status::Running, Status::NeedsInput, Status::Idle, Status::Done, Status::Unknown];
    /// Columns the standard (XS) and large (S) cards give the spirit.
    const STD_COLS: u16 = 17;
    const LARGE_COLS: u16 = 29;

    fn p(theme: Theme, status: Status, size: Size, tick: u64) -> SpiritParams {
        let cols = if size == Size::XS { STD_COLS } else { LARGE_COLS };
        SpiritParams { theme, status, size, tick, seed: 7, warning: false, magic: Magic::Normal, stage: Stage::Lively, cols }
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

    /// Whether `ch` is the bead (either bob offset).
    fn is_bead(ch: char) -> bool {
        let px = cell_px(ch);
        (0..2).any(|dy| px == BEAD.iter().map(|&(x, y)| (x, y + dy)).collect())
    }

    /// Columns of the bead cells found on the bottom row in the span `n`
    /// beads reserve (2n-1 cells and the empty one after them; the body's
    /// own edge left of it, and magic further right, may use the same glyph).
    fn beads_in(g: &SpiritGrid, size: Size, n: usize) -> Vec<u16> {
        let c0 = dot_cols(size, 1)[0];
        (c0..(c0 + 2 * n as u16).min(g.cols)).filter(|&c| is_bead(cell_at(g, c, g.rows - 1).ch)).collect()
    }

    const BLANK: Cell = Cell { ch: ' ', fg: None };

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
    fn a_bead_is_one_filled_cell() {
        for dy in 0..2 {
            let px = cell_px(glyph(dy));
            assert_eq!(px.len(), 4, "a 2 x 2 square");
            assert!(px.iter().all(|&(_, y)| (dy + 1..dy + 3).contains(&y)));
            let mirror: BTreeSet<(i32, i32)> = px.iter().map(|&(x, y)| (1 - x, y)).collect();
            assert_eq!(mirror, px, "symmetric left-right");
        }
        assert_eq!((glyph(0), glyph(1)), ('⠶', '⣤'));
    }

    #[test]
    fn one_row_of_beads_one_cell_each_growing_right_from_the_body() {
        for (size, cols, max_fit) in [(Size::S, LARGE_COLS, 5), (Size::XS, STD_COLS, 5), (Size::XS, 0, 5), (Size::S, 12, 3), (Size::XS, 9, 2), (Size::S, 60, 5)] {
            for status in [Status::Running, Status::NeedsInput, Status::Idle, Status::Unknown] {
                for tick in [0, 11, 29, 300] {
                    let mut prev: Vec<u16> = Vec::new();
                    for n in 1..=7 {
                        let q = SpiritParams { cols, ..p(Theme::Fire, status, size, tick) };
                        let g = render_dots(&q, &dots(&vec![Theme::Water; n]));
                        let want = n.min(max_fit);
                        let found = beads_in(&g, size, want);
                        assert_eq!(found.len(), want, "{size:?} cols={cols} {status:?} n={n} t={tick}: {found:?}");
                        assert_eq!(found, dot_cols(size, want), "{size:?} cols={cols}");
                        assert!(found.windows(2).all(|w| w[1] == w[0] + 2), "exactly two columns apart: {found:?}");
                        assert!(found.starts_with(&prev), "the row grows to the right: {prev:?} -> {found:?}");
                        prev = found.clone();
                        // N beads span 2N-1 cells: the cells between (and right after) are empty
                        let (first, last) = (found[0], found[want - 1]);
                        assert_eq!(cell_at(&g, first - 1, g.rows - 1), BLANK, "an empty cell between the body and the first bead");
                        for c in first..=(last + 1).min(g.cols - 1) {
                            let cell = cell_at(&g, c, g.rows - 1);
                            if (c - first) % 2 == 1 {
                                assert_eq!(cell, BLANK, "{size:?} n={n}: gap {c}");
                            } else {
                                assert!(is_bead(cell.ch));
                            }
                        }
                        // the same pattern and colour in every bead (one theme here)
                        let fgs: BTreeSet<_> = found.iter().map(|&c| cell_at(&g, c, g.rows - 1).fg).collect();
                        assert_eq!(fgs.len(), 1, "same brightness: {fgs:?}");
                        for &c in &found {
                            assert!(f32::from(c) > body_center_x(size), "right of the body centre ({c})");
                            if cols > 0 {
                                assert!(c < cols, "inside the card's columns");
                            }
                        }
                        // nothing above the bottom row changes
                        let plain = render(&q);
                        for r in 0..g.rows - 1 {
                            for c in 0..g.cols {
                                let old = if c < plain.cols { cell_at(&plain, c, r) } else { BLANK };
                                assert_eq!(cell_at(&g, c, r), old, "{size:?} n={n}: row {r} col {c}");
                            }
                        }
                    }
                }
            }
        }
        assert_eq!(MAX_DOTS, 5);
    }

    #[test]
    fn two_beads_are_never_confused_with_one() {
        for size in [Size::XS, Size::S] {
            for tick in 0..80 {
                for magic in Magic::ALL {
                    let q = SpiritParams { magic, ..p(Theme::Water, Status::Running, size, tick) };
                    let (one, two) = (render_dots(&q, &dots(&[Theme::Fire])), render_dots(&q, &dots(&[Theme::Fire; 2])));
                    let row = |g: &SpiritGrid| -> Vec<Cell> { (0..g.cols).map(|c| cell_at(g, c, g.rows - 1)).collect() };
                    let c0 = usize::from(dot_cols(size, 1)[0]);
                    // one: a bead with an empty cell after it; two: bead, empty, bead, empty
                    assert!(is_bead(row(&one)[c0].ch) && row(&one)[c0 + 1] == BLANK);
                    let r2 = row(&two);
                    assert!(is_bead(r2[c0].ch) && r2[c0 + 1] == BLANK && is_bead(r2[c0 + 2].ch), "{size:?} t={tick}");
                    if c0 + 3 < r2.len() {
                        assert_eq!(r2[c0 + 3], BLANK);
                    }
                    assert_ne!(row(&one)[c0 + 2], r2[c0 + 2], "{size:?} t={tick}: the second bead is its own cell");
                    assert_eq!(beads_in(&one, size, 1).len(), 1);
                    assert_eq!(beads_in(&two, size, 2).len(), 2);
                }
            }
        }
    }

    #[test]
    fn beads_stay_off_the_eye_rows_and_in_the_lower_half() {
        for size in [Size::XS, Size::S] {
            let cv = canvas::Canvas::new(size.geom());
            let rows = grid_size(size).1;
            for (_, r0, _, h) in eyes::boxes(&cv) {
                assert!(r0 + h - 1 < i32::from(rows) - 1, "{size:?}: eyes above the bead row");
            }
            assert!(rows - 1 >= rows / 2);
        }
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
        for size in [Size::XS, Size::S] {
            for status in [Status::Running, Status::NeedsInput, Status::Idle, Status::Unknown] {
                for tick in [0, 9, 44, 123] {
                    let q = p(Theme::Fire, status, size, tick);
                    let themes: Vec<Theme> = WANT.iter().map(|w| w.0).collect();
                    let g = render_dots(&q, &dots(&themes));
                    for (i, (theme, colour)) in WANT.iter().enumerate() {
                        let fg = cell_at(&g, dot_cols(size, 4)[i], g.rows - 1).fg.expect("painted");
                        assert!(fg.0.max(fg.1).max(fg.2) <= 250, "never white: {fg:?}");
                        assert_eq!(dominant(fg), *colour, "{size:?} {status:?} {theme:?} t={tick}: {fg:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn beads_win_over_magic_and_water() {
        for parent in Theme::ALL {
            for magic in Magic::ALL {
                for size in [Size::XS, Size::S] {
                    for tick in 0..60 {
                        let q = SpiritParams { magic, stage: Stage::Struggling, ..p(parent, Status::Running, size, tick) };
                        let themes: Vec<Theme> = WANT.iter().map(|w| w.0).chain([Theme::Water]).collect();
                        let g = render_dots(&q, &dots(&themes));
                        let n = 5;
                        assert_eq!(beads_in(&g, size, n), dot_cols(size, n), "{parent:?} {magic:?} {size:?} t={tick}");
                        for (i, (_, colour)) in WANT.iter().enumerate() {
                            let fg = cell_at(&g, dot_cols(size, n)[i], g.rows - 1).fg.unwrap();
                            assert_eq!(dominant(fg), *colour, "{parent:?} {magic:?} t={tick}");
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
        let levels: BTreeSet<u8> = (0..40).map(|t| dot_level(Status::Running, t)).collect();
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
        let distinct: BTreeSet<char> = lively.iter().map(|f| f[0]).collect();
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
                            for cols in [0, 1, 5, 17, 29, 80] {
                                let q = SpiritParams { magic, stage, cols, warning: true, ..p(Theme::Lightning, status, size, tick) };
                                let d = dots(&[Theme::Water, Theme::Poison, Theme::Fire, Theme::Lightning, Theme::Water, Theme::Fire]);
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
}
