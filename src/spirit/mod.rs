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

/// How strong the cast magic (RUNNING particles) is; driven by context usage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Magic {
    /// few, tiny, short-range particles
    Small,
    /// the approved reference look
    #[default]
    Normal,
    /// more, larger particles that fly farther (on a wider canvas)
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
    /// to the right for cast particles. The body centre sits at 1/3 of the
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
/// is reserved for cast particles, so this sits left of the grid centre).
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
                    assert!(white.iter().all(|w| w.2 == '\u{2802}'), "one white dot (⠂) per eye: {white:?}");
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
            let eyes_of = |status| white_cells(&render(&params(theme, status, size, t, seed)));
            let base = eyes_of(Status::Idle);
            for status in STATUSES {
                let e = eyes_of(status);
                if status == Status::Running {
                    // a running spirit may glance one cell to the right, where it casts
                    let shifted: Vec<_> = base.iter().map(|&(c, r, ch)| (c + 1, r, ch)).collect();
                    assert!(e == base || e == shifted, "{theme:?} {size:?}: {e:?} vs {base:?}");
                } else {
                    assert_eq!(e, base, "{theme:?} {status:?} {size:?}");
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
            let ice = |t| element::ice_facets(seed, at(Theme::Ice, t).tf, at(Theme::Ice, t).sp).1;
            assert!((ice(400) - ice(0)).abs() > 2.0, "{seed}: ice facets rotate");
            for i in 0..4 {
                let ph: Vec<f32> = (0..60).map(|t| element::star_phase(&at(Theme::Ice, t), i)).collect();
                assert!(ph.windows(2).all(|w| w[0] != w[1]), "{seed}: star {i} twinkles");
            }
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
        let p = params(Theme::Ice, Status::Running, Size::M, 300, 706_312_658);
        let fresh = render(&p);
        for t in 0..300 {
            render(&SpiritParams { tick: t, ..p });
        }
        render(&SpiritParams { seed: 1, ..p });
        assert_eq!(render(&p), fresh);
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

    /// Prints a running spirit of every element at each magic level
    /// (`cargo test dump_magic -- --ignored --nocapture`, `SPIRIT_TICK=…`).
    #[test]
    #[ignore]
    fn dump_magic() {
        let tick = std::env::var("SPIRIT_TICK").ok().and_then(|v| v.parse().ok()).unwrap_or(40);
        for magic in Magic::ALL {
            println!("{magic:?}");
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
