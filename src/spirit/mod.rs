//! Element spirits: a small braille creature per session card.
//!
//! [`render`] is a pure function: the spirit is painted procedurally on a
//! sub-pixel canvas (2 x 4 braille dots per terminal cell) and returned as a
//! grid of braille cells with 24-bit foreground colours. Cells never carry a
//! background colour. The same parameters always give the same grid; motion
//! comes from `tick` (about 15 per second).
//!
//! - Element ([`Theme`]): palette, outline (fire tongues, water drop, ice
//!   facets, lightning spikes), decorations and particles.
//! - State ([`Status`]): RUNNING shoots element particles to the right,
//!   NEEDS_INPUT shows a bobbing `?` and a pulsing glow, IDLE is dim and slow
//!   with a `z`, DONE sparkles and bounces, UNKNOWN is grey.
//! - Eyes are the same in every state (see `eyes`).

mod body;
mod canvas;
mod element;
mod eyes;
mod noise;
mod particles;
mod state;
pub mod theme;

use crate::model::Status;
use canvas::Geom;
use theme::Theme;

/// Design space (scale x1 canvas in sub-pixels): 17 x 7 terminal cells.
const DW: f32 = 34.0;
const DH: f32 = 28.0;
/// Body centre / radius in design units.
const CX: f32 = 11.0;
const CY: f32 = 15.0;
const BASE_R: f32 = 8.5;
/// Water ripple line.
const FLOOR_Y: f32 = 26.0;
/// Ticks per half period of the red warning flicker (about 0.5 s at 15 fps).
const WARN_HALF_PERIOD: u64 = 8;

/// Spirit size step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Size {
    S,
    M,
    L,
}

impl Size {
    pub const ALL: [Size; 3] = [Size::S, Size::M, Size::L];

    const fn geom(self) -> Geom {
        match self {
            // 11 x 5 cells, body radius 5.5 sub-pixels
            Size::S => Geom { ppu: 5.5 / BASE_R, w: 22, h: 20, rcx: 7.0, rcy: 11.0, small: true },
            // 14 x 6 cells, body radius 7 sub-pixels
            Size::M => Geom { ppu: 7.0 / BASE_R, w: 28, h: 24, rcx: 9.0, rcy: 13.0, small: true },
            // 17 x 7 cells: the design space at scale x1
            Size::L => Geom { ppu: 1.0, w: 34, h: 28, rcx: CX, rcy: CY, small: false },
        }
    }
}

/// Grid size (columns, rows) of a spirit of `size`.
pub const fn grid_size(size: Size) -> (u16, u16) {
    let g = size.geom();
    ((g.w / 2) as u16, (g.h / 4) as u16)
}

/// Grid size of the largest spirit (cards reserve this much room).
pub const fn max_grid_size() -> (u16, u16) {
    let (mut w, mut h, mut i) = (0, 0, 0);
    while i < Size::ALL.len() {
        let (cw, ch) = grid_size(Size::ALL[i]);
        if cw > w {
            w = cw;
        }
        if ch > h {
            h = ch;
        }
        i += 1;
    }
    (w, h)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpiritParams {
    pub theme: Theme,
    pub status: Status,
    pub size: Size,
    /// animation frame (about 15 per second)
    pub tick: u64,
    /// per-session variation (phases, blinks, particle streams)
    pub seed: u32,
    /// context nearly full: the body flickers red
    pub warning: bool,
}

/// One terminal cell: a braille (or mark) glyph and its foreground colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cell {
    pub ch: char,
    pub fg: Option<(u8, u8, u8)>,
}

/// Row-major cells, `cols` x `rows`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpiritGrid {
    pub cols: u16,
    pub rows: u16,
    pub cells: Vec<Cell>,
}

impl SpiritGrid {
    /// Overwrites the cell at (col, row) with a text glyph; ignored outside the grid.
    fn put(&mut self, col: i32, row: i32, ch: char, fg: canvas::Rgb) {
        if col < 0 || row < 0 || col >= i32::from(self.cols) || row >= i32::from(self.rows) {
            return;
        }
        let i = row as usize * usize::from(self.cols) + col as usize;
        self.cells[i] = Cell { ch, fg: Some(canvas::to_rgb(fg, 250.0)) };
    }
}

/// Per-frame values shared by the painters.
struct Ctx {
    elem: Theme,
    state: Status,
    seed: u32,
    t: u64,
    tf: f32,
    /// outline animation speed of the state
    sp: f32,
    small: bool,
}

