//! Element spirits: a small braille creature per session card.
//!
//! [`render`] is a pure function: the spirit is painted procedurally on a
//! sub-pixel canvas (2 x 4 braille dots per terminal cell) and returned as a
//! grid of braille cells with 24-bit foreground colours. Cells never carry a
//! background colour. The same parameters always give the same grid; motion
//! comes from `tick` (about 15 per second).
//!
//! - Element ([`Theme`]): palette, outline (fire tongues, water drop, poison
//!   slither with bubbles, lightning spikes), decorations and particles.
//! - State ([`Status`]): RUNNING casts element magic to the right (see `magic`),
//!   NEEDS_INPUT shows a bobbing `?` and a pulsing glow, IDLE is dim and slow
//!   with a `z`, DONE sparkles and bounces, UNKNOWN is grey.
//! - Eyes are the same in every state (see `eyes`); while casting, the eye
//!   dot looks one sub-pixel toward the magic.
//! - Casting pose (RUNNING): a short wind-up leaning toward the magic before
//!   each cast and a small recoil on release (see `state::pose`).

mod body;
mod canvas;
mod element;
mod eyes;
mod magic;
mod noise;
mod particles;
mod state;
pub mod theme;

use crate::model::Status;
use canvas::Geom;
use theme::Theme;

/// Design space (the reference x1 canvas in sub-pixels: 17 x 7 terminal
/// cells); every size maps it onto its own smaller canvas.
const DW: f32 = 34.0;
const DH: f32 = 28.0;
/// Body centre / radius in design units.
const CX: f32 = 11.0;
const CY: f32 = 15.0;
const BASE_R: f32 = 8.5;
/// Water ripple line (design units); lifted onto the last sub-pixel row of
/// canvases too short to reach it (see [`floor_y`]).
const FLOOR_Y: f32 = 26.0;
/// Float phases wrap every 2^16 ticks (about 73 min at 15 fps) so f32 math
/// keeps sub-tick resolution on a monitor that runs for weeks; the wrap is a
/// single-frame jump in phase.
const PHASE_PERIOD: u64 = 1 << 16;

/// Tick as an f32 phase base, exact and small at any uptime.
fn phase_tick(t: u64) -> f32 {
    (t % PHASE_PERIOD) as f32
}

/// Ticks per half period of the red warning flicker (about 0.5 s at 15 fps).
pub const WARN_HALF_PERIOD: u64 = 8;

/// Context usage (percent) at which the magic grows from small to normal,
/// and from normal to big.
const MAGIC_NORMAL_AT: f32 = 30.0;
const MAGIC_BIG_AT: f32 = 70.0;
/// Context usage above which the body flickers red.
const WARNING_ABOVE: f32 = 90.0;
/// Canvas width (sub-pixels) of a spirit casting big magic: 17 cells, the
/// card's inner width minus a one-column left pad.
const BIG_MAGIC_W: usize = 34;

/// How strong the cast magic (RUNNING, see `magic`) is; driven by context usage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Magic {
    /// a short, thin cast right next to the body
    Small,
    /// the approved reference look
    #[default]
    Normal,
    /// a longer, wider, denser cast (on a wider canvas)
    Big,
}

impl Magic {
    #[cfg(test)]
    pub const ALL: [Magic; 3] = [Magic::Small, Magic::Normal, Magic::Big];
}

/// Magic level for a context usage: < 30 % small, 30..70 % normal,
/// >= 70 % big. Unknown or invalid (NaN, negative) usage casts normal magic.
pub fn magic_for(ctx_percent: Option<f32>) -> Magic {
    match ctx_percent {
        Some(p) if p.is_nan() || p < 0.0 => Magic::Normal,
        Some(p) if p >= MAGIC_BIG_AT => Magic::Big,
        Some(p) if p >= MAGIC_NORMAL_AT => Magic::Normal,
        Some(_) => Magic::Small,
        None => Magic::Normal,
    }
}

/// Whether the context is nearly full (> 90 %): the body flickers red.
pub fn warning(ctx_percent: Option<f32>) -> bool {
    ctx_percent.is_some_and(|p| p > WARNING_ABOVE)
}

/// Spirit size step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Size {
    S,
    M,
    L,
}

impl Size {
    pub const ALL: [Size; 3] = [Size::S, Size::M, Size::L];

    /// Card variants: every size is 4 rows tall (16 sub-pixels) so a 20x9
    /// card never changes size; bigger steps get a bigger body and more room
    /// to the right for the cast magic. The body centre sits at 1/3 of the
    /// width (as in the reference design) and high enough that the eye row
    /// (cell row 1) has body above and below it.
    const fn geom(self) -> Geom {
        match self {
            // 11 x 4 cells, body radius 5.5 sub-pixels
            Size::S => Geom { ppu: 5.5 / BASE_R, w: 22, h: 16, rcx: 7.0, rcy: 8.0, small: true },
            // 14 x 4 cells, body radius 6 sub-pixels
            Size::M => Geom { ppu: 6.0 / BASE_R, w: 28, h: 16, rcx: 9.0, rcy: 8.0, small: true },
            // 17 x 4 cells, body radius 6.5 sub-pixels
            Size::L => Geom { ppu: 6.5 / BASE_R, w: 34, h: 16, rcx: 11.0, rcy: 8.0, small: true },
        }
    }
}

/// Horizontal centre of the body in grid columns (the right side of the grid
/// is reserved for the cast magic, so this sits left of the grid centre).
#[cfg_attr(not(test), allow(dead_code))]
pub fn body_center_x(size: Size) -> f32 {
    size.geom().rcx / 2.0
}

/// Grid size (columns, rows) of a spirit of `size`.
pub const fn grid_size(size: Size) -> (u16, u16) {
    let g = size.geom();
    ((g.w / 2) as u16, (g.h / 4) as u16)
}

/// Grid size of a spirit of `size` casting `magic`: big magic widens the grid
/// to the right (the body stays where it is).
pub const fn grid_size_for(size: Size, magic: Magic) -> (u16, u16) {
    let g = geom_for(size, magic);
    ((g.w / 2) as u16, (g.h / 4) as u16)
}

const fn geom_for(size: Size, magic: Magic) -> Geom {
    let mut g = size.geom();
    if matches!(magic, Magic::Big) && g.w < BIG_MAGIC_W {
        g.w = BIG_MAGIC_W;
    }
    g
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
    /// strength of the cast magic (context usage)
    pub magic: Magic,
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
    /// water ripple line in design units, inside this size's canvas
    floor: f32,
    magic: Magic,
    /// right edge of the canvas in design units (particles die past it)
    right: f32,
}

/// The ripple line for a canvas: [`FLOOR_Y`], or the last sub-pixel row when
/// the canvas ends above it.
fn floor_y(g: &Geom) -> f32 {
    FLOOR_Y.min(CY + (g.h as f32 - 1.0 - g.rcy) / g.ppu)
}

impl Ctx {
    fn new(p: &SpiritParams) -> Self {
        let g = geom_for(p.size, p.magic);
        Ctx {
            elem: p.theme,
            state: p.status,
            seed: p.seed,
            t: p.tick,
            tf: phase_tick(p.tick),
            sp: state::speed(p.status),
            small: g.small,
            floor: floor_y(&g),
            magic: p.magic,
            // the reference design width (unchanged look), or the wider big-magic canvas
            right: if matches!(p.magic, Magic::Big) { DW.max(CX + (g.w as f32 - g.rcx) / g.ppu) } else { DW },
        }
    }
}

