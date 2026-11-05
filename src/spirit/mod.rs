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
//! - State ([`Status`]): RUNNING churns fast and leans in short bursts,
//!   NEEDS_INPUT shows a bobbing `?` and a pulsing glow, IDLE is dim and slow
//!   with a `z`, DONE sparkles and bounces, UNKNOWN is grey.
//! - Eyes are the same in every state (see `eyes`); during a burst the eye
//!   dot looks one sub-pixel to the right.
//! - Burst pose (RUNNING): a short wind-up leaning right and a small recoil
//!   on release (see `state::pose`).
//! - Context stage ([`Stage`]): relaxed spirits
//!   barely move, lively ones are the reference look, struggling ones
//!   tremble, pant and sweat (see `state::bob_at`, `state::sweat`).

mod body;
mod canvas;
mod element;
mod eyes;
mod noise;
mod particles;
mod state;
mod subdots;
pub mod theme;

use crate::model::Status;
use canvas::Geom;
pub use subdots::{render_beads, SubagentDot, MAX_DOTS};
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

/// Context usage (percent) at which the spirit goes from relaxed to lively,
/// and from lively to struggling.
const STAGE_LIVELY_AT: f32 = 30.0;
const STAGE_STRUGGLING_AT: f32 = 70.0;
/// Context usage above which the body flickers red.
const WARNING_ABOVE: f32 = 90.0;
/// How hard the spirit works, driven by context usage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Stage {
    /// stage 1 (< 30 %): slow, tiny bob, calm churn, few ambient bits
    Relaxed,
    /// stage 2: the reference motion
    #[default]
    Lively,
    /// stage 3 (>= 70 %): trembling, panting, sweat drops, effortful casts
    Struggling,
}

/// Context stage for a context usage: < 30 % relaxed, 30..70 % lively,
/// >= 70 % struggling; unknown or invalid usage is lively.
pub fn stage_for(ctx_percent: Option<f32>) -> Stage {
    match ctx_percent {
        Some(p) if p.is_nan() || p < 0.0 => Stage::Lively,
        Some(p) if p >= STAGE_STRUGGLING_AT => Stage::Struggling,
        Some(p) if p >= STAGE_LIVELY_AT => Stage::Lively,
        Some(_) => Stage::Relaxed,
        None => Stage::Lively,
    }
}

/// Whether the context is nearly full (> 90 %): the body flickers red.
pub fn warning(ctx_percent: Option<f32>) -> bool {
    ctx_percent.is_some_and(|p| p > WARNING_ABOVE)
}

/// Spirit size step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Size {
    /// The standard card's small spirit: a round body of about 5 x 3 cells
    /// (10 x 10 sub-pixels, body radius 4.75 sub-pixels) centred in a 7 x 4
    /// grid. Its eyes are two single white dots (see `eyes`); same cues,
    /// element looks, pose and stages as S.
    XS,
    S,
}

impl Size {
    /// Every size is 4 rows tall (16 sub-pixels), the card band; bigger
    /// steps get a bigger body. The body centre sits at 1/3 of the width (as in the reference
    /// design) and high enough that the eye row (cell row 1) has body above
    /// and below it.
    const fn geom(self) -> Geom {
        match self {
            // 11 x 4 cells, body radius 5.5 sub-pixels
            Size::S => Geom { ppu: 5.5 / BASE_R, w: 22, h: 16, rcx: 7.0, rcy: 8.0, small: true, dot_eyes: false },
            // 7 x 4 cells, body radius 4.75 sub-pixels: the round body
            // fills cells 1..5 and is centred on the band's middle row line
            Size::XS => Geom { ppu: 4.75 / BASE_R, w: 14, h: 16, rcx: 7.0, rcy: 8.0, small: true, dot_eyes: true },
        }
    }
}

/// Horizontal centre of the body in grid columns (left of the grid centre).
#[cfg_attr(not(test), allow(dead_code))]
pub fn body_center_x(size: Size) -> f32 {
    size.geom().rcx / 2.0
}

/// Grid size (columns, rows) of a spirit of `size`.
pub const fn grid_size(size: Size) -> (u16, u16) {
    let g = size.geom();
    ((g.w / 2) as u16, (g.h / 4) as u16)
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
    /// how hard the spirit works (context usage)
    pub stage: Stage,
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
    stage: Stage,
    /// body clock (outline, churn, decorations): `t`/`tf` scaled by the stage
    bt: u64,
    btf: f32,
    /// outline animation speed of the state
    sp: f32,
    small: bool,
    /// water ripple line in design units, inside this size's canvas
    floor: f32,
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
        let g = p.size.geom();
        let (bt, btf) = state::body_clock(p.stage, p.tick);
        Ctx {
            elem: p.theme,
            state: p.status,
            seed: p.seed,
            t: p.tick,
            tf: phase_tick(p.tick),
            stage: p.stage,
            bt,
            btf,
            sp: state::speed(p.status),
            small: g.small,
            floor: floor_y(&g),
            right: DW,
        }
    }
}

/// Renders one frame of a spirit.
pub fn render(p: &SpiritParams) -> SpiritGrid {
    render_with(p, true)
}

/// Sub-pixels per design unit of bob of a small body: its real scale would
/// round most bobs to 0, so it moves like an S body (one sub-pixel at the
/// peaks of a lively bob, more often when struggling).
const SHORT_BOB_PPU: f32 = 0.65;

/// Body bob plus the casting pose's vertical shift, in real sub-pixels.
fn bob_px(ctx: &Ctx, g: &Geom, pose: &state::Pose) -> f32 {
    let k = if g.dot_eyes { SHORT_BOB_PPU } else { g.ppu };
    (state::bob_at(ctx.stage, ctx.state, ctx.seed, ctx.tf) * k).round() + pose.dy
}

/// Casting pose plus the struggling tremble (added to the horizontal shift).
fn pose_of(ctx: &Ctx) -> state::Pose {
    let pose = state::pose_at(ctx.stage, ctx.elem, ctx.state, ctx.seed, ctx.t);
    match ctx.stage {
        Stage::Struggling => state::Pose { dx: pose.dx + state::tremble(ctx.stage, ctx.state, ctx.seed, ctx.t), ..pose },
        Stage::Relaxed | Stage::Lively => pose,
    }
}

/// Renders one frame; `bubbles: false` leaves the poison bubbles out (a seam
/// for tests: bubbles only ever lighten and add to the frame without them).
fn render_with(p: &SpiritParams, bubbles: bool) -> SpiritGrid {
    let g = p.size.geom();
    let ctx = Ctx::new(p);
    let parts = particles::simulate(&ctx);
    let pulse = state::pulse(ctx.state, ctx.tf);
    let pose = pose_of(&ctx);
    let bob_px = bob_px(&ctx, &g, &pose);
    let look = body::Look {
        cy: CY + bob_px / g.ppu,
        pulse,
        bright: state::brightness(ctx.state, pulse),
        flash: element::flashing(&ctx),
        warn: p.warning && (p.tick / WARN_HALF_PERIOD) % 2 == 0,
        dx: pose.dx,
        tilt: pose.tilt,
        swell: pose.swell,
        squash: state::squash(ctx.stage, ctx.state, ctx.seed, ctx.tf),
    };
    let mut cv = canvas::Canvas::new(g);
    body::draw(&mut cv, &ctx, &look);
    element::decorate(&mut cv, &ctx, look.cy);
    particles::draw(&mut cv, &parts);
    state::sweat(&mut cv, &ctx, look.cy);
    state::sparkles(&mut cv, &ctx);
    if bubbles {
        element::draw_bubbles(&mut cv, &ctx, CX + pose.dx / g.ppu, look.cy);
    }
    eyes::draw(&mut cv, pose.facing, eyes::blinking(ctx.seed, ctx.t), element::ramp(ctx.elem)[1], look.bright);
    let mut grid = cv.cells();
    state::marks(&mut grid, &cv, &ctx, pulse);
    grid
}

