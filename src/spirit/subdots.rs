//! Subagent dots: one bead per running subagent in the spirit band, in
//! ONE row on the band's bottom row (never the eye row), starting at the
//! body's lower right and growing to the right: bead 1 nearest the body.
//!
//! Each bead is a plus: a 3 x 3 sub-pixel block minus its four corners,
//! spanning TWO braille cells (`⠐⠗`: the left arm in the left cell's right
//! column, the rest in the right cell's left column plus the right arm).
//! Both cells take the same colour. Two beads are separated by exactly one
//! empty cell, so N beads span 3N-1 cells and a count is never ambiguous.
//! The cells between the body and the first bead and after the last bead
//! are kept empty too.
//!
//! Colour: the subagent's element (bright base tone with its lighter core
//! blended in, as a braille cell has one colour), never white; all beads
//! share the same brightness: in sync pulsing while the parent runs, dimmed
//! like the body when it is idle or unknown, gone when it is done. The bob
//! only moves the whole plus one sub-pixel down inside its cells (`⠠⡦`,
//! rows 1..3; never x), so it stays a whole plus.
//!
//! The beads are painted on the finished cell grid, so they always win their
//! cells (and the gap cells) over the body, the water ripple and the magic.
//! At most [`MAX_DOTS`], and only as many whole beads as fit in the card's
//! columns ([`fit`]); the text counts the rest.

use super::canvas::{lerp, mul, Geom, Rgb, BRAILLE_BITS};
use super::theme::Theme;
use super::{element, state, Cell, Ctx, SpiritGrid, BASE_R};
use crate::model::Status;

/// At most this many dots; further running subagents are only counted in the text.
pub const MAX_DOTS: usize = 4;
/// Cells of one bead.
const WIDTH: i32 = 2;
/// Columns from one bead to the next: the two bead cells and one empty cell.
const PITCH: i32 = WIDTH + 1;
/// Ticks each dot lags behind the body's bob (times its index + 1).
const LAG: f32 = 4.0;
/// The bead's sub-pixels over its two cells (x 0..4, the right cell from
/// x 2; y 0..4): a plus, a 3 x 3 block minus its corners, centred on x 2.
const BEAD: [(i32, i32); 5] = [(2, 0), (1, 1), (2, 1), (3, 1), (2, 2)];

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

/// Left cell column of bead `i`: one empty cell after the body, then a
/// bead every [`PITCH`] cells.
fn bead_col(g: &Geom, i: usize) -> i32 {
    first_col(g) + 1 + PITCH * i as i32
}

/// How many of `n` dots are drawn: at most [`MAX_DOTS`] and, when the card
/// gives the spirit `cols` columns (0: no limit), as many whole beads as fit
/// in them (bead `i` ends at column `first_col + PITCH * (i + 1) - 1`).
pub(super) fn fit(g: &Geom, cols: u16, n: usize) -> usize {
    let n = n.min(MAX_DOTS);
    if cols == 0 {
        return n;
    }
    let room = (i32::from(cols) - first_col(g)).max(0) / PITCH;
    n.min(room as usize)
}

/// Left grid columns of the first `n` beads of a spirit of `size`.
#[cfg(test)]
pub(super) fn dot_cols(size: super::Size, n: usize) -> Vec<u16> {
    let g = size.geom();
    (0..n.min(MAX_DOTS)).map(|i| bead_col(&g, i) as u16).collect()
}

/// Sub-pixel row offset of bead `i` inside its cells (0 or 1): the body's
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

/// Braille glyphs (left cell, right cell) of a bead `dy` sub-pixels down.
fn glyphs(dy: i32) -> (char, char) {
    let cell = |c: i32| {
        let bits = BEAD.iter().filter(|&&(x, _)| x / 2 == c).fold(0u32, |b, &(x, y)| b | BRAILLE_BITS[(x % 2) as usize][(y + dy) as usize]);
        char::from_u32(0x2800 + bits).unwrap_or(' ')
    };
    (cell(0), cell(1))
}