/// Renders one frame of a spirit.
pub fn render(p: &SpiritParams) -> SpiritGrid {
    let g = geom_for(p.size, p.magic);
    let ctx = Ctx::new(p);
    let parts = particles::simulate(&ctx);
    let pulse = state::pulse(ctx.state, ctx.tf);
    let pose = state::pose(ctx.elem, ctx.state, ctx.seed, ctx.t);
    let bob_px = (state::bob(ctx.state, ctx.seed, ctx.tf) * g.ppu).round() + pose.dy;
    let look = body::Look {
        cy: CY + bob_px / g.ppu,
        pulse,
        bright: state::brightness(ctx.state, pulse),
        flash: element::flashing(&ctx),
        warn: p.warning && (p.tick / WARN_HALF_PERIOD) % 2 == 0,
        dx: pose.dx,
        tilt: pose.tilt,
        swell: pose.swell,
    };
    let mut cv = canvas::Canvas::new(g);
    body::draw(&mut cv, &ctx, &look);
    element::decorate(&mut cv, &ctx, CX + pose.dx / g.ppu, look.cy, look.bright);
    particles::draw(&mut cv, &parts);
    magic::draw(&mut cv, &ctx, CY + (bob_px - pose.dy) / g.ppu);
    state::sparkles(&mut cv, &ctx);
    eyes::draw(&mut cv, pose.facing, eyes::blinking(ctx.seed, ctx.t), element::ramp(ctx.elem)[1], look.bright);
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
        SpiritParams { theme, status, size, tick, seed, warning: false, magic: Magic::Normal }
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
    fn grid_sizes_are_four_rows_tall() {
        // card variant: every size fits the 4-row spirit band of a 20x9 card
        assert_eq!(grid_size(Size::S), (11, 4));
        assert_eq!(grid_size(Size::M), (14, 4));
        assert_eq!(grid_size(Size::L), (17, 4));
        assert_eq!(max_grid_size(), (17, 4));
        for size in Size::ALL {
            for theme in Theme::ALL {
                for status in STATUSES {
                    let g = render(&params(theme, status, size, 3, 1));
                    assert_eq!((g.cols, g.rows), grid_size(size), "{theme:?} {status:?} {size:?}");
                    assert_eq!(g.cells.len(), usize::from(g.cols) * usize::from(g.rows));
                }
            }
        }
    }

    #[test]
    fn bigger_sizes_have_bigger_bodies() {
        // M and L are wider and their bodies cover more dots than S
        let dots = |size| -> u32 {
            let g = render(&params(Theme::Water, Status::Idle, size, 0, 3));
            g.cells.iter().filter(|c| ('\u{2801}'..='\u{28ff}').contains(&c.ch)).map(|c| (c.ch as u32 - 0x2800).count_ones()).sum()
        };
        let (s, m, l) = (dots(Size::S), dots(Size::M), dots(Size::L));
        assert!(s < m && m < l, "S {s} < M {m} < L {l}");
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
            for (theme, size) in Theme::ALL.into_iter().flat_map(|t| Size::ALL.map(|s| (t, s))) {
                for status in STATUSES {
                    let g = render(&params(theme, status, size, t, seed));
                    let white = white_cells(&g);
                    assert_eq!(white.len(), 2, "{theme:?} {status:?} {size:?}: {white:?}");
                    // casting: the dot faces the magic, one sub-pixel right in the same cell (⠐)
                    let casting = status == Status::Running && state::cast_phase(seed, t).is_some();
                    let dot = if casting { '\u{2810}' } else { '\u{2802}' };
                    assert!(white.iter().all(|w| w.2 == dot), "one white dot ({dot}) per eye: {white:?}");
                    assert_eq!(white[0].1, white[1].1, "eyes on one row");
                }
            }
        }
    }

    #[test]
    fn eyes_sit_inside_the_body_in_every_size() {
        let seed = 9;
        let t = first_tick(seed, false);
        let is_body = |g: &SpiritGrid, c: u16, r: u16| ('\u{2801}'..='\u{28ff}').contains(&cell(g, c, r).ch);
        for theme in Theme::ALL {
            for size in Size::ALL {
                let g = render(&params(theme, Status::Idle, size, t, seed));
                for (col, row, _) in white_cells(&g) {
                    assert!(row > 0 && row + 1 < g.rows, "{theme:?} {size:?}: eye row {row}");
                    assert!(is_body(&g, col, row - 1) && is_body(&g, col, row + 1), "{theme:?} {size:?}: body above and below the eye");
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
        for (theme, size) in Theme::ALL.into_iter().flat_map(|t| Size::ALL.map(|s| (t, s))) {
            let eyes_of = |status, t| white_cells(&render(&params(theme, status, size, t, seed)));
            let base = eyes_of(Status::Idle, t);
            for status in STATUSES {
                assert_eq!(eyes_of(status, t), base, "{theme:?} {status:?} {size:?}");
            }
            // casting only moves the dot inside its cell; the eye cells never move
            for u in (0..200).filter(|&u| !eyes::blinking(seed, u)) {
                let cells = |status| eyes_of(status, u).into_iter().map(|(c, r, _)| (c, r)).collect::<Vec<_>>();
                assert_eq!(cells(Status::Running), cells(Status::Idle), "{theme:?} {size:?} {u}");
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
            for other in [Theme::Fire, Theme::Water, Theme::Poison] {
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
        for (theme, size) in Theme::ALL.into_iter().flat_map(|t| Size::ALL.map(|s| (t, s))) {
            let mut rows = std::collections::BTreeSet::new();
            for t in 0..60 {
                let g = render(&params(theme, Status::NeedsInput, size, t, 2));
                let q: Vec<_> = chars(&g).into_iter().filter(|c| c.2 == '?').collect();
                assert_eq!(q.len(), 1, "{theme:?} {size:?} {t}");
                rows.insert(q[0].1);
            }
            assert!(rows.len() > 1, "{theme:?} {size:?}: the ? bobs");
            let other = render(&params(theme, Status::Running, Size::S, 5, 2));
            assert!(!chars(&other).iter().any(|c| c.2 == '?'));
        }
    }

    #[test]
    fn idle_shows_z_cell() {
        for (theme, size) in Theme::ALL.into_iter().flat_map(|t| Size::ALL.map(|s| (t, s))) {
            for t in 0..60 {
                let g = render(&params(theme, Status::Idle, size, t, 2));
                assert!(chars(&g).iter().any(|c| c.2 == 'z' || c.2 == 'Z'), "{theme:?} {size:?} {t}");
            }
            let other = render(&params(theme, Status::Done, Size::S, 5, 2));
            assert!(!chars(&other).iter().any(|c| c.2 == 'z' || c.2 == 'Z'));
        }
    }

    #[test]
    fn magic_for_thresholds() {
        let cases: [(Option<f32>, Magic); 13] = [
            (None, Magic::Normal),
            (Some(f32::NAN), Magic::Normal),
            (Some(-5.0), Magic::Normal),
            (Some(f32::NEG_INFINITY), Magic::Normal),
            (Some(0.0), Magic::Small),
            (Some(29.9), Magic::Small),
            (Some(30.0), Magic::Normal),
            (Some(69.9), Magic::Normal),
            (Some(70.0), Magic::Big),
            (Some(90.1), Magic::Big),
            (Some(100.0), Magic::Big),
            (Some(250.0), Magic::Big),
            (Some(f32::INFINITY), Magic::Big),
        ];
        for (pct, want) in cases {
            assert_eq!(magic_for(pct), want, "{pct:?}");
        }
        assert_eq!(Magic::default(), Magic::Normal);
    }

    /// Braille dots right of the body's right edge, summed over a cast cycle.
    fn magic_stats(theme: Theme, magic: Magic) -> (u32, u16) {
        let (mut dots, mut reach) = (0u32, 0u16);
        let body_right = (body_center_x(Size::S) + 3.0).ceil() as u16;
        for t in 0..120 {
            let g = render(&SpiritParams { magic, ..params(theme, Status::Running, Size::S, t, 4) });
            for row in 0..g.rows {
                for col in body_right..g.cols {
                    let c = cell(&g, col, row).ch;
                    if ('\u{2801}'..='\u{28ff}').contains(&c) {
                        dots += (c as u32 - 0x2800).count_ones();
                        reach = reach.max(col);
                    }
                }
            }
        }
        (dots, reach)
    }

    #[test]
    fn magic_scales_the_cast_particles() {
        for theme in Theme::ALL {
            let (s_dots, s_reach) = magic_stats(theme, Magic::Small);
            let (n_dots, n_reach) = magic_stats(theme, Magic::Normal);
            let (b_dots, b_reach) = magic_stats(theme, Magic::Big);
            assert!(s_dots < n_dots && n_dots < b_dots, "{theme:?}: dots {s_dots} < {n_dots} < {b_dots}");
            assert!(s_reach < n_reach && n_reach < b_reach, "{theme:?}: reach {s_reach} < {n_reach} < {b_reach}");
        }
    }

    /// Painted cells right of the body and the farthest painted column of a
    /// running spirit, over a replay window of several casts.
    fn magic_spread(theme: Theme, magic: Magic) -> (u32, u16) {
        // the cast magic only: ambient sparks/embers are not part of it
        particles::NO_AMBIENT.with(|c| c.set(true));
        let (mut cells, mut reach) = (0u32, 0u16);
        let body_right = (body_center_x(Size::S) + 3.0).ceil() as u16;
        for seed in [4, 11, 23] {
            for t in 0..240 {
                let g = render(&SpiritParams { magic, ..params(theme, Status::Running, Size::S, t, seed) });
                for row in 0..g.rows {
                    for col in body_right..g.cols {
                        if ('\u{2801}'..='\u{28ff}').contains(&cell(&g, col, row).ch) {
                            cells += 1;
                            reach = reach.max(col);
                        }
                    }
                }
            }
        }
        particles::NO_AMBIENT.with(|c| c.set(false));
        (cells, reach)
    }

    #[test]
    fn magic_levels_are_clearly_visible() {
        let spreads: Vec<_> = Theme::ALL.iter().map(|&t| (t, Magic::ALL.map(|m| magic_spread(t, m)))).collect();
        for (theme, [(s_cells, s_reach), (n_cells, n_reach), (b_cells, b_reach)]) in &spreads {
            println!("{theme:?}: reach S {s_reach} N {n_reach} B {b_reach}; cells S {s_cells} N {n_cells} B {b_cells}");
        }
        for &(theme, [(s_cells, s_reach), (n_cells, n_reach), (b_cells, b_reach)]) in &spreads {
            // a running lightning body's own spikes already reach col 8 (measured
            // with no bolt at all), so small magic can only end 2 columns short
            // (the lightning bolt strikes far out at every level, so its small
            // cast ends only one column short and keeps more cells)
            let (gap, small_cells) = if theme == Theme::Lightning { (1, 8) } else { (3, 6) };
            assert!(s_reach + gap <= n_reach, "{theme:?}: small reach {s_reach} vs normal {n_reach}");
            assert!(b_reach >= n_reach + 4, "{theme:?}: big reach {b_reach} vs normal {n_reach}");
            assert!(s_cells * 10 <= n_cells * small_cells, "{theme:?}: small cells {s_cells} vs normal {n_cells}");
            assert!(b_cells * 10 >= n_cells * 15, "{theme:?}: big cells {b_cells} vs normal {n_cells}");
        }
    }

    #[test]
    fn magic_grid_widths() {
        assert_eq!(grid_size_for(Size::S, Magic::Small), (11, 4));
        assert_eq!(grid_size_for(Size::S, Magic::Normal), (11, 4), "approved look");
        assert_eq!(grid_size_for(Size::S, Magic::Big), (17, 4), "room to cast farther");
        assert_eq!(grid_size_for(Size::L, Magic::Big), (17, 4));
        for theme in Theme::ALL {
            for magic in Magic::ALL {
                let g = render(&SpiritParams { magic, ..params(theme, Status::Running, Size::S, 30, 1) });
                assert_eq!((g.cols, g.rows), grid_size_for(Size::S, magic));
            }
        }
    }

    #[test]
    fn magic_leaves_body_and_eyes_unchanged() {
        let left = |g: &SpiritGrid| -> Vec<(u16, u16, char)> { chars(g).into_iter().filter(|c| c.0 < 11).collect() };
        for theme in Theme::ALL {
            for status in STATUSES {
                for t in [0, 5, 61] {
                    let base = render(&params(theme, status, Size::S, t, 6));
                    for magic in Magic::ALL {
                        let g = render(&SpiritParams { magic, ..params(theme, status, Size::S, t, 6) });
                        assert_eq!(white_cells(&g), white_cells(&base), "{theme:?} {status:?} {magic:?} {t}: eyes");
                        if status != Status::Running {
                            assert_eq!(left(&g), left(&base), "{theme:?} {status:?} {magic:?} {t}: no cast, same spirit");
                        }
                    }
                }
            }
            // a running spirit before its first cast is the same at every level
            for magic in Magic::ALL {
                let g = render(&SpiritParams { magic, ..params(theme, Status::Running, Size::S, 0, 6) });
                assert_eq!(left(&g), left(&render(&params(theme, Status::Running, Size::S, 0, 6))), "{theme:?} {magic:?}");
            }
        }
    }

    #[test]
    fn warning_only_above_90() {
        assert!(!warning(None));
        assert!(!warning(Some(f32::NAN)));
        assert!(!warning(Some(50.0)));
        assert!(!warning(Some(90.0)));
        assert!(warning(Some(90.1)));
        assert!(warning(Some(100.0)));
        assert!(!warning(Some(-95.0)));
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

    /// Full-range seeds as cards produce them (FNV-1a of a session id).
    const BIG_SEEDS: [u32; 3] = [u32::MAX, 706_312_658, 0x9e37_79b9];

    /// Mean luminance of the drawn cells, without `skip` glyphs and without the
    /// eye catchlights (pure white, the same in every state; in a 4-row spirit
    /// they are a large share of the cells and would mask the body's mood).
    fn luminance(g: &SpiritGrid, skip: &[char]) -> f32 {
        let lum: Vec<f32> = g
            .cells
            .iter()
            .filter(|c| !skip.contains(&c.ch) && c.fg != WHITE)
            .filter_map(|c| c.fg)
            .map(|(r, gg, b)| 0.2126 * f32::from(r) + 0.7152 * f32::from(gg) + 0.0722 * f32::from(b))
            .collect();
        lum.iter().sum::<f32>() / lum.len().max(1) as f32
    }

    #[test]
    fn large_seeds_keep_bobbing() {
        for seed in BIG_SEEDS {
            for status in [Status::Running, Status::NeedsInput] {
                let bob: Vec<f32> = (0..60).map(|t| state::bob(status, seed, phase_tick(t))).collect();
                let moving = bob.windows(2).filter(|w| (w[0] - w[1]).abs() > 1e-3).count();
                assert!(moving > 50, "{seed} {status:?}: {moving} of 59 ticks move");
            }
            let done: Vec<f32> = (0..60).map(|t| state::bob(Status::Done, seed, phase_tick(t))).collect();
            let spread = done.iter().cloned().fold(f32::MIN, f32::max) - done.iter().cloned().fold(f32::MAX, f32::min);
            assert!(spread > 1.0, "{seed}: DONE bounce spread {spread}");
            // the bounce shows on the grid: some frames differ by more than particles
            let frames: std::collections::BTreeSet<Vec<char>> =
                (0..30).map(|t| render(&params(Theme::Water, Status::Done, Size::S, t, seed)).cells.iter().map(|c| c.ch).collect()).collect();
            assert!(frames.len() > 3, "{seed}: DONE frames vary");
        }
    }

    #[test]
    fn large_seeds_keep_rotating_and_twinkling() {
        for seed in BIG_SEEDS {
            let at = |theme, t| Ctx::new(&params(theme, Status::Running, Size::S, t, seed));
            let rot0 = element::bolt_rot(&at(Theme::Lightning, 0));
            assert!((1..400).all(|t| element::bolt_rot(&at(Theme::Lightning, t)) != rot0), "{seed}: lightning rotates");
            assert!((element::bolt_rot(&at(Theme::Lightning, 400)) - rot0).abs() > 5.0);
            let sway: Vec<f32> = (0..400).map(|t| element::sway(&at(Theme::Poison, t))).collect();
            assert!(sway.windows(2).all(|w| w[0] != w[1]), "{seed}: poison keeps swaying");
            let spread = sway.iter().cloned().fold(f32::MIN, f32::max) - sway.iter().cloned().fold(f32::MAX, f32::min);
            assert!(spread > 0.8, "{seed}: sway spread {spread}");
            let pops = (0..400).filter(|&t| element::bubbles(&at(Theme::Poison, t)).iter().any(|b| b.stage == element::Stage::Pop)).count();
            assert!(pops >= 3, "{seed}: bubbles keep popping ({pops})");
        }
    }

    #[test]
    fn motion_survives_long_uptime() {
        let t0 = 40_000_000;
        for seed in [7, 706_312_658] {
            let g = |t| render(&params(Theme::Fire, Status::Running, Size::S, t, seed));
            assert_ne!(g(t0), g(t0 + 10), "{seed}");
            let bob: Vec<f32> = (t0..t0 + 30).map(|t| state::bob(Status::Running, seed, phase_tick(t))).collect();
            let moving = bob.windows(2).filter(|w| (w[0] - w[1]).abs() > 1e-3).count();
            assert!(moving > 25, "{seed}: {bob:?}");
            assert!((t0..t0 + 400).any(|t| eyes::blinking(seed, t)));
        }
    }

    #[test]
    fn needs_input_pulses_idle_is_dim_done_sparkles() {
        let seed = 706_312_658;
        for theme in Theme::ALL {
            // NEEDS_INPUT: the body brightness pulses (the `?` mark excluded)
            let lum: Vec<f32> = (0..22).map(|t| luminance(&render(&params(theme, Status::NeedsInput, Size::S, t, seed)), &['?'])).collect();
            let spread = lum.iter().cloned().fold(f32::MIN, f32::max) - lum.iter().cloned().fold(f32::MAX, f32::min);
            assert!(spread > 10.0, "{theme:?}: pulse spread {spread}");
            // IDLE is dimmer than RUNNING
            for (t, size) in [0, 20, 45].into_iter().flat_map(|t| Size::ALL.map(|s| (t, s))) {
                let idle = luminance(&render(&params(theme, Status::Idle, size, t, seed)), &['z', 'Z']);
                let run = luminance(&render(&params(theme, Status::Running, size, t, seed)), &[]);
                assert!(idle < run * 0.8, "{theme:?} {size:?} {t}: idle {idle} vs running {run}");
            }
            // DONE: gold sparkles appear around the body
            let sparkle = |t| render(&params(theme, Status::Done, Size::S, t, seed)).cells.iter().any(|c| matches!(c.fg, Some((250, 236, 150)) | Some((204, 188, 120))));
            assert!((0..60).any(sparkle), "{theme:?}: sparkles");
        }
    }

    #[test]
    fn render_has_no_hidden_state() {
        let p = params(Theme::Poison, Status::Running, Size::M, 300, 706_312_658);
        let fresh = render(&p);
        for t in 0..300 {
            render(&SpiritParams { tick: t, ..p });
        }
        render(&SpiritParams { seed: 1, ..p });
        assert_eq!(render(&p), fresh);
    }

    fn is_braille(ch: char) -> bool {
        ('\u{2801}'..='\u{28ff}').contains(&ch)
    }

    /// First tick a running spirit of `seed` casts.
    fn first_cast(seed: u32) -> u64 {
        (1..).find(|&t| state::casts_at(seed, t)).expect("casts")
    }

    /// Body cells (columns 0..7, left of the magic area) that carry a colour,
    /// without eyes and text marks.
    fn body_cells(g: &SpiritGrid) -> Vec<Cell> {
        (0..g.rows)
            .flat_map(|r| (0..7).map(move |c| (c, r)))
            .map(|(c, r)| *cell(g, c, r))
            .filter(|c| c.fg.is_some() && c.fg != WHITE && !matches!(c.ch, '?' | 'z' | 'Z'))
            .collect()
    }

    #[test]
    fn poison_body_is_green() {
        for seed in [9, 706_312_658] {
            for status in [Status::Running, Status::NeedsInput, Status::Idle] {
                for t in [0, 17, 33, 90, 141] {
                    let g = render(&params(Theme::Poison, status, Size::S, t, seed));
                    let body = body_cells(&g);
                    assert!(body.len() >= 10, "{status:?} {t}: {} body cells", body.len());
                    let green = body.iter().filter(|c| c.fg.is_some_and(|(r, gg, b)| gg > r && gg > b)).count();
                    assert!(green * 10 >= body.len() * 9, "{status:?} {t}: {green} of {} cells green", body.len());
                }
            }
        }
    }

    #[test]
    fn poison_bubbles_form_and_pop() {
        let ring = Some(canvas::to_rgb(element::BUBBLE_RING, 250.0));
        let pop = Some(canvas::to_rgb(element::BUBBLE_POP, 250.0));
        let has = |g: &SpiritGrid, c| g.cells.iter().any(|x| x.fg == c);
        for seed in [3, 41, 706_312_658] {
            let frames: Vec<SpiritGrid> = (0..240).map(|t| render(&params(Theme::Poison, Status::Running, Size::S, t, seed))).collect();
            // a ring shows on the body, then pops into a sparkle a few ticks later
            let popped = (0..236).filter(|&t| has(&frames[t], ring) && (t + 1..=t + 3).any(|u| has(&frames[u], pop))).count();
            assert!(popped >= 2, "{seed}: {popped} ring -> pop sequences");
        }
        // every bubble goes bead -> ring -> swell -> pop, one stage per tick or more
        let ctx = |status, t| Ctx::new(&params(Theme::Poison, status, Size::S, t, 5));
        let pops = |status| (0..600).map(|t| element::bubbles(&ctx(status, t)).iter().filter(|b| b.stage == element::Stage::Pop).count()).sum::<usize>();
        let (run, idle, done) = (pops(Status::Running), pops(Status::Idle), pops(Status::Done));
        assert!(idle >= 1 && idle < run, "idle bubbles are occasional: idle {idle}, running {run}");
        assert!(done > idle, "done pops like sparkles: done {done}, idle {idle}");
        assert_eq!(pops(Status::Unknown), 0, "unknown is still");
        for theme in [Theme::Fire, Theme::Water, Theme::Lightning] {
            assert!((0..100).all(|t| element::bubbles(&Ctx::new(&params(theme, Status::Running, Size::S, t, 5))).is_empty()));
        }
    }

    /// Mean share of body cells (columns 0..7) whose glyph changes between
    /// adjacent ticks at rest: steps into or out of a casting pose (a
    /// deliberate lean, see `casting_pose_leans_right_then_recoils`) are left out.
    fn churn(theme: Theme, seed: u32) -> f32 {
        let body = |t| {
            let g = render(&params(theme, Status::Running, Size::S, t, seed));
            (0..g.rows).flat_map(|r| (0..7).map(move |c| (c, r))).map(|(c, r)| cell(&g, c, r).ch).collect::<Vec<char>>()
        };
        let resting = |t| state::cast_phase(seed, t).is_none();
        let pairs: Vec<(Vec<char>, Vec<char>)> = (100..340).filter(|&t| resting(t) && resting(t + 1)).map(|t| (body(t), body(t + 1))).collect();
        let changed: usize = pairs.iter().map(|(a, b)| a.iter().zip(b).filter(|(a, b)| a != b).count()).sum();
        changed as f32 / (pairs.len() * pairs[0].0.len()) as f32
    }

    #[test]
    fn poison_slithers_smoothly() {
        for seed in [4, 11, 706_312_658] {
            let (poison, fire) = (churn(Theme::Poison, seed), churn(Theme::Fire, seed));
            println!("{seed}: churn poison {poison:.3} fire {fire:.3}");
            assert!(poison < 0.19, "{seed}: poison body changes {poison} of its cells per tick");
            assert!(poison < fire * 0.7, "{seed}: poison {poison} calmer than fire {fire}");
        }
        // smooth slow sway: tiny steps per tick, a visible range over a few seconds
        for status in [Status::Running, Status::Idle] {
            let sw: Vec<f32> = (0..300).map(|t| element::sway(&Ctx::new(&params(Theme::Poison, status, Size::S, t, 7)))).collect();
            assert!(sw.windows(2).all(|w| (w[0] - w[1]).abs() < 0.12), "{status:?}: no jitter");
            let spread = sw.iter().cloned().fold(f32::MIN, f32::max) - sw.iter().cloned().fold(f32::MAX, f32::min);
            assert!(spread > 0.5, "{status:?}: sway spread {spread}");
        }
    }

    /// Whether some row is painted in each of the `n` columns from `from`.
    fn jet_run(g: &SpiritGrid, from: u16, n: u16) -> bool {
        (0..g.rows).any(|r| (from..from + n).all(|c| c < g.cols && is_braille(cell(g, c, r).ch)))
    }

    #[test]
    fn fire_breath_is_a_continuous_jet() {
        for seed in [4, 11, 23, 706_312_658] {
            let start = first_cast(seed) + 3;
            let ticks: Vec<u64> = (start..start + 240).collect();
            let at = |t, magic| render(&SpiritParams { magic, ..params(Theme::Fire, Status::Running, Size::S, t, seed) });
            let normal = ticks.iter().filter(|&&t| jet_run(&at(t, Magic::Normal), 7, 4)).count();
            assert!(normal * 10 >= ticks.len() * 9, "{seed}: normal jet on {normal} of {} ticks", ticks.len());
            let big = ticks.iter().filter(|&&t| jet_run(&at(t, Magic::Big), 7, 8)).count();
            assert!(big * 10 >= ticks.len() * 9, "{seed}: big jet on {big} of {} ticks", ticks.len());
            for status in [Status::Idle, Status::Done, Status::NeedsInput] {
                assert!((0..240).all(|t| !jet_run(&render(&params(Theme::Fire, status, Size::S, t, seed)), 7, 2)), "{seed} {status:?}: no breath");
            }
        }
    }

    /// (col, row) of the painted cells right of the body (column 7 on).
    fn magic_cells(g: &SpiritGrid) -> Vec<(u16, u16)> {
        (0..g.rows).flat_map(|r| (7..g.cols).map(move |c| (c, r))).filter(|&(c, r)| is_braille(cell(g, c, r).ch)).collect()
    }

    #[test]
    fn water_casts_a_spinning_vortex() {
        particles::NO_AMBIENT.with(|c| c.set(true));
        for seed in [4, 11, 23] {
            let c = first_cast(seed);
            for (magic, rows) in [(Magic::Normal, 3), (Magic::Big, 4)] {
                let at = |t| magic_cells(&render(&SpiritParams { magic, ..params(Theme::Water, Status::Running, Size::S, t, seed) }));
                let spread: std::collections::BTreeSet<u16> = at(c + 6).iter().map(|p| p.1).collect();
                assert!(spread.len() >= rows, "{seed} {magic:?}: vortex spans {spread:?}");
                assert!((c + 4..c + 10).any(|t| at(t) != at(t + 1)), "{seed} {magic:?}: the vortex spins");
            }
        }
        particles::NO_AMBIENT.with(|c| c.set(false));
    }

    #[test]
    fn lightning_bolt_strikes_top_to_bottom() {
        particles::NO_AMBIENT.with(|c| c.set(true));
        for seed in [4, 11, 23] {
            let casts: Vec<u64> = (1..400).filter(|&t| state::casts_at(seed, t)).take(4).collect();
            let mut shapes = std::collections::BTreeSet::new();
            for &c in &casts {
                let g = render(&params(Theme::Lightning, Status::Running, Size::S, c, seed));
                for row in 0..g.rows {
                    assert!((8..g.cols).any(|col| is_braille(cell(&g, col, row).ch)), "{seed} {c}: bolt crosses row {row}");
                }
                shapes.insert(chars(&g).into_iter().filter(|p| p.0 >= 8).collect::<Vec<_>>());
            }
            assert!(shapes.len() >= 3, "{seed}: each cast strikes a new zig-zag");
        }
        particles::NO_AMBIENT.with(|c| c.set(false));
    }

    /// Mean column of the main lightning bolt's strike (where it hits the
    /// bottom row, clipped to the magic area) at each cast tick, over several
    /// seeds. Since several bolts strike at once (task 3c) the main bolt is
    /// taken from the bolt plan; the render must paint the bottom row there.
    fn bolt_strike_mean(magic: Magic) -> f32 {
        particles::NO_AMBIENT.with(|c| c.set(true));
        let (mut sum, mut n) = (0.0f32, 0u32);
        for seed in [4, 11, 23, 37, 58, 91] {
            for t in (1..400).filter(|&t| state::casts_at(seed, t)) {
                let p = SpiritParams { magic, ..params(Theme::Lightning, Status::Running, Size::S, t, seed) };
                let strikes = magic::strikes(&geom_for(Size::S, magic), &Ctx::new(&p));
                let col = (strikes[0].round() / 2.0).floor();
                let g = render(&p);
                let row = g.rows - 1;
                let near = (col as u16).saturating_sub(1)..=(col as u16 + 1).min(g.cols - 1);
                assert!(near.into_iter().any(|c| is_braille(cell(&g, c, row).ch)), "{seed} {t} {magic:?}: strike painted at col {col}");
                sum += col + 0.5;
                n += 1;
            }
        }
        particles::NO_AMBIENT.with(|c| c.set(false));
        sum / n.max(1) as f32
    }

    /// Braille dots of a grid as sub-pixel coordinates (x, y).
    fn dots(g: &SpiritGrid) -> std::collections::BTreeSet<(i32, i32)> {
        const BITS: [[u32; 4]; 2] = [[0x01, 0x02, 0x04, 0x40], [0x08, 0x10, 0x20, 0x80]];
        let mut out = std::collections::BTreeSet::new();
        for row in 0..g.rows {
            for col in 0..g.cols {
                let ch = cell(g, col, row).ch;
                if !is_braille(ch) {
                    continue;
                }
                let bits = ch as u32 - 0x2800;
                for (dx, column) in BITS.iter().enumerate() {
                    for (dy, &b) in column.iter().enumerate() {
                        if bits & b != 0 {
                            out.insert((i32::from(col) * 2 + dx as i32, i32::from(row) * 4 + dy as i32));
                        }
                    }
                }
            }
        }
        out
    }

    /// Runs of consecutive x in dot row `y` from `x0` on: (start, length).
    fn dot_runs(d: &std::collections::BTreeSet<(i32, i32)>, y: i32, x0: i32, x1: i32) -> Vec<(i32, i32)> {
        let mut runs: Vec<(i32, i32)> = Vec::new();
        for x in x0..x1 {
            if d.contains(&(x, y)) {
                match runs.last_mut() {
                    Some((s, l)) if *s + *l == x => *l += 1,
                    _ => runs.push((x, 1)),
                }
            }
        }
        runs
    }

    #[test]
    fn cast_phase_winds_up_then_releases_with_each_cast() {
        for seed in [4, 11, 23, 706_312_658] {
            for t in (1..600).filter(|&t| state::casts_at(seed, t)) {
                for k in 1..=3 {
                    if t > k {
                        assert_eq!(state::cast_phase(seed, t - k), Some(state::CastPhase::WindUp((3 - k) as u8)), "{seed} {t}-{k}");
                    }
                }
                assert_eq!(state::cast_phase(seed, t), Some(state::CastPhase::Release(0)));
                assert_eq!(state::cast_phase(seed, t + 1), Some(state::CastPhase::Release(1)));
            }
            // most ticks are at rest
            let rest = (1..600).filter(|&t| state::cast_phase(seed, t).is_none()).count();
            assert!(rest > 400, "{seed}: {rest} rest ticks");
            // the magic spawns on the release frame: the lightning bolt appears exactly then
            particles::NO_AMBIENT.with(|c| c.set(true));
            let c = first_cast(seed);
            let bolt = |t| magic_cells(&render(&params(Theme::Lightning, Status::Running, Size::S, t, seed))).iter().any(|p| p.0 >= 8);
            assert!(bolt(c) && !bolt(c - 1), "{seed}: bolt on the release frame");
            particles::NO_AMBIENT.with(|c| c.set(false));
        }
    }

    #[test]
    fn casting_pose_faces_the_magic() {
        for seed in [4, 11, 706_312_658] {
            for theme in Theme::ALL {
                for t in (0..300).filter(|&t| !eyes::blinking(seed, t)) {
                    let g = render(&params(theme, Status::Running, Size::S, t, seed));
                    let white = white_cells(&g);
                    assert_eq!(white.len(), 2, "{theme:?} {t}: still the only white thing");
                    let rest = render(&params(theme, Status::Idle, Size::S, t, seed));
                    let cells = |w: &[(u16, u16, char)]| w.iter().map(|&(c, r, _)| (c, r)).collect::<Vec<_>>();
                    assert_eq!(cells(&white), cells(&white_cells(&rest)), "{theme:?} {t}: same eye cells");
                    let casting = state::cast_phase(seed, t).is_some();
                    let want = if casting { '\u{2810}' } else { '\u{2802}' };
                    assert!(white.iter().all(|w| w.2 == want), "{theme:?} {t} casting {casting}: {white:?}");
                }
            }
        }
    }

    /// Mean x of the leftmost body dot per dot row (the left side never holds magic).
    fn left_edge(g: &SpiritGrid) -> f32 {
        let d = dots(g);
        let xs: Vec<f32> = (0..i32::from(g.rows) * 4).filter_map(|y| (0..12).find(|&x| d.contains(&(x, y)))).map(|x| x as f32).collect();
        xs.iter().sum::<f32>() / xs.len().max(1) as f32
    }

    #[test]
    fn casting_pose_leans_right_then_recoils() {
        particles::NO_AMBIENT.with(|c| c.set(true));
        for theme in Theme::ALL {
            let (mut wind, mut release, mut rest) = (Vec::new(), Vec::new(), Vec::new());
            for seed in [4, 11, 23, 37, 58, 706_312_658] {
                for t in 1..600 {
                    let e = left_edge(&render(&params(theme, Status::Running, Size::S, t, seed)));
                    match state::cast_phase(seed, t) {
                        Some(state::CastPhase::WindUp(f)) if f >= 1 => wind.push(e),
                        Some(state::CastPhase::Release(_)) => release.push(e),
                        None => rest.push(e),
                        _ => {}
                    }
                }
            }
            let mean = |v: &[f32]| v.iter().sum::<f32>() / v.len() as f32;
            let (w, r, s) = (mean(&wind), mean(&release), mean(&rest));
            println!("{theme:?}: left edge wind-up {w:.2} release {r:.2} rest {s:.2}");
            assert!(w > s + 0.4, "{theme:?}: leans right while winding up ({w} vs rest {s})");
            assert!(r < s - 0.3, "{theme:?}: recoils left on release ({r} vs rest {s})");
        }
        particles::NO_AMBIENT.with(|c| c.set(false));
        // other states never pose
        for t in 0..200 {
            for status in [Status::Idle, Status::Done, Status::NeedsInput, Status::Unknown] {
                let g = render(&params(Theme::Fire, status, Size::S, t, 4));
                assert!(white_cells(&g).iter().all(|w| w.2 == '\u{2802}'), "{status:?} {t}");
            }
        }
    }

    #[test]
    fn fire_breath_starts_tiny_after_a_gap() {
        particles::NO_AMBIENT.with(|c| c.set(true));
        for magic in [Magic::Normal, Magic::Big] {
            let run_len = if magic == Magic::Big { 14 } else { 6 };
            let (mut ok_gap, mut ok_run, mut n) = (0, 0, 0);
            let mut heights = [0f32; 5];
            for seed in [4, 11, 23, 706_312_658] {
                let start = first_cast(seed) + 3;
                for t in start..start + 120 {
                    let g = render(&SpiritParams { magic, ..params(Theme::Fire, Status::Running, Size::S, t, seed) });
                    let d = dots(&g);
                    let h = i32::from(g.rows) * 4;
                    let occupied = |x: i32| (0..h).any(|y| d.contains(&(x, y)));
                    let count = |x: i32| (0..h).filter(|&y| d.contains(&(x, y))).count() as f32;
                    // the body edge: first empty dot column right of the body centre
                    let Some(e) = (7..i32::from(g.cols) * 2).find(|&x| !occupied(x)) else { continue };
                    n += 1;
                    let s = e + 1;
                    if occupied(s) || occupied(s + 1) {
                        ok_gap += 1;
                    }
                    let s = if occupied(s) { s } else { s + 1 };
                    if (s..s + run_len).all(occupied) {
                        ok_run += 1;
                    }
                    for (i, hgt) in heights.iter_mut().enumerate() {
                        *hgt += count(s + i as i32);
                    }
                }
            }
            let hs: Vec<f32> = heights.iter().map(|h| h / n as f32).collect();
            println!("{magic:?}: gap {ok_gap}/{n} run {ok_run}/{n} heights {hs:?}");
            assert!(ok_gap * 10 >= n * 9, "{magic:?}: an empty dot column between body and flame on {ok_gap} of {n} ticks");
            assert!(ok_run * 10 >= n * 8, "{magic:?}: contiguous flame on {ok_run} of {n} ticks");
            assert!(hs[0] <= 1.6, "{magic:?}: the breath starts tiny: {hs:?}");
            assert!(hs.windows(2).all(|w| w[1] >= w[0] - 0.05) && hs[4] >= hs[0] + 1.5, "{magic:?}: the flame widens: {hs:?}");
        }
        particles::NO_AMBIENT.with(|c| c.set(false));
    }

    fn lum((r, g, b): (u8, u8, u8)) -> f32 {
        0.2126 * f32::from(r) + 0.7152 * f32::from(g) + 0.0722 * f32::from(b)
    }

    #[test]
    fn water_vortex_is_glossy_and_shaded() {
        particles::NO_AMBIENT.with(|c| c.set(true));
        for seed in [4, 11, 23] {
            let c = first_cast(seed);
            for magic in [Magic::Normal, Magic::Big] {
                let mut brightest = Vec::new();
                let mut frames = Vec::new();
                for t in c + 5..c + 13 {
                    let g = render(&SpiritParams { magic, ..params(Theme::Water, Status::Running, Size::S, t, seed) });
                    let cells: Vec<(u16, u16, f32)> = magic_cells(&g).into_iter().filter_map(|(col, row)| cell(&g, col, row).fg.map(|fg| (col, row, lum(fg)))).collect();
                    let bands: std::collections::BTreeSet<u32> = cells.iter().map(|c| (c.2 / 50.0) as u32).collect();
                    assert!(bands.len() >= 3, "{seed} {magic:?} {t}: luminance bands {bands:?}");
                    let mean = cells.iter().map(|c| c.2).sum::<f32>() / cells.len() as f32;
                    let top = cells.iter().cloned().fold((0, 0, 0.0f32), |a, b| if b.2 > a.2 { b } else { a });
                    assert!(top.2 > mean + 45.0, "{seed} {magic:?} {t}: highlight {} vs mean {mean}", top.2);
                    // lit from the upper left: the highlight sits up-left of the vortex
                    let (cx, cy) = (cells.iter().map(|c| f32::from(c.0)).sum::<f32>() / cells.len() as f32, cells.iter().map(|c| f32::from(c.1)).sum::<f32>() / cells.len() as f32);
                    assert!(f32::from(top.0) <= cx + 0.5 && f32::from(top.1) <= cy + 0.5, "{seed} {magic:?} {t}: highlight {top:?} vs centre ({cx}, {cy})");
                    brightest.push((top.0, top.1));
                    frames.push(cells.iter().map(|c| (c.0, c.1)).collect::<Vec<_>>());
                }
                assert!(frames.windows(2).filter(|w| w[0] != w[1]).count() >= 4, "{seed} {magic:?}: the arms rotate");
                let still = brightest.windows(2).filter(|w| w[0] == w[1]).count();
                assert!(still * 10 >= (brightest.len() - 1) * 6, "{seed} {magic:?}: the light stays put {brightest:?}");
            }
        }
        particles::NO_AMBIENT.with(|c| c.set(false));
    }

    #[test]
    fn poison_fog_spreads_radially_from_an_anchor() {
        let mut checked = 0;
        for magic in [Magic::Normal, Magic::Big] {
            let g0 = geom_for(Size::S, magic);
            let (ax, ay) = magic::fog_anchor(&g0, magic);
            let life = magic::fog_life(magic);
            let x_min = 15;
            for seed in 1..40u32 {
                let casts: Vec<u64> = (1..800).filter(|&t| state::casts_at(seed, t)).collect();
                for w in casts.windows(3) {
                    let (prev, c, next) = (w[0], w[1], w[2]);
                    if c - prev < life || next - c <= 12 {
                        continue;
                    }
                    let at = |age: u64| -> Vec<(f32, f32)> {
                        let g = render(&SpiritParams { magic, ..params(Theme::Poison, Status::Running, Size::S, c + age, seed) });
                        dots(&g).into_iter().filter(|&(x, _)| x >= x_min).map(|(x, y)| (x as f32 + 0.5 - ax, y as f32 + 0.5 - ay)).collect()
                    };
                    let radius = |d: &[(f32, f32)]| d.iter().map(|p| p.0.hypot(p.1)).fold(0.0f32, f32::max);
                    let (early, late) = (at(2), at(12));
                    assert!(!early.is_empty() && !late.is_empty(), "{magic:?} {seed} {c}: fog shows");
                    assert!(radius(&late) >= radius(&early) + 1.5, "{magic:?} {seed} {c}: grows {} -> {}", radius(&early), radius(&late));
                    if magic == Magic::Big {
                        for d in [&early, &at(6), &late] {
                            let (mx, my) = (d.iter().map(|p| p.0).sum::<f32>() / d.len() as f32, d.iter().map(|p| p.1).sum::<f32>() / d.len() as f32);
                            assert!(mx.abs() <= 2.0 && my.abs() <= 2.0, "{seed} {c}: centroid off the anchor by ({mx}, {my})");
                        }
                    }
                    // thinner at the edge: inner half of the radius is denser than the outer ring
                    let r = radius(&late);
                    let in_canvas = |inner: bool| {
                        let mut n = 0;
                        for y in 0..i32::try_from(g0.h).unwrap_or(16) {
                            for x in x_min..i32::try_from(g0.w).unwrap_or(22) {
                                let dd = (x as f32 + 0.5 - ax).hypot(y as f32 + 0.5 - ay);
                                if (dd < 0.5 * r) == inner && dd <= r {
                                    n += 1;
                                }
                            }
                        }
                        n as f32
                    };
                    let inner = late.iter().filter(|p| p.0.hypot(p.1) < 0.5 * r).count() as f32 / in_canvas(true);
                    let outer = late.iter().filter(|p| p.0.hypot(p.1) >= 0.5 * r).count() as f32 / in_canvas(false);
                    assert!(inner > outer, "{magic:?} {seed} {c}: density inner {inner} vs edge {outer}");
                    checked += 1;
                }
            }
        }
        assert!(checked >= 6, "{checked} isolated casts checked");
    }

    #[test]
    fn lightning_bolts_multiply_and_thicken() {
        particles::NO_AMBIENT.with(|c| c.set(true));
        let mut widths = Vec::new();
        for magic in Magic::ALL {
            let mut counts = std::collections::BTreeSet::new();
            let mut runs = Vec::new();
            for seed in 1..30u32 {
                for t in (1..300).filter(|&t| state::casts_at(seed, t)) {
                    let g = render(&SpiritParams { magic, ..params(Theme::Lightning, Status::Running, Size::S, t, seed) });
                    let d = dots(&g);
                    let w = i32::from(g.cols) * 2;
                    let n = dot_runs(&d, 0, 15, w).len();
                    counts.insert(n);
                    for y in 2..i32::from(g.rows) * 4 - 3 {
                        runs.extend(dot_runs(&d, y, 15, w).into_iter().map(|r| r.1));
                    }
                }
            }
            let mut hist = std::collections::BTreeMap::new();
            for r in &runs {
                *hist.entry(*r).or_insert(0) += 1;
            }
            let mode = hist.iter().max_by_key(|e| e.1).map(|e| *e.0).unwrap_or(0);
            let mean = runs.iter().sum::<i32>() as f32 / runs.len() as f32;
            println!("{magic:?}: bolts per cast {counts:?}, width mode {mode} mean {mean:.2}");
            match magic {
                Magic::Small => assert_eq!(counts, [1].into(), "one thin bolt"),
                Magic::Normal => assert_eq!(counts, [1, 2].into(), "one or two bolts"),
                Magic::Big => assert!(counts.iter().all(|&n| n >= 3), "three or more bolts: {counts:?}"),
            }
            widths.push((mode, mean));
        }
        assert_eq!(widths.iter().map(|w| w.0).collect::<Vec<_>>(), [1, 2, 3], "bolt width in dots: {widths:?}");
        assert!(widths.windows(2).all(|w| w[1].1 > w[0].1), "thicker with more context: {widths:?}");
        particles::NO_AMBIENT.with(|c| c.set(false));
    }

    #[test]
    fn lightning_small_and_normal_strike_far_from_the_body() {
        let [s, n, b] = Magic::ALL.map(bolt_strike_mean);
        println!("bolt strike mean col: S {s:.2} N {n:.2} B {b:.2}");
        // distance from the mouth (body's right edge), in columns
        let g = Size::S.geom();
        let mouth = (g.rcx + (BASE_R - 0.5) * g.ppu) / 2.0;
        let (ds, dn, db) = (s - mouth, n - mouth, b - mouth);
        println!("distance from mouth: S {ds:.2} N {dn:.2} B {db:.2} ({:.0}% / {:.0}%)", ds / db * 100.0, dn / db * 100.0);
        assert!(s <= n && n <= b, "strike order {s} {n} {b}");
        // the 11-column small/normal grid ends at col 10, so the strike (and its clipped sparks) tops out near 50%
        assert!(dn >= 0.47 * db, "normal strike {n} too close vs big {b}");
        assert!(ds >= 0.33 * db, "small strike {s} too close vs big {b}");
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

    /// Prints running frames per element for task 3c: a rest frame, a wind-up
    /// and a release frame (casting pose) and the magic at Normal and Big a few
    /// ticks after a cast (`cargo test dump_cast -- --ignored --nocapture`;
    /// `SPIRIT_ANSI=1` adds 24-bit colour, `SPIRIT_SEED=…`).
    #[test]
    #[ignore]
    fn dump_cast() {
        let seed = std::env::var("SPIRIT_SEED").ok().and_then(|v| v.parse().ok()).unwrap_or(4);
        let ansi = std::env::var("SPIRIT_ANSI").is_ok();
        let c = (40..).find(|&t| state::casts_at(seed, t)).expect("cast");
        let show = |label: &str, frames: &[(String, SpiritGrid)]| {
            println!("{label}");
            println!("{}", frames.iter().map(|f| format!("{:<19}", f.0)).collect::<String>());
            for row in 0..4 {
                let mut line = String::new();
                for (_, g) in frames {
                    for col in 0..g.cols {
                        let x = cell(g, col, row);
                        match (ansi, x.fg) {
                            (true, Some((r, gg, b))) => line += &format!("\x1b[38;2;{r};{gg};{b}m{}\x1b[0m", x.ch),
                            _ => line.push(x.ch),
                        }
                    }
                    line += &" ".repeat(19 - usize::from(g.cols));
                }
                println!("{line}");
            }
            println!();
        };
        for theme in Theme::ALL {
            let at = |t, magic| render(&SpiritParams { magic, ..params(theme, Status::Running, Size::S, t, seed) });
            let rest = (c - 12..c - 3).rev().find(|&t| state::cast_phase(seed, t).is_none() && !eyes::blinking(seed, t)).unwrap_or(c - 6);
            let pose: Vec<(String, SpiritGrid)> = [(rest, "rest"), (c - 2, "wind-up"), (c - 1, "wind-up"), (c, "release"), (c + 1, "release")]
                .iter()
                .map(|&(t, l)| (format!("{l} t{t}"), at(t, Magic::Normal)))
                .collect();
            show(&format!("{theme:?} seed {seed}: casting pose (cast at {c})"), &pose);
            for magic in [Magic::Normal, Magic::Big] {
                let frames: Vec<(String, SpiritGrid)> = [c + 3, c + 6, c + 10].iter().map(|&t| (format!("{magic:?} t{t}"), at(t, magic))).collect();
                show(&format!("{theme:?} {magic:?}"), &frames);
            }
        }
    }

    /// Prints a running spirit of every element at each magic level
    /// (`cargo test dump_magic -- --ignored --nocapture`, `SPIRIT_TICK=…`).
    #[test]
    #[ignore]
    fn dump_magic() {
        let ticks: Vec<u64> = std::env::var("SPIRIT_TICK").unwrap_or_else(|_| "40".into()).split(',').filter_map(|v| v.trim().parse().ok()).collect();
        for (tick, magic) in ticks.into_iter().flat_map(|t| Magic::ALL.map(|m| (t, m))) {
            println!("tick {tick} {magic:?}");
            let grids: Vec<SpiritGrid> =
                Theme::ALL.iter().map(|&theme| render(&SpiritParams { magic, ..params(theme, Status::Running, Size::S, tick, 4) })).collect();
            for row in 0..grids[0].rows {
                let mut line = String::new();
                for g in &grids {
                    for col in 0..g.cols {
                        line.push(cell(g, col, row).ch);
                    }
                    line += &" ".repeat(19 - usize::from(g.cols));
                }
                println!("{line}");
            }
        }
    }
}