/// Sub-pixel centre (x, y) and stage of every poison bubble drawn on a frame.
#[cfg(test)]
fn bubble_spots(p: &SpiritParams) -> Vec<(i32, i32, element::Stage)> {
    let g = p.size.geom();
    let ctx = Ctx::new(p);
    let pose = pose_of(&ctx);
    let cy = CY + bob_px(&ctx, &g, &pose) / g.ppu;
    let cv = canvas::Canvas::new(g);
    element::bubble_spots(&cv, &ctx, CX + pose.dx / g.ppu, cy).into_iter().map(|(x, y, b)| (x, y, b.stage)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Status;
    use theme::Theme;

    const STATUSES: [Status; 5] = [Status::Running, Status::NeedsInput, Status::Idle, Status::Done, Status::Unknown];
    const WHITE: Option<(u8, u8, u8)> = Some((255, 255, 255));

    fn params(theme: Theme, status: Status, size: Size, tick: u64, seed: u32) -> SpiritParams {
        SpiritParams { theme, status, size, tick, seed, warning: false, stage: Stage::Lively }
    }

    fn cell(g: &SpiritGrid, col: u16, row: u16) -> &Cell {
        &g.cells[usize::from(row) * usize::from(g.cols) + usize::from(col)]
    }

    /// (col, row, char) of every eye catchlight: a cell drawn in pure white
    /// with a single dot. Poison bubbles may be pure white too, but never a
    /// lone dot (see `only_eyes_and_poison_bubbles_are_pure_white`).
    fn white_cells(g: &SpiritGrid) -> Vec<(u16, u16, char)> {
        let mut out = Vec::new();
        for row in 0..g.rows {
            for col in 0..g.cols {
                let c = cell(g, col, row);
                if c.fg == WHITE && bits(c).count_ones() == 1 {
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
                for size in [Size::S] {
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
        // card variant: every size fits the 4 rows of a card
        assert_eq!(grid_size(Size::S), (11, 4));
        for size in [Size::S] {
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
        assert!(dots(Size::XS) < dots(Size::S));
    }

    #[test]
    fn every_state_and_theme_renders_nonempty_grid() {
        for theme in Theme::ALL {
            for status in STATUSES {
                for size in [Size::S] {
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
            for (theme, size) in Theme::ALL.into_iter().flat_map(|t| [Size::S].map(|s| (t, s))) {
                for status in STATUSES {
                    let g = render(&params(theme, status, size, t, seed));
                    let white = white_cells(&g);
                    assert_eq!(white.len(), 2, "{theme:?} {status:?} {size:?}: {white:?}");
                    // during a burst: the dot faces right, one sub-pixel in the same cell (⠐)
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
            for size in [Size::S] {
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
        for (theme, size) in Theme::ALL.into_iter().flat_map(|t| [Size::S].map(|s| (t, s))) {
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
        for (theme, size) in Theme::ALL.into_iter().flat_map(|t| [Size::S].map(|s| (t, s))) {
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
        for (theme, size) in Theme::ALL.into_iter().flat_map(|t| [Size::S].map(|s| (t, s))) {
            for t in 0..60 {
                let g = render(&params(theme, Status::Idle, size, t, 2));
                assert!(chars(&g).iter().any(|c| c.2 == 'z' || c.2 == 'Z'), "{theme:?} {size:?} {t}");
            }
            let other = render(&params(theme, Status::Done, Size::S, 5, 2));
            assert!(!chars(&other).iter().any(|c| c.2 == 'z' || c.2 == 'Z'));
        }
    }

    #[test]
    fn warning_only_above_90() {
        assert!(!warning(None));
        assert!(!warning(Some(f32::NAN)));
        assert!(!warning(Some(50.0)));
        assert!(!warning(Some(70.0)), "stage 3 alone does not warn");
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
            for (t, size) in [0, 20, 45].into_iter().flat_map(|t| [Size::S].map(|s| (t, s))) {
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
        let p = params(Theme::Poison, Status::Running, Size::S, 300, 706_312_658);
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

    /// Body cells (columns 0..7, the body area) that carry a colour,
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
                    // body cells a bubble does not touch
                    let bare = render_with(&params(Theme::Poison, status, Size::S, t, seed), false);
                    let body: Vec<Cell> = body_cells(&g).into_iter().filter(|c| body_cells(&bare).contains(c)).collect();
                    assert!(body.len() >= 10, "{status:?} {t}: {} body cells", body.len());
                    let green = body.iter().filter(|c| c.fg.is_some_and(|(r, gg, b)| gg > r && gg > b)).count();
                    assert!(green * 10 >= body.len() * 9, "{status:?} {t}: {green} of {} cells green", body.len());
                }
            }
        }
    }

    #[test]
    fn poison_bubbles_form_and_pop() {
        for seed in [3, 41, 706_312_658] {
            let p = |t| params(Theme::Poison, Status::Running, Size::S, t, seed);
            // a bubble is visible on frame t when it changes the grid; its stage
            // comes from the bubble schedule
            let visible = |t: u64, stage| {
                let b = element::bubbles(&Ctx::new(&p(t)));
                b.iter().any(|b| b.stage == stage) && render_with(&p(t), true) != render_with(&p(t), false)
            };
            // a ring shows on the body, then pops into sparkles a few ticks later
            let popped = (0..236).filter(|&t| visible(t, element::Stage::Ring) && (t + 1..=t + 3).any(|u| visible(u, element::Stage::Pop))).count();
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

    /// Braille dot bits of a cell (0 for blanks and text marks).
    fn bits(c: &Cell) -> u32 {
        if is_braille(c.ch) {
            c.ch as u32 - 0x2800
        } else {
            0
        }
    }

    /// Every poison frame that can hold a bubble (running incl. casting,
    /// needs-input, idle, done), each rendered with and without bubbles:
    /// several seeds and sizes over a long tick window and at a long uptime.
    fn bubble_frames(mut f: impl FnMut(&SpiritParams, &SpiritGrid, &SpiritGrid)) {
        let ticks = (0..260).chain(40_000_000..40_000_080);
        for seed in [3, 41, 706_312_658, u32::MAX] {
            for status in [Status::Running, Status::NeedsInput, Status::Idle, Status::Done] {
                for size in [Size::S] {
                    for t in ticks.clone() {
                        let p = params(Theme::Poison, status, size, t, seed);
                        f(&p, &render_with(&p, true), &render_with(&p, false));
                    }
                }
            }
        }
    }

    #[test]
    fn poison_bubbles_never_erase_body_dots() {
        let (mut drawn, mut cast) = (0, 0);
        bubble_frames(|p, with, without| {
            if with != without {
                drawn += 1;
                cast += usize::from(p.status == Status::Running && state::cast_phase(p.seed, p.tick).is_some());
            }
            for (i, (a, b)) in with.cells.iter().zip(&without.cells).enumerate() {
                // every dot painted without bubbles is still there with them
                assert_eq!(bits(a) & bits(b), bits(b), "{p:?} cell {i}: {b:?} -> {a:?}");
                if b.fg.is_some() {
                    assert!(a.fg.is_some(), "{p:?} cell {i}: painted cell emptied");
                }
            }
        });
        assert!(drawn > 1000 && cast > 50, "bubbles show ({drawn} frames, {cast} while casting)");
    }

    #[test]
    fn poison_bubbles_never_darken_the_body() {
        bubble_frames(|p, with, without| {
            for (i, (a, b)) in with.cells.iter().zip(&without.cells).enumerate() {
                if let (Some(fa), Some(fb)) = (a.fg, b.fg) {
                    assert!(lum(fa) + 0.5 >= lum(fb), "{p:?} cell {i}: {fb:?} darkened to {fa:?}");
                }
            }
        });
    }

    fn neutral((r, g, b): (u8, u8, u8)) -> bool {
        r.abs_diff(g) <= 8 && g.abs_diff(b) <= 8
    }

    #[test]
    fn poison_bubble_rings_and_sparkles_are_white() {
        let (mut rings, mut sparkles) = (0, 0);
        bubble_frames(|p, with, without| {
            let b = element::bubbles(&Ctx::new(p));
            let swell = b.iter().any(|b| b.stage == element::Stage::Swell);
            let pop0 = b.iter().all(|b| b.stage == element::Stage::Pop && b.frame == 0);
            for (a, o) in with.cells.iter().zip(&without.cells) {
                if a == o {
                    continue;
                }
                let fa = a.fg.expect("a bubble cell is painted");
                // a bubble on an empty spot is drawn in white tones alone
                if o.fg.is_none() {
                    assert!(neutral(fa), "{p:?}: bubble dots {fa:?} on the card are not neutral white");
                    if pop0 && !b.is_empty() {
                        assert!(lum(fa) >= 200.0, "{p:?}: sparkle {fa:?} too dim");
                        sparkles += 1;
                    }
                }
                // the swelling ring with its glint turns its cells white
                if swell && neutral(fa) && fa.0.min(fa.1).min(fa.2) >= 250 {
                    rings += 1;
                }
            }
        });
        assert!(rings > 300, "white ring cells ({rings})");
        assert!(sparkles > 30, "white pop sparkles on the card ({sparkles})");
    }

    #[test]
    fn poison_bubbles_have_no_warm_tint() {
        let mut lit = 0;
        bubble_frames(|p, with, without| {
            for (i, (a, o)) in with.cells.iter().zip(&without.cells).enumerate() {
                if a == o {
                    continue;
                }
                let (r, g, b) = a.fg.expect("a bubble cell is painted");
                match o.fg {
                    // white blended into the green body: lighter, never warmer
                    // than the body, never redder than green
                    Some((or, og, ob)) => {
                        assert!(lum((r, g, b)) + 0.5 >= lum((or, og, ob)), "{p:?} cell {i}: {:?} -> {:?} darker", o.fg, a.fg);
                        let warm = i32::from(r) - i32::from(b);
                        let warm0 = (i32::from(or) - i32::from(ob)).max(0);
                        assert!(warm <= warm0 + 8, "{p:?} cell {i}: {:?} -> {:?} turns warm", o.fg, a.fg);
                        assert!(r <= g.saturating_add(8), "{p:?} cell {i}: {:?} -> {:?} redder than green", o.fg, a.fg);
                        lit += 1;
                    }
                    None => assert!(neutral((r, g, b)), "{p:?} cell {i}: {:?} not neutral", a.fg),
                }
            }
        });
        assert!(lit > 1000, "bubbles light the body ({lit} cells)");
    }

    #[test]
    fn only_eyes_and_poison_bubbles_are_pure_white() {
        for seed in [3, 41, 706_312_658] {
            for (theme, size) in Theme::ALL.into_iter().flat_map(|t| [Size::S].map(|s| (t, s))) {
                for status in STATUSES {
                    for t in 0..120 {
                        let p = params(theme, status, size, t, seed);
                        let (g, bare) = (render_with(&p, true), render_with(&p, false));
                        let cv = canvas::Canvas::new(size.geom());
                        let eye_bottom = eyes::boxes(&cv).iter().map(|b| b.1 + b.3).max().unwrap_or(0) as u16;
                        for (i, c) in g.cells.iter().enumerate().filter(|(_, c)| c.fg == WHITE) {
                            if bits(c).count_ones() == 1 && bare.cells[i] == *c {
                                continue; // an eye catchlight
                            }
                            let row = i as u16 / g.cols;
                            assert!(theme == Theme::Poison && bare.cells[i] != *c, "{p:?} cell {i}: {c:?} pure white outside the eyes");
                            assert!(bits(c).count_ones() >= 2 && row >= eye_bottom, "{p:?} cell {i}: bubble {c:?} could pass for an eye");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn poison_pop_sparkles_fade_into_their_surroundings() {
        let mut checked = 0;
        bubble_frames(|p, with, without| {
            let b = element::bubbles(&Ctx::new(p));
            // frames where every bubble is on the last pop frame
            if b.is_empty() || !b.iter().all(|b| b.stage == element::Stage::Pop && b.frame == element::LAST_POP_FRAME) {
                return;
            }
            for (i, (a, o)) in with.cells.iter().zip(&without.cells).enumerate() {
                if a == o {
                    continue;
                }
                let fa = a.fg.expect("a sparkle cell is painted");
                match o.fg {
                    Some(fo) => assert!((lum(fa) - lum(fo)).abs() <= 20.0, "{p:?} cell {i}: {fa:?} vs surrounding {fo:?}"),
                    None => assert!(lum(fa) <= 60.0, "{p:?} cell {i}: {fa:?} has not faded to the card"),
                }
                checked += 1;
            }
        });
        assert!(checked > 30, "last pop frames seen ({checked} cells)");
    }

    #[test]
    fn poison_bubbles_leave_the_eyes_alone() {
        bubble_frames(|p, with, without| {
            let cv = canvas::Canvas::new(p.size.geom());
            for (col0, row0, w, h) in eyes::boxes(&cv) {
                for (c, r) in (row0..row0 + h).flat_map(|r| (col0..col0 + w).map(move |c| (c as u16, r as u16))) {
                    assert_eq!(cell(with, c, r), cell(without, c, r), "{p:?} eye cell ({c}, {r})");
                }
            }
            assert_eq!(white_cells(with), white_cells(without), "{p:?}");
        });
        // the bubbles that are drawn never sit on the eye rows
        for seed in [3, 41, 706_312_658] {
            for size in [Size::S] {
                for t in 0..400 {
                    let p = params(Theme::Poison, Status::Done, size, t, seed);
                    let cv = canvas::Canvas::new(size.geom());
                    let bottom = eyes::boxes(&cv).iter().map(|b| (b.1 + b.3) * 4).max().unwrap_or(0);
                    for (_, py, _) in bubble_spots(&p) {
                        assert!(py - 2 >= bottom, "{p:?}: bubble at row {py} reaches the eyes (bottom {bottom})");
                    }
                }
            }
        }
    }

    /// Prints a poison bubble growing and popping on the body, with and
    /// without colour (`cargo test dump_bubble -- --ignored --nocapture`;
    /// `SPIRIT_ANSI=1`, `SPIRIT_SEED=…`).
    #[test]
    #[ignore]
    fn dump_bubble() {
        let seed = std::env::var("SPIRIT_SEED").ok().and_then(|v| v.parse().ok()).unwrap_or(41);
        let ansi = std::env::var("SPIRIT_ANSI").is_ok();
        let p = |t| params(Theme::Poison, Status::Done, Size::S, t, seed);
        let swell = (20..2000).find(|&t| {
            let b = |t| element::bubbles(&Ctx::new(&p(t)));
            b(t).len() == 1 && b(t)[0].stage == element::Stage::Swell && !eyes::blinking(seed, t) && !eyes::blinking(seed, t + 3)
        });
        let s = swell.expect("a swell frame");
        let frames: Vec<(String, SpiritGrid)> =
            [(s - 2, "ring"), (s, "swell"), (s + 1, "pop 0"), (s + 2, "pop 1"), (s + 3, "pop 2")].iter().map(|&(t, l)| (format!("{l} t{t}"), render(&p(t)))).collect();
        println!("{}", frames.iter().map(|f| format!("{:<16}", f.0)).collect::<String>());
        for row in 0..4 {
            let mut line = String::new();
            for (_, g) in &frames {
                for col in 0..g.cols {
                    let x = cell(g, col, row);
                    match (ansi, x.fg) {
                        (true, Some((r, gg, b))) => line += &format!("\x1b[38;2;{r};{gg};{b}m{}\x1b[0m", x.ch),
                        _ => line.push(x.ch),
                    }
                }
                line += &" ".repeat(16 - usize::from(g.cols));
            }
            println!("{line}");
        }
        // what the bubble changes, cell by cell, against the same frame without it
        for (label, g) in &frames {
            let t: u64 = label.rsplit('t').next().and_then(|v| v.parse().ok()).unwrap_or(0);
            let bare = render_with(&p(t), false);
            let diff: Vec<String> = (0..g.rows)
                .flat_map(|r| (0..g.cols).map(move |c| (c, r)))
                .filter(|&(c, r)| cell(g, c, r) != cell(&bare, c, r))
                .map(|(c, r)| format!("({c},{r}) {}{:?} -> {}{:?}", cell(&bare, c, r).ch, cell(&bare, c, r).fg, cell(g, c, r).ch, cell(g, c, r).fg))
                .collect();
            println!("{label}: {}", diff.join(", "));
        }
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
        }
    }

    #[test]
    fn burst_pose_faces_right() {
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

    /// Mean x of the leftmost body dot per dot row .
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

    fn lum((r, g, b): (u8, u8, u8)) -> f32 {
        0.2126 * f32::from(r) + 0.7152 * f32::from(g) + 0.0722 * f32::from(b)
    }

    /// FNV-1a over every cell's glyph and colour.
    fn grid_hash(g: &SpiritGrid) -> u64 {
        let mut h = 0xcbf2_9ce4_8422_2325u64;
        let mut eat = |v: u32| {
            for b in v.to_le_bytes() {
                h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
            }
        };
        eat(u32::from(g.cols) << 16 | u32::from(g.rows));
        for c in &g.cells {
            eat(c.ch as u32);
            eat(c.fg.map_or(u32::MAX, |(r, gg, b)| u32::from(r) << 16 | u32::from(gg) << 8 | u32::from(b)));
        }
        h
    }

    /// (size, tick, seed, warning) of the golden frames, rendered for
    /// every element and state.
    const GOLDEN_CASES: [(Size, u64, u32, bool); 8] = [
        (Size::S, 0, 7, false),
        (Size::S, 37, 7, false),
        (Size::S, 121, 706_312_658, false),
        (Size::S, 500, 3, false),
        (Size::S, 77, 11, false),
        (Size::S, 260, 41, false),
        (Size::S, 40_000_013, u32::MAX, false),
        (Size::S, 16, 23, true),
    ];

    fn golden_params() -> Vec<SpiritParams> {
        let mut out = Vec::new();
        for theme in Theme::ALL {
            for status in STATUSES {
                for (size, tick, seed, warning) in GOLDEN_CASES {
                    out.push(SpiritParams { warning, ..params(theme, status, size, tick, seed) });
                }
            }
        }
        out
    }

    /// Hashes of the golden frames, captured when the cast magic was removed
    /// (theme-major, then state, then `GOLDEN_CASES`); non-running frames kept
    /// their earlier hashes. `SPIRIT_GOLDEN_PRINT=1` prints them.
    const GOLDEN: [u64; 160] = [
        0x3512407e8b3be050, 0x1a944c84fcb99d05, 0xbe2dd4fda8169939, 0xf78aed37934e6cc1, 0xd690876fb93c5da4, 0xe4e2d8155b511f11,
        0xa6c4b96194b8ba3f, 0x9af8c26b2e7cb1df, 0xe63151fe9a9034cc, 0x7a8ac6628408efd6, 0xe7c8d4a08b21e8dd, 0xfbf400b022b97576,
        0x429907e3ea984a4f, 0x3d7198f183a6f699, 0xe495d0e08a6d0ac3, 0x360e578976d6b475, 0xef4f5337fe84e25d, 0x9491d9830f79a16d,
        0xfade110e02982b10, 0x641f43bba1b4a26f, 0x1d72b208cadb2612, 0xb35def89ef4ff9c9, 0x061b319b31d2279b, 0xae106a39f3427582,
        0xb07554c2511c01b3, 0x7c2ccbca01365697, 0x7324e7ce2c9bce28, 0x7f90b66dc3b18a22, 0x28e0813a9098126a, 0xddb08feddde44f72,
        0x4b66d6c3940a9054, 0x0f45b90a4d2df7c3, 0x9f8f36f54ef0c55f, 0x3bc34a7c5a30173a, 0x795315231571603c, 0x4553c14003a5c15b,
        0xb7b728bbd35dd439, 0x4684d7e93e1ecf26, 0x8e336f2c84a30f29, 0x9033c7cabef7dcce, 0xe0f1d9d204dbb53b, 0x76f62a4c58a69e90,
        0x711d6e1311990a8b, 0xc6f0bb9601aac739, 0xe5288dcb73e21e04, 0x73ace2a07c91d87f, 0x80456e2f4bef6966, 0x405ec8fdbde542df,
        0x623cb10f33e21bd2, 0xb81b2bfc61e53c3b, 0xeba92b71721f1094, 0x070f6ce331078343, 0x9777c8597b5d44cd, 0xcee480f5bb3ba604,
        0x43a1725cb0226326, 0x67fdef386bcdef4a, 0xd82350e5a41e871f, 0x6eda02e347dd31c1, 0x03f9c0c849cfb49c, 0x00a2dc8f8311e737,
        0xcd9043024643de03, 0x6b4b5378291cb075, 0xa9f68fe65f01805d, 0x3e5bad230a45cfd7, 0xa98ffb855054ad80, 0x5e2ff43d23c35201,
        0x12d998a33761e63a, 0xd87d3ac9ce0ccd35, 0x7a572e991b8d4aa9, 0x97a0921451361b80, 0x1175660964169ba9, 0xd63cde832ab3eb0e,
        0x48397efc5c8c3b41, 0x21da5b3f472a49f0, 0xec21ee809d8c883b, 0x2b36b7ce76972c42, 0x147c72413298e7b7, 0x4f3d4438efbd360d,
        0xbc71562d40e87921, 0xf15983afbd5a66d6, 0x17ba577ce79f9d69, 0x942a3d8235af424f, 0x46f9b1e4182a2d2a, 0xda2deb338b432aff,
        0x0383e4e008a7ae42, 0x03d6036ebc155502, 0x134711d647479693, 0xe83b4b7cbada183f, 0x6e6f83e990bad121, 0xe8c937a5b5565162,
        0x06b998dbd08c3922, 0x75a4c4f89851057b, 0x9fdd8155df8d757e, 0xbf9b1e45c6f184c3, 0x7c9fe874789f9770, 0xbdfa8d0c6164ec15,
        0xaf99522f19ebd5d3, 0x7ed6f69b81fdce30, 0x59431f82976b233e, 0x1f9f1ae805eefe51, 0xccfd0b24f4222bb8, 0x91a61bcdb1643167,
        0xa2abcb9a1c81e6cf, 0x3f47116c7a438ad6, 0x7850eb98fd19d4ac, 0x1505f53f641c81d0, 0x8bb55120b1e5e349, 0xf3447a81695c432e,
        0x5be297e2c426ab13, 0x8244f1770964617e, 0xb22b1b31160a8e92, 0x8649e9e257ac4035, 0xc51a594a36d6eb0b, 0x0443307dc0b76eab,
        0x86a09c1e3c1e4b46, 0x248c01ef325d4d31, 0x420f51d495f2a608, 0x0c25d9cff723c5bf, 0xbd1d2e032c80cc3c, 0xb35321a2a8c8dbf8,
        0xef794c374d54696e, 0xd39652c7a2c18969, 0xba6346cfd3b9e32b, 0x604e50d6400e3e5b, 0x87ef16552c58ea26, 0xb86910b66f35c897,
        0xb5d0a2f8085c1712, 0x4e1be45bde2e41fc, 0xfb3713c21a6230d7, 0xb33e274192e20762, 0xe8104ddd4a1dcb9a, 0x55963dbf618bce81,
        0xc82e649859a5126d, 0x843fad5de4e5cdd0, 0x4bfeeef794af17dc, 0xbd018aafa4a4202f, 0x4b2bd8f000ed4f8e, 0xc742374a9609c975,
        0xe5d5ea037ca3c896, 0xe757adf3f43a9a51, 0xef1e616b72d85bd2, 0x9a84ac2dd347ba31, 0x18d61c2e4f299246, 0x108a70c8603a8625,
        0xd722fa259fae0021, 0x769a96d2255c9b88, 0x8063af057cf6a7a6, 0xb18e90e744233e42, 0x3cae6af5a2da686f, 0x596551fe955412ba,
        0x5a3c2419c49743dd, 0xc1d40b4db9a98e9a, 0xbdd86d15bb794e41, 0xaa80d38b83ba5833, 0x8b7c9e6147ebb050, 0x253e3edb1252f5fc,
        0xe100fe8fdd1afb11, 0xc9e7612e6d630db6, 0x209e4f6442e3bc78, 0x3333d3cd9698332f,
    ];

    #[test]
    fn stage_two_matches_the_pre_stage_renders() {
        let hashes: Vec<u64> = golden_params().iter().map(|p| grid_hash(&render(p))).collect();
        if std::env::var("SPIRIT_GOLDEN_PRINT").is_ok() {
            for chunk in hashes.chunks(8) {
                println!("        {},", chunk.iter().map(|h| format!("0x{h:016x}")).collect::<Vec<_>>().join(", "));
            }
        }
        for (i, (p, (got, want))) in golden_params().iter().zip(hashes.iter().zip(GOLDEN)).enumerate() {
            assert_eq!(*got, want, "golden frame {i}: {p:?}");
        }
    }

    fn staged(theme: Theme, status: Status, stage: Stage, tick: u64, seed: u32) -> SpiritParams {
        SpiritParams { stage, ..params(theme, status, Size::S, tick, seed) }
    }

    #[test]
    fn stage_for_thresholds() {
        let cases: [(Option<f32>, Stage); 13] = [
            (None, Stage::Lively),
            (Some(f32::NAN), Stage::Lively),
            (Some(-5.0), Stage::Lively),
            (Some(0.0), Stage::Relaxed),
            (Some(20.0), Stage::Relaxed),
            (Some(29.9), Stage::Relaxed),
            (Some(30.0), Stage::Lively),
            (Some(50.0), Stage::Lively),
            (Some(69.9), Stage::Lively),
            (Some(70.0), Stage::Struggling),
            (Some(95.0), Stage::Struggling),
            (Some(100.0), Stage::Struggling),
            (Some(f32::INFINITY), Stage::Struggling),
        ];
        for (pct, want) in cases {
            assert_eq!(stage_for(pct), want, "{pct:?}");
        }
        assert_eq!(Stage::default(), Stage::Lively);
        assert_eq!(params(Theme::Fire, Status::Running, Size::S, 0, 1).stage, Stage::Lively, "tests default to stage 2");
    }

    /// Like [`churn`], at a context stage and for any state: the mean share of
    /// body cells (columns 0..7) that change between adjacent resting ticks.
    fn stage_churn(theme: Theme, status: Status, stage: Stage, seed: u32) -> f32 {
        let body = |t| {
            let g = render(&staged(theme, status, stage, t, seed));
            (0..g.rows).flat_map(|r| (0..7).map(move |c| (c, r))).map(|(c, r)| cell(&g, c, r).ch).collect::<Vec<char>>()
        };
        let resting = |t| status != Status::Running || state::cast_phase_at(stage, seed, t).is_none();
        let pairs: Vec<(Vec<char>, Vec<char>)> = (100..340).filter(|&t| resting(t) && resting(t + 1)).map(|t| (body(t), body(t + 1))).collect();
        let changed: usize = pairs.iter().map(|(a, b)| a.iter().zip(b).filter(|(a, b)| a != b).count()).sum();
        changed as f32 / (pairs.len() * pairs[0].0.len()) as f32
    }

    #[test]
    fn stages_calm_down_and_strain_the_body() {
        for theme in Theme::ALL {
            for status in [Status::Running, Status::NeedsInput] {
                let mean = |stage| [4, 11, 706_312_658].iter().map(|&s| stage_churn(theme, status, stage, s)).sum::<f32>() / 3.0;
                let (s1, s2, s3) = (mean(Stage::Relaxed), mean(Stage::Lively), mean(Stage::Struggling));
                println!("{theme:?} {status:?}: churn relaxed {s1:.3} lively {s2:.3} struggling {s3:.3}");
                assert!(s1 < s2 && s2 < s3, "{theme:?} {status:?}: {s1} < {s2} < {s3}");
                if status == Status::Running {
                    assert!(s1 <= 0.6 * s2, "{theme:?}: relaxed {s1} vs lively {s2}");
                    assert!(s3 >= 1.3 * s2, "{theme:?}: struggling {s3} vs lively {s2}");
                }
            }
        }
    }

    #[test]
    fn stages_order_the_bob() {
        let spread = |v: &[f32]| v.iter().cloned().fold(f32::MIN, f32::max) - v.iter().cloned().fold(f32::MAX, f32::min);
        for seed in [4, 706_312_658] {
            for status in [Status::Running, Status::NeedsInput, Status::Idle] {
                let bob = |stage| (0..400).map(|t| state::bob_at(stage, status, seed, phase_tick(t))).collect::<Vec<f32>>();
                let (b1, b2, b3) = (bob(Stage::Relaxed), bob(Stage::Lively), bob(Stage::Struggling));
                assert!(spread(&b1) < spread(&b2) && spread(&b2) < spread(&b3), "{seed} {status:?}: {} {} {}", spread(&b1), spread(&b2), spread(&b3));
                assert_eq!(b2, (0..400).map(|t| state::bob(status, seed, phase_tick(t))).collect::<Vec<f32>>(), "stage 2 bob unchanged");
            }
            // a struggling idle spirit slumps (sits lower on average)
            let mean = |stage| (0..400).map(|t| state::bob_at(stage, Status::Idle, seed, phase_tick(t))).sum::<f32>() / 400.0;
            assert!(mean(Stage::Struggling) > mean(Stage::Lively) + 0.3, "{seed}: idle slump");
            // the panting faster than the lively bob: more direction changes
            let turns = |stage| {
                let b: Vec<f32> = (0..400).map(|t| state::bob_at(stage, Status::Running, seed, phase_tick(t))).collect();
                b.windows(3).filter(|w| (w[1] - w[0]) * (w[2] - w[1]) < 0.0).count()
            };
            assert!(turns(Stage::Relaxed) < turns(Stage::Lively) && turns(Stage::Lively) < turns(Stage::Struggling), "{seed}: bob tempo");
        }
        // the rendered body: its vertical centroid wanders least when relaxed, most when struggling
        for theme in Theme::ALL {
            let centroid = |stage, t| {
                let d: Vec<f32> = dots(&render(&staged(theme, Status::Running, stage, t, 4))).iter().filter(|p| p.0 < 12).map(|p| p.1 as f32).collect();
                d.iter().sum::<f32>() / d.len().max(1) as f32
            };
            let sd = |stage| {
                let v: Vec<f32> = (100..300).map(|t| centroid(stage, t)).collect();
                let m = v.iter().sum::<f32>() / v.len() as f32;
                (v.iter().map(|x| (x - m) * (x - m)).sum::<f32>() / v.len() as f32).sqrt()
            };
            let (r1, r2, r3) = (sd(Stage::Relaxed), sd(Stage::Lively), sd(Stage::Struggling));
            println!("{theme:?}: body centroid sd relaxed {r1:.3} lively {r2:.3} struggling {r3:.3}");
            assert!(r1 < r2 && r2 < r3, "{theme:?}: body bob relaxed {r1} < lively {r2} < struggling {r3}");
        }
    }

    #[test]
    fn struggling_spirits_sweat_drops_that_fall_and_fade() {
        for seed in [4, 11, 706_312_658] {
            for status in [Status::Running, Status::NeedsInput] {
                let mut lives = 0;
                let mut run: Vec<(i32, i32)> = Vec::new();
                for t in 0..400u64 {
                    let p = staged(Theme::Water, status, Stage::Struggling, t, seed);
                    let drops = state::sweat_drops(&Ctx::new(&p), &p.size.geom(), CY);
                    assert!(drops.len() <= 1, "{seed} {t}: one drop at a time");
                    match drops.first() {
                        Some(&(x, y, _)) => run.push((x, y)),
                        None if !run.is_empty() => {
                            // a whole drop: it slides down the side and is gone
                            assert!(run.windows(2).all(|w| w[1].1 >= w[0].1), "{seed} {status:?}: {run:?}");
                            assert!(run.last().unwrap().1 >= run[0].1 + 3, "{seed} {status:?}: falls {run:?}");
                            lives += 1;
                            run.clear();
                        }
                        None => {}
                    }
                    for stage in [Stage::Relaxed, Stage::Lively] {
                        let q = SpiritParams { stage, ..p };
                        assert!(state::sweat_drops(&Ctx::new(&q), &q.size.geom(), CY).is_empty(), "{stage:?}: no sweat");
                    }
                }
                assert!(lives >= 10, "{seed} {status:?}: {lives} drops");
            }
            // on the grid: a drop paints its dot left of the body in a pale blue-white
            let mut seen = 0;
            for t in 0..200u64 {
                for theme in Theme::ALL {
                    let p = staged(theme, Status::Running, Stage::Struggling, t, seed);
                    let g = render(&p);
                    let ctx = Ctx::new(&p);
                    let gm = p.size.geom();
                    let pose = state::pose_at(p.stage, ctx.elem, ctx.state, ctx.seed, ctx.t);
                    let cy = CY + bob_px(&ctx, &gm, &pose) / gm.ppu;
                    for (x, y, _) in state::sweat_drops(&ctx, &gm, cy) {
                        let c = cell(&g, (x / 2) as u16, (y / 4) as u16);
                        if dots(&g).contains(&(x, y)) && c.fg.is_some_and(|(r, gg, b)| b > r && b >= gg && b < 255) {
                            seen += 1;
                        }
                    }
                }
            }
            assert!(seen > 30, "{seed}: sweat drops on the grid ({seen})");
        }
    }

    #[test]
    fn struggling_casts_wind_up_longer_and_recoil_harder() {
        for seed in [4, 11, 706_312_658] {
            let c = (20..).find(|&t| state::casts_at(seed, t)).expect("cast");
            assert_eq!(state::cast_phase_at(Stage::Struggling, seed, c - 5), Some(state::CastPhase::WindUp(0)), "{seed}: five wind-up frames");
            assert_eq!(state::cast_phase_at(Stage::Lively, seed, c - 5), None);
            for theme in Theme::ALL {
                let recoil = |stage| state::pose_at(stage, theme, Status::Running, seed, c).dx;
                assert!(recoil(Stage::Struggling) < recoil(Stage::Lively), "{theme:?}: stronger recoil");
                for t in c - 3..c + 2 {
                    assert_eq!(state::cast_phase_at(Stage::Lively, seed, t), state::cast_phase(seed, t));
                    assert_eq!(state::pose_at(Stage::Lively, theme, Status::Running, seed, t), state::pose(theme, Status::Running, seed, t));
                }
            }
        }
        // relaxed: the pose comes on some casts only, and gentler
        for theme in Theme::ALL {
            let (mut posed, mut casts) = (0, 0);
            for seed in 1..40u32 {
                for c in (1..300).filter(|&t| state::casts_at(seed, t)) {
                    casts += 1;
                    let p = state::pose_at(Stage::Relaxed, theme, Status::Running, seed, c);
                    if p.facing {
                        posed += 1;
                        assert!(p.dx.abs() < state::pose(theme, Status::Running, seed, c).dx.abs(), "{theme:?}: gentler recoil");
                    }
                }
            }
            assert!(posed * 10 >= casts * 2 && posed * 10 <= casts * 7, "{theme:?}: posed on {posed} of {casts} casts");
        }
    }

    #[test]
    fn eyes_keep_their_structure_in_every_stage() {
        for seed in [1, 9, 706_312_658] {
            for stage in [Stage::Relaxed, Stage::Lively, Stage::Struggling] {
                for (theme, status) in Theme::ALL.into_iter().flat_map(|t| STATUSES.map(|s| (t, s))) {
                    for t in (0..160).filter(|&t| !eyes::blinking(seed, t)) {
                        let g = render(&staged(theme, status, stage, t, seed));
                        let white = white_cells(&g);
                        assert_eq!(white.len(), 2, "{theme:?} {status:?} {stage:?} {t}: {white:?}");
                        assert_eq!(white[0].1, white[1].1, "eyes on one row");
                        let base = white_cells(&render(&staged(theme, Status::Idle, Stage::Lively, t, seed)));
                        let cells = |w: &[(u16, u16, char)]| w.iter().map(|&(c, r, _)| (c, r)).collect::<Vec<_>>();
                        assert_eq!(cells(&white), cells(&base), "{theme:?} {status:?} {stage:?} {t}: same eye cells");
                        assert!(white.iter().all(|w| matches!(w.2, '\u{2802}' | '\u{2810}')), "{white:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn stages_keep_the_cues_and_never_panic() {
        let ticks = [0, 1, 2, 3, 37, 4_000_000_000, u64::MAX - 3, u64::MAX - 1, u64::MAX];
        for stage in [Stage::Relaxed, Stage::Lively, Stage::Struggling] {
            for (theme, status) in Theme::ALL.into_iter().flat_map(|t| STATUSES.map(|s| (t, s))) {
                for size in [Size::S] {
                    for (tick, seed) in ticks.iter().flat_map(|&t| [0, 1, u32::MAX].map(|s| (t, s))) {
                        let p = SpiritParams { stage, warning: tick % 2 == 0, ..params(theme, status, size, tick, seed) };
                        let g = render(&p);
                        assert_eq!(g, render(&p), "deterministic");
                        assert_eq!((g.cols, g.rows), grid_size(size), "{p:?}: inside the canvas");
                        assert_eq!(g.cells.len(), usize::from(g.cols) * usize::from(g.rows));
                    }
                }
            }
            for (theme, size) in Theme::ALL.into_iter().flat_map(|t| [Size::S].map(|s| (t, s))) {
                for t in 0..60 {
                    let q = |status| chars(&render(&SpiritParams { stage, ..params(theme, status, size, t, 2) }));
                    assert_eq!(q(Status::NeedsInput).iter().filter(|c| c.2 == '?').count(), 1, "{stage:?} {theme:?} {size:?} {t}: ?");
                    assert!(q(Status::Idle).iter().any(|c| c.2 == 'z' || c.2 == 'Z'), "{stage:?} {theme:?} {size:?} {t}: z");
                }
            }
        }
    }

    // ---- Size::XS: the small spirit of the standard card ----

    /// Columns the standard card gives its XS spirit (inner width 18 minus the pad).
    /// Columns the large card gives its S spirit (inner width 30 minus the pad).
    const STAGES: [Stage; 3] = [Stage::Relaxed, Stage::Lively, Stage::Struggling];
    /// First sub-pixel column right of the XS body.
    const XS_AREA: i32 = 12;

    fn xs(theme: Theme, status: Status, tick: u64, seed: u32) -> SpiritParams {
        params(theme, status, Size::XS, tick, seed)
    }

    /// Body dots of a frame: poison bubbles and ambient bits off,
    /// everything left of [`XS_AREA`].
    fn xs_body(p: &SpiritParams) -> std::collections::BTreeSet<(i32, i32)> {
        particles::NO_AMBIENT.with(|c| c.set(true));
        let g = render_with(p, false);
        particles::NO_AMBIENT.with(|c| c.set(false));
        let marks: Vec<(i32, i32)> = chars(&g).into_iter().filter(|c| !is_braille(c.2) && c.2 != ' ').map(|c| (i32::from(c.0), i32::from(c.1))).collect();
        dots(&g).into_iter().filter(|d| d.0 < XS_AREA && !marks.contains(&(d.0 / 2, d.1 / 4))).collect()
    }

    #[test]
    fn xs_is_a_small_round_spirit_centred_in_the_band() {
        assert_eq!(grid_size(Size::XS), (7, 4), "7 columns, as tall as the 4-row card band");
        assert!(![Size::S].contains(&Size::XS), "S/M/L stay the large card's sizes");
        assert_eq!(grid_size(Size::XS), (7, 4));
        for theme in Theme::ALL {
            for status in STATUSES {
                let g = render(&SpiritParams { ..xs(theme, status, 30, 1) });
                assert_eq!((g.cols, g.rows), grid_size(Size::XS), "{theme:?} {status:?} ");
                assert_eq!(g.cells.len(), usize::from(g.cols) * usize::from(g.rows));
            }
        }
        // a round body about 7 x 3 cells (14 x 12 sub-pixels), centred in the
        // 4-row band: no longer a 2-row strip
        for theme in Theme::ALL {
            for t in [0, 17, 33] {
                let b = xs_body(&xs(theme, Status::Idle, t, 3));
                let (x0, x1) = (b.iter().map(|d| d.0).min().unwrap(), b.iter().map(|d| d.0).max().unwrap());
                let (y0, y1) = (b.iter().map(|d| d.1).min().unwrap(), b.iter().map(|d| d.1).max().unwrap());
                let (w, h) = (x1 - x0 + 1, y1 - y0 + 1);
                // (fire tongues may add a dot row on top, the water's ripple line one below)
                assert!((7..=12).contains(&w) && (7..=14).contains(&h), "{theme:?} {t}: body {w}x{h} sub-pixels");
                let rows: std::collections::BTreeSet<i32> = b.iter().map(|d| d.1 / 4).collect();
                assert!(rows.len() >= 3, "{theme:?} {t}: body on {rows:?}");
                // (the fire's tongues rise a little above its round body)
                let mid = (y0 + y1 + 1) as f32 / 2.0;
                assert!((mid - 8.0).abs() <= 2.0, "{theme:?} {t}: body centre y {mid} (band centre 8)");
            }
        }
        // smaller than the S spirit
        let body = |size| -> u32 { render(&params(Theme::Water, Status::Idle, size, 0, 3)).cells.iter().filter(|c| is_braille(c.ch)).map(|c| (c.ch as u32 - 0x2800).count_ones()).sum() };
        assert!(body(Size::XS) * 10 < body(Size::S) * 8, "XS {} vs S {}", body(Size::XS), body(Size::S));
    }

    #[test]
    fn xs_every_state_theme_and_stage_renders() {
        for theme in Theme::ALL {
            for status in STATUSES {
                for stage in STAGES {
                    let g = render(&SpiritParams { stage, ..xs(theme, status, 40, 3) });
                    let n = g.cells.iter().filter(|c| is_braille(c.ch)).count();
                    assert!(n >= 8, "{theme:?} {status:?} {stage:?} {n} braille cells");
                    assert!(g.cells.iter().all(|c| c.fg.is_some() || c.ch == ' '), "drawn cells have a colour");
                }
            }
        }
    }

    #[test]
    fn xs_eyes_are_two_small_white_dots() {
        let g = Size::XS.geom();
        for facing in [false, true] {
            for blink in [false, true] {
                let plan = eyes::plan(&g, facing, blink);
                for e in &plan {
                    // a dot with at most a one-dot dark halo, never a cleared cell
                    assert!(e.clear.len() <= 2, "{facing} {blink}: {} cleared dots", e.clear.len());
                    match e.white {
                        Some(w) => {
                            assert!(!blink);
                            assert!(e.clear.iter().all(|&c| c != w && (c.0 - w.0).abs() <= 1 && (c.1 - w.1).abs() <= 1), "halo touches the dot: {:?} {:?}", w, e.clear);
                        }
                        None => assert!(blink && !e.dash.is_empty() && e.dash.len() <= 2, "blink: a short dash {:?}", e.dash),
                    }
                }
            }
        }
        // the S eyes clear a whole cell; the XS ones are clearly smaller
        let s = eyes::plan(&Size::S.geom(), false, false);
        let x = eyes::plan(&g, false, false);
        assert!(s.iter().all(|e| e.clear.len() >= 8) && x.iter().all(|e| e.clear.len() + 1 <= 3), "S clears {} dots, XS {}", s[0].clear.len(), x[0].clear.len());
        // proportional to the body: each eye (dot and halo) spans at most a
        // quarter of the body width
        for theme in Theme::ALL {
            let b = xs_body(&xs(theme, Status::Idle, first_tick(9, false), 9));
            let bw = b.iter().map(|d| d.0).max().unwrap() - b.iter().map(|d| d.0).min().unwrap() + 1;
            for facing in [false, true] {
                for e in eyes::plan(&g, facing, false) {
                    let xs: Vec<i32> = e.clear.iter().chain(e.white.iter()).map(|d| d.0).collect();
                    let w = xs.iter().max().unwrap() - xs.iter().min().unwrap() + 1;
                    assert!(w as f32 <= 0.25 * bw as f32, "{theme:?}: eye {w} of body {bw} sub-pixels");
                }
            }
        }
        // in the frame: two white dots in separate cells with body between,
        // left and right of them, and below them (UNKNOWN: no cue beside them)
        let t = first_tick(9, false);
        for theme in Theme::ALL {
            let gr = render(&xs(theme, Status::Unknown, t, 9));
            let white = white_cells(&gr);
            assert_eq!(white.len(), 2, "{theme:?}: {white:?}");
            let ((l, row), r) = ((white[0].0, white[0].1), white[1].0);
            assert_eq!(white[1].1, row, "{theme:?}: eyes on one row");
            assert!(r >= l + 2, "{theme:?}: eyes in separate cells, {l} and {r}");
            let body = |c: u16, rr: u16| is_braille(cell(&gr, c, rr).ch) && cell(&gr, c, rr).fg != WHITE;
            assert!((l + 1..r).all(|c| body(c, row)), "{theme:?}: body between the eyes");
            assert!(l >= 1 && body(l - 1, row) && body(r + 1, row), "{theme:?}: body left and right of the eyes");
            assert!(body(l, row + 1) && body(r, row + 1), "{theme:?}: body under the eyes");
        }
    }

    #[test]
    fn xs_eyes_keep_their_structure_in_every_state_theme_and_stage() {
        for seed in [1, 9, 706_312_658] {
            for stage in STAGES {
                for (theme, status) in Theme::ALL.into_iter().flat_map(|t| STATUSES.map(|s| (t, s))) {
                    for t in (0..160).filter(|&t| !eyes::blinking(seed, t)) {
                        let p = SpiritParams { stage, ..xs(theme, status, t, seed) };
                        let g = render(&p);
                        let white = white_cells(&g);
                        assert_eq!(white.len(), 2, "{p:?}: {white:?}");
                        assert_eq!(white[0].1, white[1].1, "eyes on one row");
                        // the dot looks one sub-pixel to the right during a burst
                        let facing = state::pose_at(stage, theme, status, seed, t).facing;
                        let dot = if facing { '\u{2810}' } else { '\u{2802}' };
                        assert!(white.iter().all(|w| w.2 == dot), "{p:?}: one white dot ({dot}) per eye: {white:?}");
                        let base = white_cells(&render(&xs(theme, Status::Idle, t, seed)));
                        let cells = |w: &[(u16, u16, char)]| w.iter().map(|&(c, r, _)| (c, r)).collect::<Vec<_>>();
                        assert_eq!(cells(&white), cells(&base), "{p:?}: same eye cells in every state");
                    }
                }
            }
        }
    }

    #[test]
    fn xs_blink_hides_the_dot_and_shows_a_dash() {
        let seed = 5;
        let starts: Vec<u64> = (1..1000).filter(|&t| eyes::blinking(seed, t) && !eyes::blinking(seed, t - 1)).collect();
        assert!(starts.len() >= 10);
        for &t in &starts {
            assert!(eyes::blinking(seed, t + 1) && !eyes::blinking(seed, t + 2), "two frames");
        }
        let t = starts[0];
        for theme in Theme::ALL {
            for status in STATUSES {
                let g = render(&xs(theme, status, t, seed));
                assert!(white_cells(&g).is_empty(), "{theme:?} {status:?}: no white while blinking");
                let open = white_cells(&render(&xs(theme, status, t - 1, seed)));
                assert_eq!(open.len(), 2);
                for (col, row, _) in open {
                    let c = cell(&g, col, row);
                    assert!(c.ch == '\u{2812}' && c.fg.is_some() && c.fg != WHITE, "{theme:?} {status:?}: ⠒ dash in the eye cell, got {c:?}");
                }
            }
        }
    }

    #[test]
    fn xs_keeps_the_state_cues() {
        let seed = 706_312_658;
        for stage in STAGES {
            for theme in Theme::ALL {
                for t in 0..60 {
                    let q = |status| chars(&render(&SpiritParams { stage, ..xs(theme, status, t, 2) }));
                    assert_eq!(q(Status::NeedsInput).iter().filter(|c| c.2 == '?').count(), 1, "{stage:?} {theme:?} {t}: ?");
                    assert!(q(Status::Idle).iter().any(|c| c.2 == 'z' || c.2 == 'Z'), "{stage:?} {theme:?} {t}: z");
                    assert!(!q(Status::Running).iter().any(|c| matches!(c.2, '?' | 'z' | 'Z')));
                }
            }
        }
        for theme in Theme::ALL {
            let sparkle = |t| render(&xs(theme, Status::Done, t, seed)).cells.iter().any(|c| matches!(c.fg, Some((250, 236, 150)) | Some((204, 188, 120))));
            assert!((0..60).any(sparkle), "{theme:?}: DONE sparkles");
            for t in [0, 20, 45] {
                let idle = luminance(&render(&xs(theme, Status::Idle, t, seed)), &['z', 'Z']);
                let run = luminance(&render(&xs(theme, Status::Running, t, seed)), &[]);
                assert!(idle < run * 0.9, "{theme:?} {t}: idle {idle} vs running {run}");
            }
            // UNKNOWN is grey: much less colour than the same spirit idle
            let sat = |status| {
                let g = render(&xs(theme, status, 10, seed));
                let s: Vec<f32> = g.cells.iter().filter(|c| is_braille(c.ch) && c.fg != WHITE).filter_map(|c| c.fg).map(|(r, gg, b)| f32::from(r.max(gg).max(b)) - f32::from(r.min(gg).min(b))).collect();
                s.iter().sum::<f32>() / s.len().max(1) as f32
            };
            assert!(sat(Status::Unknown) < 0.6 * sat(Status::Idle), "{theme:?}: unknown {} vs idle {}", sat(Status::Unknown), sat(Status::Idle));
        }
    }

    #[test]
    fn xs_element_looks_survive_the_small_size() {
        // poison bubbles form and pop on the body, and stay off the eye row
        let mut bubbles = 0;
        for t in 0..300 {
            let p = xs(Theme::Poison, Status::Done, t, 4);
            for (_, py, _) in bubble_spots(&p) {
                assert!(py >= 8, "{t}: bubble at {py} on the eye row");
                bubbles += 1;
            }
            let (g, bare) = (render_with(&p, true), render_with(&p, false));
            assert_eq!(white_cells(&g), white_cells(&bare), "{t}: bubbles leave the eyes alone");
        }
        assert!(bubbles > 20, "{bubbles} bubbles");
        // only eyes and poison bubbles are pure white, and nothing is a solid bright tile
        for (theme, status) in Theme::ALL.into_iter().flat_map(|t| STATUSES.map(|s| (t, s))) {
            for t in 0..120 {
                let p = SpiritParams { ..xs(theme, status, t, 41) };
                let (g, bare) = (render_with(&p, true), render_with(&p, false));
                for (i, c) in g.cells.iter().enumerate().filter(|(_, c)| c.fg == WHITE) {
                    if bits(c).count_ones() == 1 && bare.cells[i] == *c {
                        continue;
                    }
                    assert!(theme == Theme::Poison && bare.cells[i] != *c && bits(c).count_ones() >= 2 && i as u16 / g.cols >= 2, "{p:?} cell {i}: {c:?}");
                }
            }
        }
        // the burst pose leans right, then recoils
        for theme in Theme::ALL {
            let mean_x = |t: u64, seed: u32| {
                let d: Vec<f32> = xs_body(&xs(theme, Status::Running, t, seed)).iter().map(|d| d.0 as f32).collect();
                d.iter().sum::<f32>() / d.len().max(1) as f32
            };
            let (mut lean, mut n) = (0.0f32, 0);
            for seed in [4, 11, 23, 41] {
                for c in (10..400).filter(|&t| state::casts_at(seed, t)) {
                    lean += mean_x(c - 1, seed) - mean_x(c, seed);
                    n += 1;
                }
            }
            assert!(lean / n as f32 > 0.3, "{theme:?}: wind-up vs release {}", lean / n as f32);
        }
    }

    #[test]
    fn xs_stages_order_the_motion() {
        for theme in Theme::ALL {
            let centroid = |stage, t| {
                let d: Vec<f32> = dots(&render(&SpiritParams { stage, ..xs(theme, Status::Running, t, 4) })).iter().filter(|p| p.0 < XS_AREA).map(|p| p.1 as f32).collect();
                d.iter().sum::<f32>() / d.len().max(1) as f32
            };
            let sd = |stage| {
                let v: Vec<f32> = (100..400).map(|t| centroid(stage, t)).collect();
                let m = v.iter().sum::<f32>() / v.len() as f32;
                (v.iter().map(|x| (x - m) * (x - m)).sum::<f32>() / v.len() as f32).sqrt()
            };
            let (r1, r2, r3) = (sd(Stage::Relaxed), sd(Stage::Lively), sd(Stage::Struggling));
            println!("{theme:?} XS: centroid sd {r1:.3} {r2:.3} {r3:.3}");
            assert!(r1 < r2 && r2 < r3, "{theme:?}: relaxed {r1} < lively {r2} < struggling {r3}");
        }
        // stage 3 sweats a drop now and then, visible beside the body
        let g = Size::XS.geom();
        let drops: Vec<(i32, i32, f32)> = (0..400u64).flat_map(|t| state::sweat_drops(&Ctx::new(&SpiritParams { stage: Stage::Struggling, ..xs(Theme::Water, Status::Running, t, 4) }), &g, CY)).collect();
        assert!(drops.len() > 20, "{} sweat frames", drops.len());
        assert!(drops.iter().all(|d| d.0 >= 0 && d.1 >= 0 && (d.1 as usize) < g.h), "drops on the canvas: {drops:?}");
    }

    #[test]
    fn xs_is_deterministic_and_never_panics() {
        let ticks = [0, 1, 2, 3, 37, 4_000_000_000, u64::MAX - 3, u64::MAX - 1, u64::MAX];
        for stage in STAGES {
            for (theme, status) in Theme::ALL.into_iter().flat_map(|t| STATUSES.map(|s| (t, s))) {
                for (tick, seed) in ticks.iter().flat_map(|&t| [0, 1, u32::MAX].map(|s| (t, s))) {
                    let p = SpiritParams { stage, warning: tick % 2 == 0, ..xs(theme, status, tick, seed) };
                    let g = render(&p);
                    assert_eq!(g, render(&p), "deterministic");
                    assert_eq!(g.rows, 4);
                    assert_eq!(g.cells.len(), usize::from(g.cols) * usize::from(g.rows));
                }
            }
        }
    }

    /// (tick, seed) of the XS golden frames, rendered for every element and state.
    const XS_GOLDEN_CASES: [(u64, u32); 3] = [(37, 7), (121, 706_312_658), (260, 41)];

    /// Hashes of the XS golden frames (theme-major, then state, then
    /// `XS_GOLDEN_CASES`), captured when the cast magic was removed;
    /// `SPIRIT_GOLDEN_PRINT=1` prints them.
    const XS_GOLDEN: [u64; 60] = [
        0xa46f1e77469f44f2, 0x9fcf593439235840, 0xb2c8ba59cb0ee61f, 0x8c4d12bcaade4c91, 0x111464d0c539b6b2, 0x4eceaf8677977634,
        0x76dae5d953ecc50b, 0xb6a8fcb821c3ca84, 0xd0f87e8dcacd0112, 0xca2b781017ea0a19, 0x116dcb87d6736c03, 0x3c78f4e14dd70080,
        0x51e58b4b661fe6da, 0xfdd0e4ae1461f5b6, 0x10dbf51a4aa54620, 0x76e40c8b197211f7, 0x3c480da031345c25, 0xabe51d43df57f198,
        0xadf281763c8442a2, 0x0c1fa5e0666725f2, 0x80552ac87dd9d191, 0x7347d102f2e3a7d0, 0x87cd62e6d61e844d, 0x8b62eaff1ff1d85b,
        0x622283c0042efcd2, 0x994aca2108bfd2fa, 0xe06ec5df04641e20, 0x843e78e5e6303f67, 0xfc60f5220596cb8a, 0xf3e52a21fa050d29,
        0xdc1822ef1c0de976, 0xe92993f76185fb98, 0x123488104bc11ae9, 0x81c25ec7e3715ee4, 0x3cb4d4e6f9d896f9, 0xf4ff42277cda4cdc,
        0x50093fd1738b1d8b, 0x943c3d05fcaae6c5, 0x7660524feb653a52, 0xb36713694cf59a69, 0x1ddea2bbacc622d4, 0x3755773239293a38,
        0x664ca2df343f6c6b, 0x3ca00a1c2c1aaff7, 0x66f62f6585feb717, 0x7879c8c2986cfee3, 0x996c55a5ae84c7ca, 0xf0183264d78c0156,
        0x0cf2c17a7f336fd0, 0x5c2acd463d12be9e, 0xfd079c364d29b5c3, 0xeeb8cb4fbee6bdd4, 0xcaf2c2a8fa3db1bb, 0x4d90ebd192feca9d,
        0x9304bde111bcb14b, 0xcd32bc23b3476f94, 0xa27c1622d1417215, 0x1dd4a914bfb989c0, 0x611a63cf9df4b675, 0x19863f428cc452c5,
    ];

    #[test]
    fn xs_golden_frames() {
        let mut ps = Vec::new();
        for theme in Theme::ALL {
            for status in STATUSES {
                for (tick, seed) in XS_GOLDEN_CASES {
                    ps.push(SpiritParams { ..xs(theme, status, tick, seed) });
                }
            }
        }
        let hashes: Vec<u64> = ps.iter().map(|p| grid_hash(&render(p))).collect();
        if std::env::var("SPIRIT_GOLDEN_PRINT").is_ok() {
            for chunk in hashes.chunks(6) {
                println!("        {},", chunk.iter().map(|h| format!("0x{h:016x}")).collect::<Vec<_>>().join(", "));
            }
        }
        for (i, (p, (got, want))) in ps.iter().zip(hashes.iter().zip(XS_GOLDEN)).enumerate() {
            assert_eq!(*got, want, "XS golden frame {i}: {p:?}");
        }
    }

    /// Every element x state as ANSI text. Run with
    /// `cargo test dump_spirits -- --ignored --nocapture` (env `SPIRIT_TICK`, `SPIRIT_SIZE` = 0..2).
    #[test]
    #[ignore]
    fn dump_spirits() {
        let env = |k: &str, d: u64| std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(d);
        let (tick, size) = (env("SPIRIT_TICK", 40), [Size::S][env("SPIRIT_SIZE", 0).min(2) as usize]);
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