/// Paints the first `n` dots on the bottom row of the finished grid (none
/// when the parent is done), clearing the cells before and after each bead.
pub(super) fn paint(grid: &mut SpiritGrid, g: &Geom, ctx: &Ctx, dots: &[SubagentDot], n: usize) {
    let n = n.min(dots.len());
    if ctx.state == Status::Done || n == 0 {
        return;
    }
    widen(grid, (bead_col(g, n - 1) + WIDTH).max(0) as u16);
    let row = i32::from(grid.rows) - 1;
    for (i, d) in dots.iter().take(n).enumerate() {
        let col = bead_col(g, i);
        let ((l, r), fg) = (glyphs(bob_dy(ctx, i)), colour(ctx, d.theme));
        clear(grid, col - 1, row);
        grid.put(col, row, l, fg);
        grid.put(col + 1, row, r, fg);
        clear(grid, col + WIDTH, row);
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
    /// The two cells of a bead: at rest and one sub-pixel lower (bobbing).
    const UP: (char, char) = ('⠐', '⠗');
    const DOWN: (char, char) = ('⠠', '⡦');

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

    /// Bottom-row cell `c`, blank outside the grid (the card draws nothing there).
    fn bottom(g: &SpiritGrid, c: u16) -> Cell {
        if c < g.cols {
            cell_at(g, c, g.rows - 1)
        } else {
            BLANK
        }
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

    /// Whether bottom-row cells `c` and `c + 1` hold one bead (either bob
    /// offset) in one colour.
    fn is_bead(g: &SpiritGrid, c: u16) -> bool {
        let (l, r) = (bottom(g, c), bottom(g, c + 1));
        ((l.ch, r.ch) == UP || (l.ch, r.ch) == DOWN) && l.fg.is_some() && l.fg == r.fg
    }

    /// Left columns of every bead on the bottom row.
    fn beads(g: &SpiritGrid) -> Vec<u16> {
        (0..g.cols).filter(|&c| is_bead(g, c)).collect()
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
    fn a_bead_is_a_plus_across_two_cells() {
        for (dy, (l, r)) in [(0, UP), (1, DOWN)] {
            assert_eq!(glyphs(dy), (l, r));
            // left cell x 0..2, right cell x 2..4: a 3 x 3 block minus its corners
            let px: BTreeSet<(i32, i32)> = cell_px(l).into_iter().chain(cell_px(r).into_iter().map(|(x, y)| (x + 2, y))).collect();
            let plus: BTreeSet<(i32, i32)> = [(2, 0), (1, 1), (2, 1), (3, 1), (2, 2)].iter().map(|&(x, y)| (x, y + dy)).collect();
            assert_eq!(px, plus, "dy={dy}");
        }
    }

    /// Bottom row from the empty cell right of the body, bob folded to the
    /// rest glyphs, for `n` requested beads in `cols` columns (table below).
    fn row_from_body(size: Size, cols: u16, n: usize, status: Status, magic: Magic, stage: Stage, tick: u64, len: usize) -> (String, SpiritGrid) {
        let q = SpiritParams { cols, magic, stage, ..p(Theme::Fire, status, size, tick) };
        let g = render_dots(&q, &dots(&vec![Theme::Water; n]));
        let c0 = first_col(&size.geom()) as u16;
        let mut s = String::new();
        let mut c = c0;
        while s.chars().count() < len {
            if is_bead(&g, c) && bottom(&g, c).ch == DOWN.0 {
                s.push(UP.0);
                s.push(UP.1);
                c += 2;
            } else {
                s.push(bottom(&g, c).ch);
                c += 1;
            }
        }
        (s, g)
    }

    #[test]
    fn exact_bottom_rows_for_one_to_four_beads_capped_and_fitted() {
        // (size, card columns (0: no limit), running subagents, drawn, cells from the empty one right of the body)
        let table: &[(Size, u16, usize, usize, &str)] = &[
            (Size::XS, STD_COLS, 1, 1, " ⠐⠗ "),
            (Size::XS, STD_COLS, 2, 2, " ⠐⠗ ⠐⠗ "),
            (Size::XS, STD_COLS, 3, 3, " ⠐⠗ ⠐⠗ ⠐⠗ "),
            (Size::XS, STD_COLS, 4, 4, " ⠐⠗ ⠐⠗ ⠐⠗ ⠐⠗"),
            (Size::XS, STD_COLS, 5, 4, " ⠐⠗ ⠐⠗ ⠐⠗ ⠐⠗"),
            (Size::XS, STD_COLS, 9, 4, " ⠐⠗ ⠐⠗ ⠐⠗ ⠐⠗"),
            (Size::XS, 0, 7, 4, " ⠐⠗ ⠐⠗ ⠐⠗ ⠐⠗"),
            (Size::S, LARGE_COLS, 1, 1, " ⠐⠗ "),
            (Size::S, LARGE_COLS, 2, 2, " ⠐⠗ ⠐⠗ "),
            (Size::S, LARGE_COLS, 3, 3, " ⠐⠗ ⠐⠗ ⠐⠗ "),
            (Size::S, LARGE_COLS, 4, 4, " ⠐⠗ ⠐⠗ ⠐⠗ ⠐⠗ "),
            (Size::S, LARGE_COLS, 6, 4, " ⠐⠗ ⠐⠗ ⠐⠗ ⠐⠗ "),
            // narrower bands: only whole beads that fit
            (Size::XS, 16, 4, 3, " ⠐⠗ ⠐⠗ ⠐⠗ "),
            (Size::XS, 12, 4, 2, " ⠐⠗ ⠐⠗ "),
            (Size::XS, 10, 4, 1, " ⠐⠗ "),
            (Size::XS, 8, 3, 1, " ⠐⠗"),
            (Size::XS, 7, 2, 0, ""),
            (Size::S, 12, 4, 2, " ⠐⠗ ⠐⠗"),
            (Size::S, 11, 4, 1, " ⠐⠗ "),
            (Size::S, 8, 1, 0, ""),
        ];
        for &(size, cols, n, drawn, want) in table {
            for status in [Status::Running, Status::NeedsInput, Status::Idle, Status::Unknown] {
                for magic in Magic::ALL {
                    for stage in [Stage::Relaxed, Stage::Lively, Stage::Struggling] {
                        for tick in [0, 11, 29, 300] {
                            let at = format!("{size:?} cols={cols} n={n} {status:?} {magic:?} {stage:?} t={tick}");
                            let (got, g) = row_from_body(size, cols, n, status, magic, stage, tick, want.chars().count());
                            assert_eq!(got, want, "{at}");
                            let found = beads(&g);
                            assert_eq!(found.len(), drawn, "{at}: {found:?}");
                            assert_eq!(found, dot_cols(size, drawn), "{at}");
                            assert!(found.windows(2).all(|w| w[1] == w[0] + 3), "{at}: one empty cell between beads");
                            if cols > 0 {
                                assert!(found.iter().all(|&c| c + 1 < cols), "{at}: whole beads inside the columns");
                            }
                            if drawn == 0 {
                                let q = SpiritParams { cols, magic, stage, ..p(Theme::Fire, status, size, tick) };
                                assert_eq!(g, render(&q), "{at}: none fit, plain frame");
                            } else {
                                // nothing above the bottom row changes
                                let q = SpiritParams { cols, magic, stage, ..p(Theme::Fire, status, size, tick) };
                                let plain = render(&q);
                                for r in 0..g.rows - 1 {
                                    for c in 0..g.cols {
                                        let old = if c < plain.cols { cell_at(&plain, c, r) } else { BLANK };
                                        assert_eq!(cell_at(&g, c, r), old, "{at}: row {r} col {c}");
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        assert_eq!(MAX_DOTS, 4);
    }

    #[test]
    fn four_beads_fit_the_standard_and_large_cards() {
        assert_eq!(fit(&Size::XS.geom(), STD_COLS, 9), 4);
        assert_eq!(fit(&Size::S.geom(), LARGE_COLS, 9), 4);
        assert_eq!(fit(&Size::XS.geom(), 0, 9), 4);
        // a bead's two cells end inside the band
        assert_eq!(dot_cols(Size::XS, 4), vec![6, 9, 12, 15]);
        assert!(dot_cols(Size::XS, 4)[3] + 1 < STD_COLS);
        assert_eq!(dot_cols(Size::S, 4), vec![7, 10, 13, 16]);
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
                        let c = dot_cols(size, 4)[i];
                        let fg = cell_at(&g, c, g.rows - 1).fg.expect("painted");
                        assert_eq!(cell_at(&g, c + 1, g.rows - 1).fg, Some(fg), "one colour across the bead");
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
                        let cols = dot_cols(size, 4);
                        assert_eq!(beads(&g), cols, "{parent:?} {magic:?} {size:?} t={tick}");
                        for (i, (_, colour)) in WANT.iter().enumerate() {
                            let fg = cell_at(&g, cols[i], g.rows - 1).fg.unwrap();
                            assert_eq!(dominant(fg), *colour, "{parent:?} {magic:?} t={tick}");
                            // the empty cell before each bead stays empty
                            assert_eq!(cell_at(&g, cols[i] - 1, g.rows - 1), BLANK, "{parent:?} {magic:?} t={tick}: gap before bead {i}");
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
        let glyphs = |stage: Stage, status: Status| -> Vec<Vec<(char, char)>> {
            (0..120)
                .map(|t| {
                    let q = SpiritParams { stage, ..p(Theme::Fire, status, Size::S, t) };
                    let g = render_dots(&q, &dots(&[Theme::Water; 3]));
                    dot_cols(Size::S, 3).iter().map(|&c| (cell_at(&g, c, g.rows - 1).ch, cell_at(&g, c + 1, g.rows - 1).ch)).collect()
                })
                .collect()
        };
        let lively = glyphs(Stage::Lively, Status::NeedsInput);
        assert!(lively.iter().flatten().all(|&b| b == UP || b == DOWN), "always a whole plus");
        let distinct: BTreeSet<(char, char)> = lively.iter().map(|f| f[0]).collect();
        assert!(distinct.len() >= 2, "the dot bobs: {distinct:?}");
        assert!(lively.iter().any(|f| f[0] != f[1] || f[1] != f[2]), "dots are offset from each other");
        // struggling moves more often than relaxed
        let moves = |frames: &[Vec<(char, char)>]| frames.windows(2).filter(|w| w[0] != w[1]).count();
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
                                if cols > 0 && status != Status::Done {
                                    assert!(beads(&a).iter().all(|&c| c + 1 < cols), "{size:?} cols={cols}: whole beads only");
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