impl Ctx {
    fn new(p: &SpiritParams) -> Self {
        Ctx {
            elem: p.theme,
            state: p.status,
            seed: p.seed,
            t: p.tick,
            tf: p.tick as f32,
            sp: state::speed(p.status),
            small: p.size.geom().small,
        }
    }
}

/// Renders one frame of a spirit.
pub fn render(p: &SpiritParams) -> SpiritGrid {
    let g = p.size.geom();
    let ctx = Ctx::new(p);
    let sim = particles::simulate(&ctx);
    let pulse = state::pulse(ctx.state, ctx.tf);
    let bob_px = (state::bob(ctx.state, ctx.seed, ctx.tf) * g.ppu).round();
    let look = body::Look {
        cy: CY + bob_px / g.ppu,
        pulse,
        bright: state::brightness(ctx.state, pulse),
        flash: element::flashing(&ctx),
        warn: p.warning && (p.tick / WARN_HALF_PERIOD) % 2 == 0,
    };
    let mut cv = canvas::Canvas::new(g);
    body::draw(&mut cv, &ctx, &look);
    element::decorate(&mut cv, &ctx, look.cy);
    particles::draw(&mut cv, &sim.parts);
    state::sparkles(&mut cv, &ctx);
    eyes::draw(&mut cv, sim.looking, eyes::blinking(ctx.seed, ctx.t), element::ramp(ctx.elem)[1], look.bright);
    let mut grid = cv.cells();
    state::marks(&mut grid, &cv, &ctx, pulse);
    grid
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Status;
    use theme::Theme;

    const STATUSES: [Status; 5] = [Status::Running, Status::NeedsInput, Status::Idle, Status::Done, Status::Unknown];
    const WHITE: Option<(u8, u8, u8)> = Some((255, 255, 255));

    fn params(theme: Theme, status: Status, size: Size, tick: u64, seed: u32) -> SpiritParams {
        SpiritParams { theme, status, size, tick, seed, warning: false }
    }

    fn cell(g: &SpiritGrid, col: u16, row: u16) -> &Cell {
        &g.cells[usize::from(row) * usize::from(g.cols) + usize::from(col)]
    }

    /// (col, row, char) of every cell drawn in pure white.
    fn white_cells(g: &SpiritGrid) -> Vec<(u16, u16, char)> {
        let mut out = Vec::new();
        for row in 0..g.rows {
            for col in 0..g.cols {
                let c = cell(g, col, row);
                if c.fg == WHITE {
                    out.push((col, row, c.ch));
                }
            }
        }
        out
    }

    fn chars(g: &SpiritGrid) -> Vec<(u16, u16, char)> {
        (0..g.rows).flat_map(|r| (0..g.cols).map(move |c| (c, r))).map(|(c, r)| (c, r, cell(g, c, r).ch)).collect()
    }

    fn first_tick(seed: u32, blink: bool) -> u64 {
        (0..400).find(|&t| eyes::blinking(seed, t) == blink).expect("tick found")
    }

    #[test]
    fn render_is_deterministic() {
        for theme in Theme::ALL {
            for status in STATUSES {
                for size in Size::ALL {
                    for tick in [0, 1, 37, 500, 10_000] {
                        let p = params(theme, status, size, tick, 7);
                        assert_eq!(render(&p), render(&p), "{theme:?} {status:?} {size:?} {tick}");
                    }
                }
            }
        }
    }

    #[test]
    fn grid_size_s_is_11x5() {
        assert_eq!(grid_size(Size::S), (11, 5));
        let (m, l) = (grid_size(Size::M), grid_size(Size::L));
        assert!(m.0 > 11 && m.1 >= 5 && l.0 > m.0 && l.1 >= m.1, "{m:?} {l:?}");
        assert!(l.0 <= 17 && l.1 <= 7, "{l:?}");
        assert_eq!(max_grid_size(), l);
        for size in Size::ALL {
            let g = render(&params(Theme::Fire, Status::Running, size, 3, 1));
            assert_eq!((g.cols, g.rows), grid_size(size));
            assert_eq!(g.cells.len(), usize::from(g.cols) * usize::from(g.rows));
        }
    }

    #[test]
    fn every_state_and_theme_renders_nonempty_grid() {
        for theme in Theme::ALL {
            for status in STATUSES {
                for size in Size::ALL {
                    let g = render(&params(theme, status, size, 40, 3));
                    let dots = g.cells.iter().filter(|c| ('\u{2801}'..='\u{28ff}').contains(&c.ch)).count();
                    assert!(dots >= 10, "{theme:?} {status:?} {size:?}: {dots} braille cells");
                    assert!(g.cells.iter().all(|c| c.fg.is_some() || c.ch == ' '), "drawn cells have a colour");
                }
            }
        }
    }

    #[test]
    fn eyes_are_dark_hole_with_single_white_dot() {
        // `Cell` has no background field: every cell is foreground-only by construction.
        for seed in [1, 9, 77] {
            let t = first_tick(seed, false);
            for theme in Theme::ALL {
                for status in STATUSES {
                    let g = render(&params(theme, status, Size::S, t, seed));
                    let white = white_cells(&g);
                    assert_eq!(white.len(), 2, "{theme:?} {status:?}: {white:?}");
                    assert!(white.iter().all(|w| w.2 == '\u{2802}'), "one white dot (⠂) per eye: {white:?}");
                    assert_eq!(white[0].1, white[1].1, "eyes on one row");
                }
            }
        }
    }

    #[test]
    fn blink_hides_white_dot_and_shows_dash() {
        let seed = 5;
        let starts: Vec<u64> = (1..1000).filter(|&t| eyes::blinking(seed, t) && !eyes::blinking(seed, t - 1)).collect();
        assert!(starts.len() >= 10);
        for w in starts.windows(2) {
            assert!((45..=75).contains(&(w[1] - w[0])), "blink every 3-5 s at 15 fps: {starts:?}");
            assert!(eyes::blinking(seed, w[0] + 1) && !eyes::blinking(seed, w[0] + 2), "two frames");
        }
        let t = starts[0];
        for theme in Theme::ALL {
            for status in STATUSES {
                let g = render(&params(theme, status, Size::S, t, seed));
                assert!(white_cells(&g).is_empty(), "{theme:?} {status:?}: no white while blinking");
                let open = render(&params(theme, status, Size::S, t - 1, seed));
                for (col, row, _) in white_cells(&open) {
                    // the eye may shift by the running gaze between frames; the dash is on the eye row
                    let dash = (col.saturating_sub(1)..=col + 1).any(|c| c < g.cols && cell(&g, c, row).ch == '\u{2824}');
                    assert!(dash, "{theme:?} {status:?}: ⠤ dash on the eye row");
                }
                let dashes: Vec<_> = g.cells.iter().filter(|c| c.ch == '\u{2824}').collect();
                assert!(dashes.iter().all(|c| c.fg.is_some() && c.fg != WHITE));
            }
        }
    }

    #[test]
    fn all_states_share_the_same_eye_cells() {
        let seed = 11;
        let t = first_tick(seed, false);
        for theme in Theme::ALL {
            let eyes_of = |status| white_cells(&render(&params(theme, status, Size::S, t, seed)));
            let base = eyes_of(Status::Idle);
            for status in STATUSES {
                let e = eyes_of(status);
                if status == Status::Running {
                    // a running spirit may glance one cell to the right, where it casts
                    let shifted: Vec<_> = base.iter().map(|&(c, r, ch)| (c + 1, r, ch)).collect();
                    assert!(e == base || e == shifted, "{theme:?}: {e:?} vs {base:?}");
                } else {
                    assert_eq!(e, base, "{theme:?} {status:?}");
                }
            }
        }
    }

    /// Outline peaks that stand out at least `min` design units over their surroundings.
    fn spikes(theme: Theme, status: Status, tick: u64, min: f32) -> usize {
        let ctx = Ctx::new(&params(theme, status, Size::S, tick, 4));
        let n = 360;
        let r: Vec<f32> = (0..n).map(|i| element::radius(&ctx, i as f32 / n as f32 * std::f32::consts::TAU)).collect();
        let at = |i: isize| r[i.rem_euclid(n as isize) as usize];
        (0..n as isize)
            .filter(|&i| {
                let peak = at(i) > at(i - 1) && at(i) >= at(i + 1);
                let floor = (-12..=12).map(|d| at(i + d)).fold(f32::MAX, f32::min);
                peak && at(i) - floor >= min
            })
            .count()
    }

    #[test]
    fn lightning_has_spiky_outline() {
        // average count of sharp outline peaks (>= 0.8 design units) over 4 s of frames
        let avg = |theme, status| (0..60).map(|t| spikes(theme, status, t, 0.8)).sum::<usize>() as f32 / 60.0;
        for status in STATUSES {
            let bolt = avg(Theme::Lightning, status);
            for other in [Theme::Fire, Theme::Water, Theme::Ice] {
                let n = avg(other, status);
                assert!(bolt > n + 1.0, "{status:?}: lightning {bolt} vs {other:?} {n}");
            }
        }
        for status in [Status::Running, Status::NeedsInput] {
            assert!(avg(Theme::Lightning, status) >= 7.0, "{status:?}: long and short spikes all round");
        }
        // spike lengths flicker over time
        let ctx = |t| Ctx::new(&params(Theme::Lightning, Status::Running, Size::S, t, 4));
        let len = |t| (0..36).map(|i| element::radius(&ctx(t), i as f32 * 0.1745)).sum::<f32>();
        assert!((0..20).any(|t| (len(t) - len(t + 1)).abs() > 0.5));
    }

    #[test]
    fn needs_input_shows_question_mark_cell() {
        for theme in Theme::ALL {
            let mut rows = std::collections::BTreeSet::new();
            for t in 0..60 {
                let g = render(&params(theme, Status::NeedsInput, Size::S, t, 2));
                let q: Vec<_> = chars(&g).into_iter().filter(|c| c.2 == '?').collect();
                assert_eq!(q.len(), 1, "{theme:?} {t}");
                rows.insert(q[0].1);
            }
            assert!(rows.len() > 1, "{theme:?}: the ? bobs");
            let other = render(&params(theme, Status::Running, Size::S, 5, 2));
            assert!(!chars(&other).iter().any(|c| c.2 == '?'));
        }
    }

    #[test]
    fn idle_shows_z_cell() {
        for theme in Theme::ALL {
            for t in 0..60 {
                let g = render(&params(theme, Status::Idle, Size::S, t, 2));
                assert!(chars(&g).iter().any(|c| c.2 == 'z' || c.2 == 'Z'), "{theme:?} {t}");
            }
            let other = render(&params(theme, Status::Done, Size::S, 5, 2));
            assert!(!chars(&other).iter().any(|c| c.2 == 'z' || c.2 == 'Z'));
        }
    }

    #[test]
    fn warning_flickers_the_body_red() {
        for theme in Theme::ALL {
            let calm = render(&params(theme, Status::Running, Size::S, 0, 2));
            let warn = render(&SpiritParams { warning: true, ..params(theme, Status::Running, Size::S, 0, 2) });
            assert_ne!(calm, warn, "{theme:?}: tinted in the on phase");
            let redness = |g: &SpiritGrid| g.cells.iter().filter_map(|c| c.fg).map(|(r, g, b)| f32::from(r) - (f32::from(g) + f32::from(b)) / 2.0).sum::<f32>();
            assert!(redness(&warn) > redness(&calm) + 100.0, "{theme:?}");
            let t_off = (0..40).find(|&t| render(&SpiritParams { warning: true, ..params(theme, Status::Running, Size::S, t, 2) }) == render(&params(theme, Status::Running, Size::S, t, 2)));
            assert!(t_off.is_some(), "{theme:?}: an off phase exists");
        }
    }

    /// Every element x state as ANSI text. Run with
    /// `cargo test dump_spirits -- --ignored --nocapture` (env `SPIRIT_TICK`, `SPIRIT_SIZE` = 0..2).
    #[test]
    #[ignore]
    fn dump_spirits() {
        let env = |k: &str, d: u64| std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(d);
        let (tick, size) = (env("SPIRIT_TICK", 40), Size::ALL[env("SPIRIT_SIZE", 0).min(2) as usize]);
        for (si, status) in STATUSES.into_iter().enumerate() {
            let grids: Vec<SpiritGrid> = Theme::ALL.iter().enumerate().map(|(ei, &theme)| render(&params(theme, status, size, tick, (si * 4 + ei) as u32 + 1))).collect();
            for row in 0..grids[0].rows {
                let mut line = String::new();
                for g in &grids {
                    for col in 0..g.cols {
                        let c = cell(g, col, row);
                        let (r, gg, b) = c.fg.unwrap_or((0, 0, 0));
                        line += &format!("\x1b[38;2;{r};{gg};{b}m{}", c.ch);
                    }
                    line += "\x1b[0m  ";
                }
                println!("{line}");
            }
            println!();
        }
    }
}
