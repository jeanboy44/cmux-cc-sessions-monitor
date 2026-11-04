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
//! - Context stage ([`Stage`], same thresholds as [`Magic`]): relaxed spirits
//!   barely move, lively ones are the reference look, struggling ones
//!   tremble, pant and sweat (see `state::bob_at`, `state::sweat`).

mod body;
mod canvas;
mod element;
mod eyes;
mod magic;
mod noise;
mod particles;
mod state;
mod subdots;
pub mod theme;

use crate::model::Status;
use canvas::Geom;
pub use subdots::{SubagentDot, MAX_DOTS};
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
/// and from normal to big; [`stage_for`] shares them.
const MAGIC_NORMAL_AT: f32 = 20.0;
const MAGIC_BIG_AT: f32 = 50.0;
/// Context usage above which the body flickers red.
const WARNING_ABOVE: f32 = 90.0;
/// Canvas width (sub-pixels) of a spirit casting big magic: 17 cells, the
/// standard card's inner width minus a one-column left pad.
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

/// Magic level for a context usage: < 20 % small, 20..50 % normal,
/// >= 50 % big. Unknown or invalid (NaN, negative) usage casts normal magic.
pub fn magic_for(ctx_percent: Option<f32>) -> Magic {
    match ctx_percent {
        Some(p) if p.is_nan() || p < 0.0 => Magic::Normal,
        Some(p) if p >= MAGIC_BIG_AT => Magic::Big,
        Some(p) if p >= MAGIC_NORMAL_AT => Magic::Normal,
        Some(_) => Magic::Small,
        None => Magic::Normal,
    }
}

/// How hard the spirit works, driven by context usage with the same
/// thresholds as [`Magic`] (which still scales the cast particles).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Stage {
    /// stage 1 (< 20 %): slow, tiny bob, calm churn, few ambient bits
    Relaxed,
    /// stage 2: the reference motion
    #[default]
    Lively,
    /// stage 3 (>= 50 %): trembling, panting, sweat drops, effortful casts
    Struggling,
}

/// Context stage for a context usage (see [`magic_for`]: < 20 % relaxed,
/// 20..50 % lively, >= 50 % struggling; unknown or invalid usage is lively).
pub fn stage_for(ctx_percent: Option<f32>) -> Stage {
    match magic_for(ctx_percent) {
        Magic::Small => Stage::Relaxed,
        Magic::Normal => Stage::Lively,
        Magic::Big => Stage::Struggling,
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
    /// element looks, pose and stages as S; its magic is drawn bolder and
    /// simpler so it reads at this size (see `magic`).
    XS,
    S,
    M,
    L,
}

impl Size {
    /// The bigger variants (the large card draws S); XS is the standard
    /// card's own small size.
    pub const ALL: [Size; 3] = [Size::S, Size::M, Size::L];

    /// Every size is 4 rows tall (16 sub-pixels), the card band; bigger
    /// steps get a bigger body and more room to the right for the cast
    /// magic. The body centre sits at 1/3 of the width (as in the reference
    /// design) and high enough that the eye row (cell row 1) has body above
    /// and below it.
    const fn geom(self) -> Geom {
        match self {
            // 11 x 4 cells, body radius 5.5 sub-pixels
            Size::S => Geom { ppu: 5.5 / BASE_R, w: 22, h: 16, rcx: 7.0, rcy: 8.0, small: true, dot_eyes: false },
            // 14 x 4 cells, body radius 6 sub-pixels
            Size::M => Geom { ppu: 6.0 / BASE_R, w: 28, h: 16, rcx: 9.0, rcy: 8.0, small: true, dot_eyes: false },
            // 17 x 4 cells, body radius 6.5 sub-pixels
            Size::L => Geom { ppu: 6.5 / BASE_R, w: 34, h: 16, rcx: 11.0, rcy: 8.0, small: true, dot_eyes: false },
            // 7 x 4 cells, body radius 4.75 sub-pixels: the round body
            // fills cells 1..5 and is centred on the band's middle row line;
            // the magic starts at column 6
            Size::XS => Geom { ppu: 4.75 / BASE_R, w: 14, h: 16, rcx: 7.0, rcy: 8.0, small: true, dot_eyes: true },
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

/// Canvas of a frame: [`geom_for`], widened to [`SpiritParams::cols`] while
/// the magic is being cast (centred magic fills the area; the fire breath
/// gets room for its level's length).
fn geom_of(p: &SpiritParams) -> Geom {
    let mut g = geom_for(p.size, p.magic);
    let room = usize::from(p.cols) * 2;
    if p.status == Status::Running && room > g.w {
        g.w = room;
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
    /// how hard the spirit works (context usage)
    pub stage: Stage,
    /// columns the card gives the spirit (its inner width right of the
    /// pad); a running water, poison or lightning spirit widens its grid to
    /// them so the magic is centred in the whole area. 0: the size's own grid.
    pub cols: u16,
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
    magic: Magic,
    /// the small [`Size::XS`] spirit: its magic is drawn bolder and simpler
    xs: bool,
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
            magic: p.magic,
            xs: matches!(p.size, Size::XS),
            // the reference design width (unchanged look), or the wider big-magic canvas
            right: if matches!(p.magic, Magic::Big) { DW.max(CX + (g.w as f32 - g.rcx) / g.ppu) } else { DW },
        }
    }
}

/// Renders one frame of a spirit (no subagent dots).
#[cfg_attr(not(test), allow(dead_code))]
pub fn render(p: &SpiritParams) -> SpiritGrid {
    render_with(p, true)
}

/// Renders one frame with a small dot per running subagent at the body's
/// lower right (see `subdots`); no dots renders exactly like [`render`].
pub fn render_dots(p: &SpiritParams, dots: &[SubagentDot]) -> SpiritGrid {
    render_frame(p, true, dots)
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
#[cfg_attr(not(test), allow(dead_code))]
fn render_with(p: &SpiritParams, bubbles: bool) -> SpiritGrid {
    render_frame(p, bubbles, &[])
}

fn render_frame(p: &SpiritParams, bubbles: bool, dots: &[SubagentDot]) -> SpiritGrid {
    // a finished spirit shows no subagents; at most MAX_DOTS dots
    let dots = if p.status == Status::Done { &[][..] } else { &dots[..dots.len().min(MAX_DOTS)] };
    let mut g = geom_of(p);
    let base_cols = (g.w / 2) as u16;
    if !dots.is_empty() {
        g.w = g.w.max(subdots::width_for(&g, dots.len()));
    }
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
    magic::draw(&mut cv, &ctx, CY + (bob_px - pose.dy) / g.ppu);
    state::sparkles(&mut cv, &ctx);
    if bubbles {
        element::draw_bubbles(&mut cv, &ctx, CX + pose.dx / g.ppu, look.cy);
    }
    subdots::draw(&mut cv, &ctx, dots);
    eyes::draw(&mut cv, pose.facing, eyes::blinking(ctx.seed, ctx.t), element::ramp(ctx.elem)[1], look.bright);
    let mut grid = cv.cells();
    state::marks(&mut grid, &cv, &ctx, pulse);
    subdots::keep_only_dots_past(&mut grid, &cv.g, base_cols, dots.len());
    grid
}

/// Sub-pixel centre (x, y) and stage of every poison bubble drawn on a frame.
#[cfg(test)]
fn bubble_spots(p: &SpiritParams) -> Vec<(i32, i32, element::Stage)> {
    let g = geom_of(p);
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
    use std::f32::consts::TAU;
    use theme::Theme;

    const STATUSES: [Status; 5] = [Status::Running, Status::NeedsInput, Status::Idle, Status::Done, Status::Unknown];
    const WHITE: Option<(u8, u8, u8)> = Some((255, 255, 255));

    fn params(theme: Theme, status: Status, size: Size, tick: u64, seed: u32) -> SpiritParams {
        SpiritParams { theme, status, size, tick, seed, warning: false, magic: Magic::Normal, stage: Stage::Lively, cols: 0 }
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
        // card variant: every size fits the 4-row spirit band of a 20x10 card
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
        let cases: [(Option<f32>, Magic); 15] = [
            (None, Magic::Normal),
            (Some(f32::NAN), Magic::Normal),
            (Some(-5.0), Magic::Normal),
            (Some(f32::NEG_INFINITY), Magic::Normal),
            (Some(0.0), Magic::Small),
            (Some(19.9), Magic::Small),
            (Some(20.0), Magic::Normal),
            (Some(30.0), Magic::Normal),
            (Some(49.9), Magic::Normal),
            (Some(50.0), Magic::Big),
            (Some(90.0), Magic::Big),
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

    /// Columns a 20x10 card gives the spirit (inner width 18 minus the pad).
    const CARD_COLS: u16 = 17;

    /// Sub-pixel dots the cast magic adds to a frame (bubbles left out):
    /// the frame diffed against the same frame with the magic switched off.
    fn magic_dots(p: &SpiritParams) -> std::collections::BTreeSet<(i32, i32)> {
        let with = dots(&render_with(p, false));
        magic::NO_MAGIC.with(|c| c.set(true));
        let without = dots(&render_with(p, false));
        magic::NO_MAGIC.with(|c| c.set(false));
        with.difference(&without).copied().collect()
    }

    /// Width in sub-pixels of a set of dots (0 when empty).
    fn extent(d: &std::collections::BTreeSet<(i32, i32)>) -> i32 {
        match (d.iter().map(|p| p.0).min(), d.iter().map(|p| p.0).max()) {
            (Some(lo), Some(hi)) => hi - lo + 1,
            _ => 0,
        }
    }

    /// Magic dots summed over a cast cycle in a card, and the width of the
    /// area they painted (sub-pixels). Centred magic makes the farthest
    /// column meaningless as a size measure; the painted width replaces it.
    fn magic_stats(theme: Theme, magic: Magic) -> (usize, i32) {
        let (mut n, mut all) = (0usize, std::collections::BTreeSet::new());
        for t in 0..120 {
            let d = magic_dots(&SpiritParams { magic, cols: CARD_COLS, ..params(theme, Status::Running, Size::S, t, 4) });
            n += d.len();
            all.extend(d);
        }
        (n, extent(&all))
    }

    #[test]
    fn magic_scales_the_cast_particles() {
        for theme in Theme::ALL {
            let (s_dots, s_ext) = magic_stats(theme, Magic::Small);
            let (n_dots, n_ext) = magic_stats(theme, Magic::Normal);
            let (b_dots, b_ext) = magic_stats(theme, Magic::Big);
            assert!(s_dots < n_dots && n_dots < b_dots, "{theme:?}: dots {s_dots} < {n_dots} < {b_dots}");
            assert!(s_ext < n_ext && n_ext < b_ext, "{theme:?}: width {s_ext} < {n_ext} < {b_ext}");
        }
    }

    /// Size of one magic level in the standard card, cell units.
    #[derive(Debug, Clone, Copy)]
    struct LevelMetrics {
        /// mean painted cells per frame over every frame
        cells: f32,
        /// mean painted cells per painted frame
        lit_cells: f32,
        /// mean width (first to last painted column) of painted frames
        width: f32,
        /// mean painted rows of painted frames, and the most
        height: f32,
        max_height: usize,
        /// mean farthest painted column of painted frames, and the farthest
        far: f32,
        max_far: u16,
        /// columns between the first and last column ever painted
        union: i32,
        /// mean painted dots per frame over every frame
        dots: f32,
    }

    /// [`LevelMetrics`] over seeds 4/11/23 x 240 running ticks at stage 2
    /// in the 17-column card, ambient particles off.
    fn level_metrics(theme: Theme, magic: Magic) -> LevelMetrics {
        level_metrics_for(Size::S, CARD_COLS, theme, magic)
    }

    /// [`level_metrics`] of a spirit of `size` given `cols` card columns.
    fn level_metrics_for(size: Size, cols: u16, theme: Theme, magic: Magic) -> LevelMetrics {
        particles::NO_AMBIENT.with(|c| c.set(true));
        let (mut cells, mut frames, mut lit, mut dots) = (0usize, 0usize, 0usize, 0usize);
        let (mut width, mut height, mut far) = (0usize, 0usize, 0usize);
        let (mut max_height, mut max_far) = (0usize, 0u16);
        let mut painted = std::collections::BTreeSet::new();
        for seed in [4, 11, 23] {
            for t in 0..240 {
                let d = magic_dots(&SpiritParams { magic, cols, ..params(theme, Status::Running, size, t, seed) });
                let c: std::collections::BTreeSet<(u16, u16)> = d.iter().map(|&(x, y)| ((x / 2) as u16, (y / 4) as u16)).collect();
                frames += 1;
                dots += d.len();
                if c.is_empty() {
                    continue;
                }
                lit += 1;
                cells += c.len();
                let (lo, hi) = (c.iter().map(|p| p.0).min().unwrap_or(0), c.iter().map(|p| p.0).max().unwrap_or(0));
                let rows: std::collections::BTreeSet<u16> = c.iter().map(|p| p.1).collect();
                width += usize::from(hi - lo + 1);
                height += rows.len();
                far += usize::from(hi);
                max_height = max_height.max(rows.len());
                max_far = max_far.max(hi);
                painted.extend(c.iter().map(|p| p.0));
            }
        }
        particles::NO_AMBIENT.with(|c| c.set(false));
        let per = |v: usize| v as f32 / lit.max(1) as f32;
        LevelMetrics {
            cells: cells as f32 / frames as f32,
            lit_cells: per(cells),
            width: per(width),
            height: per(height),
            max_height,
            far: per(far),
            max_far,
            union: painted.first().zip(painted.last()).map_or(0, |(lo, hi)| i32::from(*hi) - i32::from(*lo) + 1),
            dots: dots as f32 / frames as f32,
        }
    }

    /// Mean painted cells per frame of the Normal magic before the level
    /// rebalance (fire, water, poison, lightning; measured with
    /// [`level_metrics`]): Normal now paints about half of that.
    const OLD_NORMAL_CELLS: [f32; 4] = [6.7, 9.1, 7.2, 2.4];

    #[test]
    fn magic_levels_are_clearly_visible() {
        let all: Vec<_> = Theme::ALL.iter().map(|&t| (t, Magic::ALL.map(|m| level_metrics(t, m)))).collect();
        for (theme, [s, n, b]) in &all {
            println!("{theme:?}: cells S {:.1} N {:.1} B {:.1}; width S {:.1} N {:.1} B {:.1}; height S {:.2} N {:.2} B {:.2}", s.cells, n.cells, b.cells, s.width, n.width, b.width, s.height, n.height, b.height);
        }
        for (i, (theme, [s, n, b])) in all.iter().enumerate() {
            // three clear steps: each level paints well under the next one
            assert!(s.cells < 0.55 * n.cells, "{theme:?}: small cells {} vs normal {}", s.cells, n.cells);
            assert!(n.cells < 0.65 * b.cells, "{theme:?}: normal cells {} vs big {}", n.cells, b.cells);
            assert!(s.width + 1.5 <= n.width && n.width + 1.5 <= b.width, "{theme:?}: width {} < {} < {}", s.width, n.width, b.width);
            // lightning bolts always strike top to bottom
            assert!(s.height <= n.height && n.height <= b.height + 0.05, "{theme:?}: height {} {} {}", s.height, n.height, b.height);
            if *theme != Theme::Lightning {
                assert!(s.height < n.height && n.height < b.height, "{theme:?}: height {} < {} < {}", s.height, n.height, b.height);
            }
            // small is a clear little shape, never a stray dot
            assert!(s.lit_cells >= 2.0, "{theme:?}: small paints {} cells", s.lit_cells);
            // normal is no longer the old crowd of cells. Fire is exempt: its
            // Normal was boosted back to a full flame by request (still well
            // under its Big), as is Lightning, whose Normal is four thin bolts
            // by request (the old Normal was one or two)
            let cap = match theme {
                Theme::Fire => 2.1,
                Theme::Lightning => f32::INFINITY,
                Theme::Water | Theme::Poison => 0.6,
            };
            assert!(n.cells <= cap * OLD_NORMAL_CELLS[i], "{theme:?}: normal cells {} vs old {}", n.cells, OLD_NORMAL_CELLS[i]);
            // big fits the card area: nothing past the last column
            assert!(b.max_far < CARD_COLS, "{theme:?}: big reaches col {}", b.max_far);
        }
    }

    #[test]
    fn fire_breathes_three_lengths() {
        // the first flame column (one dot right of the body) is column 7, so
        // the flame is `far - 6` columns long
        let [s, n, b] = Magic::ALL.map(|m| level_metrics(Theme::Fire, m));
        let len = |m: &LevelMetrics| m.far - 6.0;
        let (ls, ln, lb) = (len(&s), len(&n), len(&b));
        println!("fire length S {ls:.2} N {ln:.2} B {lb:.2}; rows S {} N {} B {}; dots S {:.1} N {:.1} B {:.1}", s.max_height, n.max_height, b.max_height, s.dots, n.dots, b.dots);
        assert!((2.5..=4.0).contains(&ls), "small flame {ls} columns");
        assert!((5.5..=7.5).contains(&ln), "normal flame {ln} columns");
        assert!(lb >= 8.0, "big flame {lb} columns");
        assert!(ls < ln - 1.5 && ln - 1.5 < lb - 2.0, "length steps {ls} {ln} {lb}");
        assert!(s.max_height <= 2 && n.max_height == 3 && b.max_height <= 4, "rows {} {} {}", s.max_height, n.max_height, b.max_height);
        assert!(s.height < n.height && n.height < b.height, "rows {} < {} < {}", s.height, n.height, b.height);
        // normal is a full flame (about twice the thin 6-cell jet it was),
        // clearly stronger than the water and poison Normal, well under Big
        assert!((10.0..=14.0).contains(&n.cells), "normal paints {} cells a frame", n.cells);
        for other in [Theme::Water, Theme::Poison] {
            let o = level_metrics(other, Magic::Normal);
            assert!(n.cells >= 1.5 * o.cells, "fire normal {} cells vs {other:?} {}", n.cells, o.cells);
        }
        assert!(s.dots * 1.6 < n.dots && n.dots * 2.5 < b.dots, "dots {} {} {}", s.dots, n.dots, b.dots);
        // the small spirit's normal flame gets the same boost
        let xn = level_metrics_for(Size::XS, STD_COLS, Theme::Fire, Magic::Normal);
        assert!(xn.cells >= 8.0, "XS normal paints {} cells a frame", xn.cells);
    }

    /// Centre (sub-pixels) of the magic area of a size-S spirit with a
    /// `cols`-wide grid: from the first cell right of the body (column 7) to
    /// the right edge.
    fn area_centre(cols: u16) -> f32 {
        let start = (body_center_x(Size::S) + 3.0).ceil() * 2.0;
        (start + f32::from(cols) * 2.0) / 2.0
    }

    #[test]
    fn centred_magic_sits_in_the_middle_of_its_area() {
        particles::NO_AMBIENT.with(|c| c.set(true));
        for theme in [Theme::Water, Theme::Poison, Theme::Lightning] {
            for cols in [0u16, CARD_COLS, LARGE_COLS, 40] {
                for magic in Magic::ALL {
                    let (mut worst, mut frames) = (0.0f32, 0);
                    for seed in [4, 11, 23, 706_312_658] {
                        let p = |t| SpiritParams { magic, cols, ..params(theme, Status::Running, Size::S, t, seed) };
                        let all: Vec<std::collections::BTreeSet<(i32, i32)>> = (0..320u64).map(|t| magic_dots(&p(t))).collect();
                        let centre = area_centre(render(&p(0)).cols.max(render(&p(1)).cols));
                        // staggered lightning bolts flash one by one: the group is
                        // centred over the whole cast (every frame until the next cast)
                        if theme == Theme::Lightning && magic != Magic::Small {
                            let casts: Vec<usize> = (1..320).filter(|&t| state::casts_at(seed, t as u64)).collect();
                            for w in casts.windows(2) {
                                let d: Vec<(i32, i32)> = all[w[0]..w[1]].iter().flatten().copied().collect();
                                let cx = d.iter().map(|p| p.0 as f32 + 0.5).sum::<f32>() / d.len().max(1) as f32;
                                worst = worst.max((cx - centre).abs());
                                frames += 1;
                            }
                            continue;
                        }
                        for t in 2..300usize {
                            let d = &all[t];
                            // not the first or last frame of a cast, nor its faint fade-in/out
                            // (under a third of the densest frame around it)
                            let peak = all[t.saturating_sub(12)..t + 12].iter().map(|d| d.len()).max().unwrap_or(0);
                            if d.len() < 4 || d.len() * 3 < peak || state::casts_at(seed, t as u64) || all[t + 1].is_empty() {
                                continue;
                            }
                            let cx = d.iter().map(|p| p.0 as f32 + 0.5).sum::<f32>() / d.len() as f32;
                            worst = worst.max((cx - centre).abs());
                            frames += 1;
                        }
                    }
                    println!("{theme:?} cols {cols} {magic:?}: {frames} frames, worst centroid offset {worst:.2} sub-pixels");
                    assert!(frames >= 20 || (theme == Theme::Lightning && frames >= 30), "{theme:?} {cols} {magic:?}: {frames} frames");
                    assert!(worst <= 2.0, "{theme:?} {cols} {magic:?}: centroid {worst} sub-pixels off the area centre");
                }
            }
        }
        particles::NO_AMBIENT.with(|c| c.set(false));
    }

    #[test]
    fn centred_magic_grows_with_the_level_and_fire_keeps_its_length() {
        particles::NO_AMBIENT.with(|c| c.set(true));
        for theme in [Theme::Water, Theme::Poison, Theme::Lightning] {
            for cols in [0u16, CARD_COLS, LARGE_COLS, 40] {
                // mean painted width per frame
                let width = |magic| {
                    let w: Vec<i32> = [4, 11, 23]
                        .iter()
                        .flat_map(|&seed| (1..300).map(move |t| (seed, t)))
                        .map(|(seed, t)| extent(&magic_dots(&SpiritParams { magic, cols, ..params(theme, Status::Running, Size::S, t, seed) })))
                        .filter(|&w| w > 0)
                        .collect();
                    w.iter().sum::<i32>() as f32 / w.len().max(1) as f32
                };
                let [s, n, b] = Magic::ALL.map(width);
                println!("{theme:?} cols {cols}: mean width S {s:.1} N {n:.1} B {b:.1}");
                assert!(s < n && n < b, "{theme:?} {cols}: width {s} < {n} < {b}");
            }
        }
        particles::NO_AMBIENT.with(|c| c.set(false));
        // the Small and Normal breath's length is set by its level, not by
        // the card room: a wider room adds only empty columns (Big roars to
        // the room's right edge, see `big_magic_reaches_into_the_large_card_area`)
        for magic in [Magic::Small, Magic::Normal] {
            for t in 0..200 {
                for status in STATUSES {
                    let p = SpiritParams { magic, cols: CARD_COLS, ..params(Theme::Fire, status, Size::S, t, 4) };
                    let (card, wide) = (render(&p), render(&SpiritParams { cols: 60, ..p }));
                    for row in 0..wide.rows {
                        for col in 0..wide.cols {
                            let want = if col < card.cols { *cell(&card, col, row) } else { Cell { ch: ' ', fg: None } };
                            assert_eq!(*cell(&wide, col, row), want, "{magic:?} {status:?} {t} ({col},{row})");
                        }
                    }
                }
            }
        }
        // only a running magic widens the grid; it never paints past it
        for theme in Theme::ALL {
            for status in STATUSES {
                for cols in [0u16, 5, CARD_COLS, 40] {
                    let p = SpiritParams { cols, ..params(theme, status, Size::S, 50, 4) };
                    let g = render(&p);
                    let wide = status == Status::Running && cols > grid_size(Size::S).0;
                    assert_eq!(g.cols, if wide { cols } else { grid_size(Size::S).0 }, "{theme:?} {status:?} {cols}");
                    assert_eq!(g.cells.len(), usize::from(g.cols) * usize::from(g.rows));
                }
            }
        }
    }

    #[test]
    fn big_magic_reaches_into_the_large_card_area() {
        // the large card gives the S spirit 29 columns: a magic area of 22
        // columns right of the body (column 7 on)
        let g = geom_of(&SpiritParams { cols: LARGE_COLS, ..params(Theme::Water, Status::Running, Size::S, 1, 4) });
        let area = (g.w as f32 - (body_center_x(Size::S) + 3.0).ceil() * 2.0) / 2.0;
        assert!(area >= 18.0, "magic area {area} columns");
        for theme in Theme::ALL {
            let [s, n, b] = Magic::ALL.map(|m| level_metrics_for(Size::S, LARGE_COLS, theme, m));
            println!("{theme:?} large: far {} {} {}, width {:.1} {:.1} {:.1}", s.max_far, n.max_far, b.max_far, s.width, n.width, b.width);
            // Big reaches past the standard card's 17 columns, inside the card
            assert!(b.max_far >= 19 && b.max_far < LARGE_COLS, "{theme:?}: big reaches col {}", b.max_far);
            assert!(s.cells < n.cells && n.cells < b.cells && s.width < n.width && n.width < b.width, "{theme:?}: levels ordered");
        }
        // the Big fire roars to the right edge of the room
        let fire = level_metrics_for(Size::S, LARGE_COLS, Theme::Fire, Magic::Big);
        assert!(fire.max_far >= LARGE_COLS - 2, "fire reaches col {}", fire.max_far);
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
        assert!(!warning(Some(50.0)), "stage 3 alone does not warn");
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
                for size in Size::ALL {
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
            for (theme, size) in Theme::ALL.into_iter().flat_map(|t| Size::ALL.map(|s| (t, s))) {
                for status in STATUSES {
                    for t in 0..120 {
                        let p = params(theme, status, size, t, seed);
                        let (g, bare) = (render_with(&p, true), render_with(&p, false));
                        let cv = canvas::Canvas::new(geom_for(size, p.magic));
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
            let cv = canvas::Canvas::new(geom_for(p.size, p.magic));
            for (col0, row0, w, h) in eyes::boxes(&cv) {
                for (c, r) in (row0..row0 + h).flat_map(|r| (col0..col0 + w).map(move |c| (c as u16, r as u16))) {
                    assert_eq!(cell(with, c, r), cell(without, c, r), "{p:?} eye cell ({c}, {r})");
                }
            }
            assert_eq!(white_cells(with), white_cells(without), "{p:?}");
        });
        // the bubbles that are drawn never sit on the eye rows
        for seed in [3, 41, 706_312_658] {
            for size in Size::ALL {
                for t in 0..400 {
                    let p = params(Theme::Poison, Status::Done, size, t, seed);
                    let cv = canvas::Canvas::new(geom_for(size, p.magic));
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
        let p = |t| params(Theme::Poison, Status::Done, Size::M, t, seed);
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

    /// Whether some row is painted in each of the `n` columns from `from`.
    fn jet_run(g: &SpiritGrid, from: u16, n: u16) -> bool {
        (0..g.rows).any(|r| (from..from + n).all(|c| c < g.cols && is_braille(cell(g, c, r).ch)))
    }

    #[test]
    fn fire_breath_is_a_continuous_jet() {
        for seed in [4, 11, 23, 706_312_658] {
            let start = first_cast(seed) + 3;
            let ticks: Vec<u64> = (start..start + 240).collect();
            let at = |t, magic| render(&SpiritParams { magic, cols: CARD_COLS, ..params(Theme::Fire, Status::Running, Size::S, t, seed) });
            // the small and normal flames are thin: every column from the
            // gap on is lit, in one row or the next
            let lit = |g: &SpiritGrid, n: u16| (7..7 + n).all(|c| (0..g.rows).any(|r| is_braille(cell(g, c, r).ch)));
            let small = ticks.iter().filter(|&&t| lit(&at(t, Magic::Small), 3)).count();
            assert!(small * 10 >= ticks.len() * 9, "{seed}: small jet on {small} of {} ticks", ticks.len());
            let normal = ticks.iter().filter(|&&t| lit(&at(t, Magic::Normal), 4)).count();
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
            for (magic, rows) in [(Magic::Small, 2), (Magic::Normal, 2), (Magic::Big, 4)] {
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

    /// Mean column where the lightning bolts strike (where they hit the
    /// bottom row, clipped to the magic area) over the casts of several
    /// seeds: the mean over every bolt of a cast, each checked on the frame
    /// it flashes (Normal and Big bolts are staggered by a tick or two).
    fn bolt_strike_mean(magic: Magic) -> f32 {
        particles::NO_AMBIENT.with(|c| c.set(true));
        let (mut sum, mut n) = (0.0f32, 0u32);
        for seed in [4, 11, 23, 37, 58, 91] {
            for t in (1..400).filter(|&t| state::casts_at(seed, t)) {
                let p = SpiritParams { magic, ..params(Theme::Lightning, Status::Running, Size::S, t, seed) };
                let plan = magic::bolt_plan(&geom_for(Size::S, magic), &Ctx::new(&p));
                for b in &plan {
                    let col = (b.strike.round() / 2.0).floor();
                    let g = render(&SpiritParams { tick: t + b.delay, ..p });
                    let row = g.rows - 1;
                    let near = (col as u16).saturating_sub(1)..=(col as u16 + 1).min(g.cols - 1);
                    assert!(near.into_iter().any(|c| is_braille(cell(&g, c, row).ch)), "{seed} {t} {magic:?}: strike painted at col {col}");
                    sum += (col + 0.5) / plan.len() as f32;
                }
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
        for magic in Magic::ALL {
            let run_len = match magic {
                Magic::Small => 4,
                Magic::Normal => 6,
                Magic::Big => 14,
            };
            let (mut ok_gap, mut ok_run, mut n) = (0, 0, 0);
            let mut heights = [0f32; 5];
            for seed in [4, 11, 23, 706_312_658] {
                let start = first_cast(seed) + 3;
                for t in start..start + 120 {
                    let g = render(&SpiritParams { magic, cols: CARD_COLS, ..params(Theme::Fire, Status::Running, Size::S, t, seed) });
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
            // it widens with distance: the big cone a lot, the thin normal
            // flame a little; the small jet stays one or two dots tall
            let widen = match magic {
                Magic::Small => 0.0,
                Magic::Normal => 0.4,
                Magic::Big => 1.5,
            };
            // (the small jet's fifth dot column is already its tapering tip)
            let body = if magic == Magic::Small { &hs[..4] } else { &hs[..] };
            assert!(body.windows(2).all(|w| w[1] >= w[0] - 0.05) && hs[4] >= hs[0] + widen, "{magic:?}: the flame widens: {hs:?}");
            if magic == Magic::Small {
                assert!(hs.iter().all(|&h| h <= 2.6), "small: a thin jet {hs:?}");
            }
        }
        particles::NO_AMBIENT.with(|c| c.set(false));
    }

    fn lum((r, g, b): (u8, u8, u8)) -> f32 {
        0.2126 * f32::from(r) + 0.7152 * f32::from(g) + 0.0722 * f32::from(b)
    }

    /// Rotation (radians) of the water vortex between two ticks of a cast.
    fn vortex_turn(magic: Magic, seed: u32, t: u64) -> f32 {
        let at = |t| magic::vortex_rot(&Ctx::new(&SpiritParams { magic, ..params(Theme::Water, Status::Running, Size::S, t, seed) }));
        at(t + 1) - at(t)
    }

    #[test]
    fn water_normal_spins_very_slowly() {
        for seed in [4, 11, 706_312_658] {
            for t in [10, 500, 65_534, u64::MAX - 2] {
                let [s, n, b] = Magic::ALL.map(|m| vortex_turn(m, seed, t));
                if t % (1 << 16) == (1 << 16) - 1 {
                    continue;
                }
                // about one revolution every 8 to 10 s at 15 fps at Normal
                let rev = TAU / n / 15.0;
                assert!((8.0..=10.0).contains(&rev), "{seed} {t}: normal revolution {rev} s");
                assert!(n > 0.0 && n <= 0.3 * b, "{seed} {t}: normal {n} vs big {b}");
                assert!(n <= 0.3 * s && b >= n, "{seed} {t}: small {s} normal {n} big {b}");
            }
        }
    }

    #[test]
    fn water_vortex_is_glossy_and_shaded() {
        particles::NO_AMBIENT.with(|c| c.set(true));
        for seed in [4, 11, 23] {
            let c = first_cast(seed);
            for magic in [Magic::Normal, Magic::Big] {
                let mut brightest = Vec::new();
                let (mut frames, mut flat) = (Vec::new(), 0usize);
                for t in c + 5..c + 13 {
                    let g = render(&SpiritParams { magic, ..params(Theme::Water, Status::Running, Size::S, t, seed) });
                    let cells: Vec<(u16, u16, f32)> = magic_cells(&g).into_iter().filter_map(|(col, row)| cell(&g, col, row).fg.map(|fg| (col, row, lum(fg)))).collect();
                    let bands: std::collections::BTreeSet<u32> = cells.iter().map(|c| (c.2 / 50.0) as u32).collect();
                    // shaded in at least three luminance bands (on most frames: a
                    // slow normal disc may hold an arm over the rim for a while)
                    flat += usize::from(bands.len() < 3);
                    assert!(bands.len() >= 2, "{seed} {magic:?} {t}: luminance bands {bands:?}");
                    let mean = cells.iter().map(|c| c.2).sum::<f32>() / cells.len() as f32;
                    let top = cells.iter().cloned().fold((0, 0, 0.0f32), |a, b| if b.2 > a.2 { b } else { a });
                    assert!(top.2 > mean + 45.0, "{seed} {magic:?} {t}: highlight {} vs mean {mean}", top.2);
                    // lit from the upper left: the highlight sits up-left of the vortex
                    let (cx, cy) = (cells.iter().map(|c| f32::from(c.0)).sum::<f32>() / cells.len() as f32, cells.iter().map(|c| f32::from(c.1)).sum::<f32>() / cells.len() as f32);
                    assert!(f32::from(top.0) <= cx + 0.5 && f32::from(top.1) <= cy + 0.5, "{seed} {magic:?} {t}: highlight {top:?} vs centre ({cx}, {cy})");
                    brightest.push((top.0, top.1));
                    frames.push(magic_cells(&g).into_iter().map(|(col, row)| (col, row, *cell(&g, col, row))).collect::<Vec<_>>());
                }
                assert!(flat * 4 <= frames.len(), "{seed} {magic:?}: {flat} of {} frames in under three bands", frames.len());
                // the normal arms turn very slowly (see water_normal_spins_very_slowly)
                let turns = if magic == Magic::Normal { 2 } else { 4 };
                assert!(frames.windows(2).filter(|w| w[0] != w[1]).count() >= turns, "{seed} {magic:?}: the arms rotate");
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
            let (mut denser, mut isolated) = (0, 0);
            // in a card: the fog is centred in the room right of the body
            let g0 = geom_of(&SpiritParams { magic, cols: CARD_COLS, ..params(Theme::Poison, Status::Running, Size::S, 0, 1) });
            let (ax, ay) = magic::fog_anchor(&g0);
            let life = magic::fog_life(magic);
            let x_min = 15;
            for seed in 1..40u32 {
                let casts: Vec<u64> = (1..800).filter(|&t| state::casts_at(seed, t)).collect();
                for w in casts.windows(3) {
                    let (prev, c, next) = (w[0], w[1], w[2]);
                    // (no fading skull of the previous Big cast in the way)
                    let quiet = if magic == Magic::Big { life.max(magic::skull_window().1.end) } else { life };
                    if c - prev < quiet || next - c <= 12 {
                        continue;
                    }
                    // the fog alone: pop sparkles of a bubble at the body's right edge may fly into the fog area
                    let at = |age: u64| -> Vec<(f32, f32)> {
                        let g = render_with(&SpiritParams { magic, cols: CARD_COLS, ..params(Theme::Poison, Status::Running, Size::S, c + age, seed) }, false);
                        dots(&g).into_iter().filter(|&(x, _)| x >= x_min).map(|(x, y)| (x as f32 + 0.5 - ax, y as f32 + 0.5 - ay)).collect()
                    };
                    let radius = |d: &[(f32, f32)]| d.iter().map(|p| p.0.hypot(p.1)).fold(0.0f32, f32::max);
                    // the late frame sits at the same point of the puffs' life at every level
                    let (early, late) = (at(2), at(life * 12 / 26));
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
                    if inner > outer {
                        denser += 1;
                    } else {
                        println!("{magic:?} {seed} {c}: density inner {inner} vs edge {outer}");
                    }
                    isolated += 1;
                    checked += 1;
                }
            }
            // a puff of a few dots is noisy: the big fog on every cast, the
            // smaller normal fog on nearly all
            let need = if magic == Magic::Big { isolated } else { isolated * 8 / 10 };
            assert!(denser >= need, "{magic:?}: denser inside on {denser} of {isolated} casts");
        }
        assert!(checked >= 6, "{checked} isolated casts checked");
    }

    /// Dots of the poison frame at `t` (no bubbles, no ambient bits) inside
    /// the skull's box, as a bitmap over [`magic::SKULL`]'s rows; and how many
    /// dots differ from the skull.
    fn skull_at(magic: Magic, seed: u32, t: u64) -> (Vec<String>, usize) {
        skull_at_for(Size::S, CARD_COLS, magic, seed, t)
    }

    /// [`skull_at`] for a spirit of `size` given `cols` card columns.
    fn skull_at_for(size: Size, cols: u16, magic: Magic, seed: u32, t: u64) -> (Vec<String>, usize) {
        let p = SpiritParams { magic, cols, ..params(Theme::Poison, Status::Running, size, t, seed) };
        particles::NO_AMBIENT.with(|c| c.set(true));
        let d = dots(&render_with(&p, false));
        particles::NO_AMBIENT.with(|c| c.set(false));
        let (x0, y0) = magic::skull_origin(&geom_of(&p));
        let mut off = 0;
        let rows: Vec<String> = magic::SKULL
            .iter()
            .enumerate()
            .map(|(r, want)| {
                want.chars()
                    .enumerate()
                    .map(|(c, w)| {
                        let on = d.contains(&(x0 + c as i32, y0 + r as i32));
                        off += usize::from(on != (w == '#'));
                        if on { '#' } else { '.' }
                    })
                    .collect()
            })
            .collect();
        (rows, off)
    }

    /// Casts of `seed` in ticks 1..800 whose skull is not mixed up with a
    /// neighbour's: the previous cast's skull is gone by 4 ticks after it
    /// (or not formed yet) and the next cast's skull forms only after this
    /// one is gone.
    fn skull_casts(seed: u32) -> Vec<u64> {
        let (_, whole) = magic::skull_window();
        let casts: Vec<u64> = (1..800).filter(|&t| state::casts_at(seed, t)).collect();
        casts
            .windows(3)
            .filter(|w| {
                let (prev, next) = (w[1] - w[0], w[2] - w[1]);
                (prev + 4 >= whole.end || prev + 4 < whole.start) && next + whole.start >= whole.end
            })
            .map(|w| w[1])
            .collect()
    }

    #[test]
    fn poison_big_fog_ends_in_a_skull() {
        let skull_dots = magic::SKULL.iter().flat_map(|r| r.chars()).filter(|&c| c == '#').count();
        let (hold, end) = magic::skull_window();
        let mut checked = 0;
        for seed in [4, 11, 23, 37, 706_312_658] {
            for c in skull_casts(seed) {
                // formed and held for 6 to 8 ticks near the end of the cast
                assert!((6..=8).contains(&(hold.end - hold.start)), "hold {hold:?}");
                for age in hold.clone() {
                    let (rows, off) = skull_at(Magic::Big, seed, c + age);
                    assert!(off <= 4, "{seed} {c}+{age}: {off} dots off the skull\n{}", rows.join("\n"));
                }
                // not before the fog settles
                assert!(skull_at(Magic::Big, seed, c + 4).1 > skull_dots / 3, "{seed} {c}: no skull early on");
                // it fades out: dimmer, then gone
                let p = |age| SpiritParams { magic: Magic::Big, cols: CARD_COLS, ..params(Theme::Poison, Status::Running, Size::S, c + age, seed) };
                let bright = |age| {
                    let g = render_with(&p(age), false);
                    let (x0, y0) = magic::skull_origin(&geom_of(&p(age)));
                    let (c0, r0) = ((x0 / 2) as u16, (y0 / 4) as u16);
                    (c0..c0 + 7).flat_map(|c| (r0..r0 + 4).map(move |r| (c, r))).filter(|&(c, r)| c < g.cols && r < g.rows).filter_map(|(c, r)| cell(&g, c, r).fg).map(lum).fold(0.0f32, f32::max)
                };
                assert!(bright(hold.end + 2) < bright(hold.start + 2) - 30.0, "{seed} {c}: fades {} -> {}", bright(hold.start + 2), bright(hold.end + 2));
                assert!(skull_at(Magic::Big, seed, c + end.end).1 > skull_dots / 2, "{seed} {c}: gone after the fade");
                // Small and Normal fog never form it
                for magic in [Magic::Small, Magic::Normal] {
                    for age in hold.clone() {
                        assert!(skull_at(magic, seed, c + age).1 > skull_dots / 2, "{magic:?} {seed} {c}+{age}: no skull");
                    }
                }
                checked += 1;
            }
        }
        println!("{checked} casts checked");
        assert!(checked >= 10, "{checked} casts checked");
    }

    #[test]
    fn poison_skull_is_centred_pale_and_leaves_the_rest_alone() {
        let (hold, _) = magic::skull_window();
        for seed in [4, 11, 706_312_658] {
            let c = first_cast(seed);
            let p = SpiritParams { magic: Magic::Big, cols: CARD_COLS, ..params(Theme::Poison, Status::Running, Size::S, c + hold.start + 2, seed) };
            let g = geom_of(&p);
            // centred on the burst anchor
            let (ax, ay) = magic::fog_anchor(&g);
            let (x0, y0) = magic::skull_origin(&g);
            let (w, h) = (magic::SKULL[0].len() as f32, magic::SKULL.len() as f32);
            assert!((x0 as f32 + w / 2.0 - ax).abs() <= 1.0 && (y0 as f32 + h / 2.0 - ay).abs() <= 2.0, "{seed}: skull at ({x0}, {y0}) vs anchor ({ax}, {ay})");
            // about as big as the big fog: 6 to 7 columns by 3 to 4 rows
            assert!((12.0..=14.0).contains(&w) && (12.0..=16.0).contains(&h), "skull {w}x{h} sub-pixels");
            // pale green-grey, never pure white, deterministic
            let grid = render(&p);
            assert_eq!(grid, render(&p));
            let bone: Vec<(u8, u8, u8)> = (0..grid.rows).flat_map(|r| (0..grid.cols).map(move |c| (c, r))).filter_map(|(c, r)| cell(&grid, c, r).fg).filter(|&(r, g, b)| r > 150 && g > 180 && b > 150 && u16::from(g) > u16::from(r) + 10).collect();
            assert!(!bone.is_empty(), "{seed}: pale skull cells");
            assert!(bone.iter().all(|&(r, g, b)| g >= r && g >= b && g < 250), "{seed}: green-grey, never white {bone:?}");
        }
        // the other elements cast as before at Big
        for theme in [Theme::Fire, Theme::Water, Theme::Lightning] {
            for seed in [4, 11] {
                let (hold, _) = magic::skull_window();
                for c in skull_casts(seed).into_iter().take(2) {
                    let p = SpiritParams { magic: Magic::Big, cols: CARD_COLS, ..params(theme, Status::Running, Size::S, c + hold.start + 2, seed) };
                    let g = render(&p);
                    assert!(g.cells.iter().all(|c| c.fg.is_none_or(|(r, gg, b)| !(r > 150 && gg > 180 && b > 150 && u16::from(gg) > u16::from(r) + 10 && gg >= b && r.abs_diff(b) < 40))), "{theme:?} {seed} {c}: no skull");
                }
            }
        }
    }

    /// The bolt plans of every cast of seeds 1..30 in ticks 1..300 in the
    /// standard card, with the centre of the magic area (sub-pixels).
    fn bolt_plans(magic: Magic) -> Vec<(u32, u64, f32, Vec<magic::PlannedBolt>)> {
        bolt_plans_for(Size::S, CARD_COLS, magic)
    }

    /// [`bolt_plans`] for a spirit of `size` given `cols` card columns.
    fn bolt_plans_for(size: Size, cols: u16, magic: Magic) -> Vec<(u32, u64, f32, Vec<magic::PlannedBolt>)> {
        let mut out = Vec::new();
        for seed in 1..30u32 {
            for t in (1..300).filter(|&t| state::casts_at(seed, t)) {
                let p = SpiritParams { magic, cols, ..params(Theme::Lightning, Status::Running, size, t, seed) };
                let g = geom_of(&p);
                out.push((seed, t, magic::area_mid(&g), magic::bolt_plan(&g, &Ctx::new(&p))));
            }
        }
        out
    }

    /// Mean x of a bolt's zig-zag points.
    fn bolt_x(b: &magic::PlannedBolt) -> f32 {
        b.pts.iter().map(|p| p.0).sum::<f32>() / b.pts.len() as f32
    }

    #[test]
    fn lightning_bolt_groups_per_level() {
        for magic in Magic::ALL {
            let plans = bolt_plans(magic);
            assert!(plans.len() >= 100, "{magic:?}: {} casts", plans.len());
            let mut layouts = std::collections::BTreeSet::new();
            for (seed, t, mid, plan) in &plans {
                let mid = *mid;
                let widths: Vec<i32> = plan.iter().map(|b| b.width).collect();
                let want: &[i32] = match magic {
                    Magic::Small => &[1],
                    Magic::Normal => &[1, 1, 1, 1],
                    Magic::Big => &[3, 1, 1, 1],
                };
                assert_eq!(widths, want, "{magic:?} {seed} {t}: bolts and widths (thick first)");
                // the bolts flash on the cast tick or a tick or two later
                assert!(plan.iter().any(|b| b.delay == 0) && plan.iter().all(|b| b.delay <= 2), "{magic:?} {seed} {t}: delays");
                if plan.len() > 1 {
                    assert!(plan.iter().any(|b| b.delay != plan[0].delay), "{magic:?} {seed} {t}: they never all flash together");
                }
                // the group is centred: its width-weighted centroid within one column
                let w: f32 = plan.iter().map(|b| b.width as f32).sum();
                let cx = plan.iter().map(|b| bolt_x(b) * b.width as f32).sum::<f32>() / w;
                assert!((cx - mid).abs() <= 2.0, "{magic:?} {seed} {t}: group centroid {cx} vs {mid}");
                // every bolt strikes top to bottom inside the area
                for b in plan {
                    assert_eq!(b.pts.first().map(|p| p.1), Some(0.0), "{magic:?} {seed} {t}: from the top");
                    assert_eq!(b.pts.last().map(|p| p.1), Some(15.0), "{magic:?} {seed} {t}: to the bottom");
                    assert!(b.pts.iter().all(|p| p.0 >= 14.0 && p.0 < 34.0), "{magic:?} {seed} {t}: inside the area {:?}", b.pts);
                }
                match magic {
                    Magic::Small => {}
                    // four thin bolts in distinct columns, spread over the area
                    Magic::Normal => {
                        let xs: std::collections::BTreeSet<i32> = plan.iter().map(|b| bolt_x(b).round() as i32).collect();
                        assert_eq!(xs.len(), 4, "{seed} {t}: distinct columns {xs:?}");
                        let span = xs.last().unwrap_or(&0) - xs.first().unwrap_or(&0);
                        assert!(span >= 6, "{seed} {t}: spread {span} dots");
                    }
                    // one thick bolt in the centre, three thin ones around it
                    // that never touch it
                    Magic::Big => {
                        assert!((bolt_x(&plan[0]) - mid).abs() <= 1.0, "{seed} {t}: thick bolt at {} vs {mid}", bolt_x(&plan[0]));
                        let thick_reach = plan[0].pts.iter().map(|p| (p.0.round() - mid).abs()).fold(0.0f32, f32::max) + 1.0;
                        for b in &plan[1..] {
                            let d = (bolt_x(b) - mid).abs();
                            assert!((3.0..=8.0).contains(&d), "{seed} {t}: thin bolt {d} dots off centre");
                            assert!(b.pts.iter().all(|p| (p.0.round() - mid).abs() >= thick_reach + 1.0), "{seed} {t}: thin bolt clear of the thick one");
                        }
                        let sides: std::collections::BTreeSet<bool> = plan[1..].iter().map(|b| bolt_x(b) > mid).collect();
                        assert_eq!(sides.len(), 2, "{seed} {t}: thin bolts on both sides");
                    }
                }
                layouts.insert(plan.iter().flat_map(|b| b.pts.iter().map(|p| (p.0.round() as i32, p.1 as i32))).collect::<Vec<_>>());
            }
            // every cast strikes new paths at new positions
            assert!(layouts.len() * 10 >= plans.len() * 9, "{magic:?}: {} layouts over {} casts", layouts.len(), plans.len());
        }
        // same tick, different seeds: different layouts
        let at = |seed| {
            let p = SpiritParams { magic: Magic::Normal, cols: CARD_COLS, ..params(Theme::Lightning, Status::Running, Size::S, first_cast(4), seed) };
            magic::bolt_plan(&geom_of(&p), &Ctx::new(&SpiritParams { seed, ..p })).iter().map(|b| b.pts[0].0.round() as i32).collect::<Vec<_>>()
        };
        assert_ne!(at(4), at(5));
    }

    #[test]
    fn lightning_bolts_paint_their_widths() {
        particles::NO_AMBIENT.with(|c| c.set(true));
        for magic in Magic::ALL {
            let (mut thick_ok, mut thin_ok, mut thick_n, mut thin_n) = (0, 0, 0, 0);
            for (seed, t, _, plan) in bolt_plans(magic) {
                for b in &plan {
                    let d = magic_dots(&SpiritParams { magic, cols: CARD_COLS, ..params(Theme::Lightning, Status::Running, Size::S, t + b.delay, seed) });
                    // the run of dots through the bolt's middle point
                    let (x, y) = b.pts[b.pts.len() / 2];
                    let (x, y) = (x.round() as i32, y as i32);
                    let run = dot_runs(&d, y, 0, 80).into_iter().find(|r| (r.0..r.0 + r.1).contains(&x)).map_or(0, |r| r.1);
                    if b.width == 3 {
                        thick_n += 1;
                        thick_ok += usize::from(run >= 3);
                    } else {
                        thin_n += 1;
                        thin_ok += usize::from((1..=2).contains(&run));
                    }
                }
            }
            println!("{magic:?}: thick {thick_ok}/{thick_n} thin {thin_ok}/{thin_n}");
            assert!(thick_ok == thick_n, "{magic:?}: thick bolts three dots wide on {thick_ok} of {thick_n}");
            assert!(thin_ok * 10 >= thin_n * 8, "{magic:?}: thin bolts one dot wide on {thin_ok} of {thin_n}");
            assert_eq!(thick_n > 0, magic == Magic::Big, "{magic:?}: only Big has a thick bolt");
        }
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

    /// (size, magic, tick, seed, warning) of the golden frames, rendered for
    /// every element and state.
    const GOLDEN_CASES: [(Size, Magic, u64, u32, bool); 8] = [
        (Size::S, Magic::Normal, 0, 7, false),
        (Size::S, Magic::Normal, 37, 7, false),
        (Size::S, Magic::Normal, 121, 706_312_658, false),
        (Size::M, Magic::Normal, 500, 3, false),
        (Size::L, Magic::Big, 77, 11, false),
        (Size::S, Magic::Small, 260, 41, false),
        (Size::S, Magic::Big, 40_000_013, u32::MAX, false),
        (Size::S, Magic::Normal, 16, 23, true),
    ];

    fn golden_params() -> Vec<SpiritParams> {
        let mut out = Vec::new();
        for theme in Theme::ALL {
            for status in STATUSES {
                for (size, magic, tick, seed, warning) in GOLDEN_CASES {
                    out.push(SpiritParams { magic, warning, ..params(theme, status, size, tick, seed) });
                }
            }
        }
        out
    }

    /// Hashes of the golden frames, captured from the renderer before the
    /// context stages existed (theme-major, then state, then `GOLDEN_CASES`).
    /// Centring the magic (water, poison, lightning) re-captured the 12
    /// running water/poison frames that show their magic, and the level
    /// rebalance (smaller Normal, clearer Small) the 15 running fire, water
    /// and poison frames casting Small or Normal magic (entries 1-3, 5, 7,
    /// 41-43, 45, 47, 81-83, 85, 87). The slow Normal vortex re-captured
    /// the running water Normal frames 41-43 and 47, and the Big poison
    /// skull the running poison Big frame 86, and the fuller fire Normal the
    /// running fire Normal frames 1-3 and 7. Every other frame is still the
    /// pre-stage render.
    #[rustfmt::skip]
    const GOLDEN: [u64; 160] = [
        0x3512407e8b3be050, 0xeb2592adbe6c0c33, 0x9a975b1f917a0f15, 0x7ddb91cb757afde7, 0xac4e96eb863a74c1, 0x11dad5b57fe9a68b, 0xfc108cd1c4fb901d, 0x9eeed11b458daf2b,
        0xe63151fe9a9034cc, 0x7a8ac6628408efd6, 0xe7c8d4a08b21e8dd, 0xa78308e8344bfc33, 0x593823a0b217a421, 0x3d7198f183a6f699, 0x6b7fdff6fb1ca679, 0x360e578976d6b475,
        0xef4f5337fe84e25d, 0x9491d9830f79a16d, 0xfade110e02982b10, 0xb3cae76b73ec5f64, 0x1bbf5c77d2e7c756, 0xb35def89ef4ff9c9, 0xe1361db73402ebb5, 0xae106a39f3427582,
        0xb07554c2511c01b3, 0x7c2ccbca01365697, 0x7324e7ce2c9bce28, 0xa4280c3f454ca467, 0x91eac22fa83d1bcf, 0xddb08feddde44f72, 0x7201e9d0b17de902, 0x0f45b90a4d2df7c3,
        0x9f8f36f54ef0c55f, 0x3bc34a7c5a30173a, 0x795315231571603c, 0xac841e8c4f701af2, 0x2bd983eaf378328d, 0x4684d7e93e1ecf26, 0x263984a74cd48d43, 0x9033c7cabef7dcce,
        0xe0f1d9d204dbb53b, 0x47136928f00dee87, 0x0c53bb4526ba73df, 0x0f1d25da1a6fc8d7, 0xcff087d2152ea409, 0x80fa72fb50148235, 0x724c882ced531194, 0xf063e204ce83a10e,
        0x623cb10f33e21bd2, 0xb81b2bfc61e53c3b, 0xeba92b71721f1094, 0x856abff4dcb80e08, 0x37d6e13fa7a6f6d8, 0xcee480f5bb3ba604, 0xf74fc0ab09547254, 0x67fdef386bcdef4a,
        0xd82350e5a41e871f, 0x6eda02e347dd31c1, 0x03f9c0c849cfb49c, 0xc0e10b81431aa916, 0xd021c09188dbd3c3, 0x6b4b5378291cb075, 0x5eab5e9a96b16fcf, 0x3e5bad230a45cfd7,
        0xa98ffb855054ad80, 0x5e2ff43d23c35201, 0x12d998a33761e63a, 0x47de1dd41ee777b0, 0xdc6417a2c1fe9a9e, 0x97a0921451361b80, 0x4e4fcdacb9d69a47, 0xd63cde832ab3eb0e,
        0x48397efc5c8c3b41, 0x21da5b3f472a49f0, 0xec21ee809d8c883b, 0x7cc2ad766f426d99, 0xf2c3f4f1bafe3b20, 0x4f3d4438efbd360d, 0x900a07b09bae5fb7, 0xf15983afbd5a66d6,
        0x17ba577ce79f9d69, 0xb04e9224431280a2, 0xa3463cda315301aa, 0x4a8dd78570dd3bce, 0x00e6fb96b9d8dbe3, 0x90b03f33f68f12ed, 0xd0a1eaecff347496, 0x31f64658867de74e,
        0x6e6f83e990bad121, 0xe8c937a5b5565162, 0x06b998dbd08c3922, 0x1de1bdf0cc3207fa, 0xdd9a20840cdd6d4f, 0xbf9b1e45c6f184c3, 0xfea08ed50dc958b6, 0xbdfa8d0c6164ec15,
        0xaf99522f19ebd5d3, 0x7ed6f69b81fdce30, 0x59431f82976b233e, 0x153e240a19ecca34, 0xcdddb889d9b805a9, 0x91a61bcdb1643167, 0xb1e025b92ee2cecd, 0x3f47116c7a438ad6,
        0x7850eb98fd19d4ac, 0x1505f53f641c81d0, 0x8bb55120b1e5e349, 0xebd099f4dc831e14, 0xba8e77ce1e06556f, 0x8244f1770964617e, 0xf70329b6cbe5386c, 0x8649e9e257ac4035,
        0xc51a594a36d6eb0b, 0x0443307dc0b76eab, 0x86a09c1e3c1e4b46, 0x68408615ea11fe06, 0x25281e71e40a8a8f, 0x0c25d9cff723c5bf, 0x1ee0b525498642a6, 0xb35321a2a8c8dbf8,
        0xef794c374d54696e, 0xd39652c7a2c18969, 0xba6346cfd3b9e32b, 0x208e0aa92cf22a0b, 0xb532387caa6bdd62, 0xb86910b66f35c897, 0x912f9416f8de1284, 0x4e1be45bde2e41fc,
        0xfb3713c21a6230d7, 0xb33e274192e20762, 0xe8104ddd4a1dcb9a, 0x05b830092ef9969f, 0x6b76a9168c876604, 0x843fad5de4e5cdd0, 0x94a53e381b28374e, 0xbd018aafa4a4202f,
        0x4b2bd8f000ed4f8e, 0xc742374a9609c975, 0xe5d5ea037ca3c896, 0xe3f620214d9bf044, 0xac416beafc647659, 0x9a84ac2dd347ba31, 0x8782b9b74e536aac, 0x108a70c8603a8625,
        0xd722fa259fae0021, 0x769a96d2255c9b88, 0x8063af057cf6a7a6, 0x159cb7e91c52db52, 0x154ed869103fe1b3, 0x596551fe955412ba, 0x3126e2448821be5f, 0xc1d40b4db9a98e9a,
        0xbdd86d15bb794e41, 0xaa80d38b83ba5833, 0x8b7c9e6147ebb050, 0xeed97e439df07233, 0x3468e1d313da2c43, 0xc9e7612e6d630db6, 0x4d6cbff1a0ca9ea6, 0x3333d3cd9698332f,
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
        let cases: [(Option<f32>, Stage); 12] = [
            (None, Stage::Lively),
            (Some(f32::NAN), Stage::Lively),
            (Some(-5.0), Stage::Lively),
            (Some(0.0), Stage::Relaxed),
            (Some(19.9), Stage::Relaxed),
            (Some(20.0), Stage::Lively),
            (Some(49.9), Stage::Lively),
            (Some(50.0), Stage::Struggling),
            (Some(70.0), Stage::Struggling),
            (Some(95.0), Stage::Struggling),
            (Some(100.0), Stage::Struggling),
            (Some(f32::INFINITY), Stage::Struggling),
        ];
        for (pct, want) in cases {
            assert_eq!(stage_for(pct), want, "{pct:?}");
            // one set of thresholds: stage n casts magic level n
            let level = match want {
                Stage::Relaxed => Magic::Small,
                Stage::Lively => Magic::Normal,
                Stage::Struggling => Magic::Big,
            };
            assert_eq!(magic_for(pct), level, "{pct:?}");
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
                    let drops = state::sweat_drops(&Ctx::new(&p), &geom_for(p.size, p.magic), CY);
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
                        assert!(state::sweat_drops(&Ctx::new(&q), &geom_for(q.size, q.magic), CY).is_empty(), "{stage:?}: no sweat");
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
                    let gm = geom_for(p.size, p.magic);
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
                for size in Size::ALL {
                    for magic in Magic::ALL {
                        for (tick, seed) in ticks.iter().flat_map(|&t| [0, 1, u32::MAX].map(|s| (t, s))) {
                            let p = SpiritParams { stage, magic, warning: tick % 2 == 0, ..params(theme, status, size, tick, seed) };
                            let g = render(&p);
                            assert_eq!(g, render(&p), "deterministic");
                            assert_eq!((g.cols, g.rows), grid_size_for(size, magic), "{p:?}: inside the canvas");
                            assert_eq!(g.cells.len(), usize::from(g.cols) * usize::from(g.rows));
                        }
                    }
                }
            }
            for (theme, size) in Theme::ALL.into_iter().flat_map(|t| Size::ALL.map(|s| (t, s))) {
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
    const STD_COLS: u16 = 17;
    /// Columns the large card gives its S spirit (inner width 30 minus the pad).
    const LARGE_COLS: u16 = 29;
    const STAGES: [Stage; 3] = [Stage::Relaxed, Stage::Lively, Stage::Struggling];
    /// First sub-pixel column of the XS magic area (the body is left of it).
    const XS_AREA: i32 = 12;

    fn xs(theme: Theme, status: Status, tick: u64, seed: u32) -> SpiritParams {
        params(theme, status, Size::XS, tick, seed)
    }

    /// Body dots of a frame: magic, poison bubbles and ambient bits off,
    /// everything left of the magic area.
    fn xs_body(p: &SpiritParams) -> std::collections::BTreeSet<(i32, i32)> {
        particles::NO_AMBIENT.with(|c| c.set(true));
        magic::NO_MAGIC.with(|c| c.set(true));
        let g = render_with(p, false);
        magic::NO_MAGIC.with(|c| c.set(false));
        particles::NO_AMBIENT.with(|c| c.set(false));
        let marks: Vec<(i32, i32)> = chars(&g).into_iter().filter(|c| !is_braille(c.2) && c.2 != ' ').map(|c| (i32::from(c.0), i32::from(c.1))).collect();
        dots(&g).into_iter().filter(|d| d.0 < XS_AREA && !marks.contains(&(d.0 / 2, d.1 / 4))).collect()
    }

    #[test]
    fn xs_is_a_small_round_spirit_centred_in_the_band() {
        assert_eq!(grid_size(Size::XS), (7, 4), "7 columns, as tall as the 4-row card band");
        assert!(!Size::ALL.contains(&Size::XS), "S/M/L stay the large card's sizes");
        assert_eq!(max_grid_size(), (17, 4));
        assert_eq!(grid_size_for(Size::XS, Magic::Small), (7, 4));
        assert_eq!(grid_size_for(Size::XS, Magic::Normal), (7, 4));
        assert_eq!(grid_size_for(Size::XS, Magic::Big), (17, 4), "big magic gets the standard card's 17 columns");
        for theme in Theme::ALL {
            for status in STATUSES {
                for magic in Magic::ALL {
                    let g = render(&SpiritParams { magic, ..xs(theme, status, 30, 1) });
                    assert_eq!((g.cols, g.rows), grid_size_for(Size::XS, magic), "{theme:?} {status:?} {magic:?}");
                    assert_eq!(g.cells.len(), usize::from(g.cols) * usize::from(g.rows));
                }
                // a running spirit widens to the card room for its magic, like S
                let g = render(&SpiritParams { cols: STD_COLS, ..xs(theme, status, 30, 1) });
                let wide = status == Status::Running;
                assert_eq!(g.cols, if wide { STD_COLS } else { 7 }, "{theme:?} {status:?}");
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
    fn xs_every_state_theme_stage_and_magic_renders() {
        for theme in Theme::ALL {
            for status in STATUSES {
                for stage in STAGES {
                    for magic in Magic::ALL {
                        for cols in [0, STD_COLS] {
                            let g = render(&SpiritParams { stage, magic, cols, ..xs(theme, status, 40, 3) });
                            let n = g.cells.iter().filter(|c| is_braille(c.ch)).count();
                            assert!(n >= 8, "{theme:?} {status:?} {stage:?} {magic:?}: {n} braille cells");
                            assert!(g.cells.iter().all(|c| c.fg.is_some() || c.ch == ' '), "drawn cells have a colour");
                        }
                    }
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
                        // the dot looks one sub-pixel toward the magic while casting
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
                assert!(idle < run * 0.8, "{theme:?} {t}: idle {idle} vs running {run}");
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
            for magic in Magic::ALL {
                for t in 0..120 {
                    let p = SpiritParams { magic, cols: STD_COLS, ..xs(theme, status, t, 41) };
                    let (g, bare) = (render_with(&p, true), render_with(&p, false));
                    for (i, c) in g.cells.iter().enumerate().filter(|(_, c)| c.fg == WHITE) {
                        if bits(c).count_ones() == 1 && bare.cells[i] == *c {
                            continue;
                        }
                        assert!(theme == Theme::Poison && bare.cells[i] != *c && bits(c).count_ones() >= 2 && i as u16 / g.cols >= 2, "{p:?} cell {i}: {c:?}");
                    }
                }
            }
        }
        // fire breath leaves the mouth with one empty dot column
        particles::NO_AMBIENT.with(|c| c.set(true));
        let (mut ok, mut n) = (0, 0);
        for seed in [4, 11, 23] {
            let start = first_cast(seed) + 3;
            for t in start..start + 60 {
                let p = SpiritParams { cols: STD_COLS, ..xs(Theme::Fire, Status::Running, t, seed) };
                let m = magic_dots(&p);
                let Some(first) = m.iter().map(|d| d.0).min() else { continue };
                let edge = xs_body(&p).iter().map(|d| d.0).max().unwrap();
                n += 1;
                if first == edge + 2 {
                    ok += 1;
                }
            }
        }
        particles::NO_AMBIENT.with(|c| c.set(false));
        assert!(n > 30 && ok * 10 >= n * 9, "the flame starts one dot off the mouth on {ok} of {n} frames");
        // the casting pose leans toward the magic, then recoils
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
    fn xs_magic_levels_are_clear_steps() {
        for theme in Theme::ALL {
            let [s, n, b] = Magic::ALL.map(|m| level_metrics_for(Size::XS, STD_COLS, theme, m));
            println!("{theme:?} XS: cells S {:.1} N {:.1} B {:.1}; width S {:.1} N {:.1} B {:.1}; height S {:.2} N {:.2} B {:.2}; far {} {} {}", s.cells, n.cells, b.cells, s.width, n.width, b.width, s.height, n.height, b.height, s.max_far, n.max_far, b.max_far);
            assert!(s.cells < 0.75 * n.cells && n.cells < 0.75 * b.cells, "{theme:?}: cells {} < {} < {}", s.cells, n.cells, b.cells);
            assert!(s.lit_cells < n.lit_cells && n.lit_cells < b.lit_cells, "{theme:?}: lit cells {} < {} < {}", s.lit_cells, n.lit_cells, b.lit_cells);
            assert!(s.width + 1.0 <= n.width && n.width + 1.0 <= b.width, "{theme:?}: width {} < {} < {}", s.width, n.width, b.width);
            assert!(s.union <= n.union && n.union < b.union, "{theme:?}: extent {} {} {}", s.union, n.union, b.union);
            // small is a clear little shape, never a stray dot
            assert!(s.lit_cells >= 2.0, "{theme:?}: small paints {} cells", s.lit_cells);
            // everything stays in the standard card's area
            assert!(b.max_far < STD_COLS, "{theme:?}: big reaches col {}", b.max_far);
        }
    }

    /// Bounding box (columns, rows) of the cells holding `d`.
    fn cell_box(d: &std::collections::BTreeSet<(i32, i32)>) -> (i32, i32) {
        let cols: std::collections::BTreeSet<i32> = d.iter().map(|p| p.0 / 2).collect();
        let rows: std::collections::BTreeSet<i32> = d.iter().map(|p| p.1 / 4).collect();
        match (cols.first().zip(cols.last()), rows.first().zip(rows.last())) {
            (Some((c0, c1)), Some((r0, r1))) => (c1 - c0 + 1, r1 - r0 + 1),
            _ => (0, 0),
        }
    }

    /// The largest cell box of a running XS element's magic over a few casts.
    fn xs_peak_box(theme: Theme, magic: Magic) -> (i32, i32) {
        particles::NO_AMBIENT.with(|c| c.set(true));
        let mut best = (0, 0);
        for seed in [4, 11, 23] {
            for t in 1..200 {
                let bx = cell_box(&magic_dots(&SpiritParams { magic, cols: STD_COLS, ..xs(theme, Status::Running, t, seed) }));
                if bx.0 * bx.1 > best.0 * best.1 {
                    best = bx;
                }
            }
        }
        particles::NO_AMBIENT.with(|c| c.set(false));
        best
    }

    #[test]
    fn xs_magic_is_big_and_bold_enough_to_read() {
        // fire: a bright breath at least 3 columns long at Normal, starting
        // one dot off the mouth (column 6)
        let n = level_metrics_for(Size::XS, STD_COLS, Theme::Fire, Magic::Normal);
        assert!(n.far - 5.0 >= 3.0, "normal flame {} columns", n.far - 5.0);
        let mut bright = 0;
        for t in 1..200 {
            let g = render(&SpiritParams { cols: STD_COLS, ..xs(Theme::Fire, Status::Running, t, 4) });
            bright += (6..g.cols).flat_map(|c| (0..g.rows).map(move |r| (c, r))).filter(|&(c, r)| cell(&g, c, r).fg.is_some_and(|f| f.0 > 240 && f.1 > 190)).count();
        }
        assert!(bright > 100, "a bright yellow flame: {bright} cells");
        // water: a disc of at least 3 x 2 cells with a bright highlight
        let (w, h) = xs_peak_box(Theme::Water, Magic::Normal);
        assert!(w >= 3 && h >= 2, "water normal disc {w}x{h} cells");
        let glint = (1..200).any(|t| {
            let g = render(&SpiritParams { cols: STD_COLS, ..xs(Theme::Water, Status::Running, t, 4) });
            (6..g.cols).flat_map(|c| (0..g.rows).map(move |r| (c, r))).any(|(c, r)| cell(&g, c, r).fg.is_some_and(|f| f.0 > 200 && f.1 > 230 && f.2 > 240))
        });
        assert!(glint, "water highlight");
        // poison: round puffs of at least 3 x 2 cells
        let (w, h) = xs_peak_box(Theme::Poison, Magic::Normal);
        assert!(w >= 3 && h >= 2, "poison normal puffs {w}x{h} cells");
        // every element's Normal and Big are at least 3 x 2 cells
        for theme in Theme::ALL {
            for magic in [Magic::Normal, Magic::Big] {
                let (w, h) = xs_peak_box(theme, magic);
                assert!(w >= 3 && h >= 2, "{theme:?} {magic:?}: {w}x{h} cells");
            }
        }
        // lightning: thin bolts one dot wide, the thick one at least two;
        // Normal four thin, Big one thick and three thin, inside the area
        for magic in Magic::ALL {
            let plans = bolt_plans_for(Size::XS, STD_COLS, magic);
            assert!(plans.len() >= 100);
            for (seed, t, mid, plan) in &plans {
                let thick: Vec<i32> = plan.iter().map(|b| b.width).filter(|&w| w > 1).collect();
                let thin = plan.iter().filter(|b| b.width == 1).count();
                let want = match magic {
                    Magic::Small => (0, 1),
                    Magic::Normal => (0, 4),
                    Magic::Big => (1, 3),
                };
                assert_eq!((thick.len(), thin), want, "{magic:?} {seed} {t}");
                assert!(thick.iter().all(|&w| w >= 2), "{magic:?}: thick {thick:?}");
                for b in plan {
                    assert!(b.pts.iter().all(|p| p.0 >= XS_AREA as f32 && p.0 < 34.0), "{magic:?} {seed} {t}: inside the area {:?}", b.pts);
                }
                let w: f32 = plan.iter().map(|b| b.width as f32).sum();
                let cx = plan.iter().map(|b| bolt_x(b) * b.width as f32).sum::<f32>() / w;
                assert!((cx - mid).abs() <= 2.0, "{magic:?} {seed} {t}: group centroid {cx} vs {mid}");
            }
        }
    }

    #[test]
    fn xs_big_poison_settles_into_a_skull_on_the_standard_card() {
        // the skull is 7 x 3.5 cells: it fits the standard card's magic area
        let skull_dots = magic::SKULL.iter().flat_map(|r| r.chars()).filter(|&c| c == '#').count();
        let (hold, _) = magic::skull_window();
        let mut checked = 0;
        for seed in [4, 11, 23, 37] {
            for c in skull_casts(seed) {
                for age in hold.clone() {
                    let (rows, off) = skull_at_for(Size::XS, STD_COLS, Magic::Big, seed, c + age);
                    assert!(off <= 4, "{seed} {c}+{age}: {off} dots off the skull\n{}", rows.join("\n"));
                }
                assert!(skull_at_for(Size::XS, STD_COLS, Magic::Normal, seed, c + hold.start).1 > skull_dots / 2, "{seed} {c}: no skull at Normal");
                checked += 1;
            }
        }
        assert!(checked >= 6, "{checked} casts checked");
        let p = SpiritParams { magic: Magic::Big, cols: STD_COLS, ..xs(Theme::Poison, Status::Running, 1, 4) };
        let (x0, y0) = magic::skull_origin(&geom_of(&p));
        assert!(x0 >= XS_AREA && x0 + magic::SKULL[0].len() as i32 <= i32::from(STD_COLS) * 2 && y0 >= 0 && y0 + magic::SKULL.len() as i32 <= 16, "skull at ({x0}, {y0}) inside the area");
    }

    #[test]
    fn xs_magic_is_centred_and_fire_leaves_the_mouth() {
        particles::NO_AMBIENT.with(|c| c.set(true));
        for theme in [Theme::Water, Theme::Poison, Theme::Lightning] {
            for magic in Magic::ALL {
                for cols in [STD_COLS, 30] {
                    let (mut sum, mut n) = (0.0f32, 0usize);
                    for seed in [4, 11, 23] {
                        for t in 1..300 {
                            let p = SpiritParams { magic, cols, ..xs(theme, Status::Running, t, seed) };
                            for d in magic_dots(&p) {
                                sum += d.0 as f32 + 0.5;
                                n += 1;
                            }
                        }
                    }
                    let g = geom_of(&SpiritParams { magic, cols, ..xs(theme, Status::Running, 1, 4) });
                    let mid = magic::area_mid(&g);
                    let cx = sum / n.max(1) as f32;
                    assert!(n > 20 && (cx - mid).abs() <= 2.0, "{theme:?} {magic:?} {cols}: centroid {cx} vs area centre {mid} ({n} dots)");
                }
            }
        }
        particles::NO_AMBIENT.with(|c| c.set(false));
        // Small and Normal fire keep their length in a wider room; Big
        // roars to the right edge of the room
        for magic in Magic::ALL {
            let far = |cols| level_metrics_for(Size::XS, cols, Theme::Fire, magic).max_far;
            let (std, wide) = (far(STD_COLS), far(30));
            match magic {
                Magic::Big => assert!(wide >= 27 && std >= 15, "{magic:?}: reaches {std} / {wide}"),
                _ => assert_eq!(std, wide, "{magic:?}: same length"),
            }
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
        let g = geom_for(Size::XS, Magic::Normal);
        let drops: Vec<(i32, i32, f32)> = (0..400u64).flat_map(|t| state::sweat_drops(&Ctx::new(&SpiritParams { stage: Stage::Struggling, ..xs(Theme::Water, Status::Running, t, 4) }), &g, CY)).collect();
        assert!(drops.len() > 20, "{} sweat frames", drops.len());
        assert!(drops.iter().all(|d| d.0 >= 0 && d.1 >= 0 && (d.1 as usize) < g.h), "drops on the canvas: {drops:?}");
    }

    #[test]
    fn xs_is_deterministic_and_never_panics() {
        let ticks = [0, 1, 2, 3, 37, 4_000_000_000, u64::MAX - 3, u64::MAX - 1, u64::MAX];
        for stage in STAGES {
            for (theme, status) in Theme::ALL.into_iter().flat_map(|t| STATUSES.map(|s| (t, s))) {
                for magic in Magic::ALL {
                    for cols in [0, 3, STD_COLS, 60] {
                        for (tick, seed) in ticks.iter().flat_map(|&t| [0, 1, u32::MAX].map(|s| (t, s))) {
                            let p = SpiritParams { stage, magic, cols, warning: tick % 2 == 0, ..xs(theme, status, tick, seed) };
                            let g = render(&p);
                            assert_eq!(g, render(&p), "deterministic");
                            assert_eq!(g.rows, 4);
                            assert_eq!(g.cells.len(), usize::from(g.cols) * usize::from(g.rows));
                        }
                    }
                }
            }
        }
    }

    /// (magic, tick, seed) of the XS golden frames, rendered for every
    /// element and state in the standard card (17 columns).
    const XS_GOLDEN_CASES: [(Magic, u64, u32); 3] = [(Magic::Normal, 37, 7), (Magic::Big, 121, 706_312_658), (Magic::Small, 260, 41)];

    /// Hashes of the XS golden frames (theme-major, then state, then
    /// `XS_GOLDEN_CASES`), captured when the standard card switched to the
    /// redesigned 7 x 4 XS spirit (the fuller fire Normal re-captured the
    /// running fire Normal frame 0); `SPIRIT_GOLDEN_PRINT=1` prints them.
    #[rustfmt::skip]
    const XS_GOLDEN: [u64; 60] = [
        0x001ccb6a4925af98, 0xdf3e1a25a64d7685, 0xde5f3b842dbe0876, 0x8c4d12bcaade4c91, 0xe03014127d2a7d60, 0x4eceaf8677977634,
        0x76dae5d953ecc50b, 0xce3e1ea1af447b5e, 0xd0f87e8dcacd0112, 0xca2b781017ea0a19, 0x4a6c7b71d9234dc1, 0x3c78f4e14dd70080,
        0x51e58b4b661fe6da, 0x88ca151c2fea63a4, 0x10dbf51a4aa54620, 0xb5bc5303d1827c2c, 0xb8ba786841e73a6d, 0xbb733524326638ed,
        0xadf281763c8442a2, 0xd251b8529bb0014c, 0x80552ac87dd9d191, 0x7347d102f2e3a7d0, 0x526ac1823fd5c4b3, 0x8b62eaff1ff1d85b,
        0x622283c0042efcd2, 0xb4cc8ffe5745f19c, 0xe06ec5df04641e20, 0x843e78e5e6303f67, 0xfbcc7acbba84cda8, 0xf3e52a21fa050d29,
        0x53a61336823e090d, 0x5f63ae1363cede27, 0x7d2fa0bd10439e38, 0x81c25ec7e3715ee4, 0x3e4c86f438a19153, 0xf4ff42277cda4cdc,
        0x50093fd1738b1d8b, 0xb244de3ecceea0af, 0x7660524feb653a52, 0xb36713694cf59a69, 0x6b9dbaa9614d8012, 0x3755773239293a38,
        0x664ca2df343f6c6b, 0x46401a4f374a6171, 0x66f62f6585feb717, 0x55977cad98989d8d, 0xf1c71edc3fa9a8d0, 0xd30aea150cce9a90,
        0x0cf2c17a7f336fd0, 0xeeab5429268d6078, 0xfd079c364d29b5c3, 0xeeb8cb4fbee6bdd4, 0x8b885a2befa052a5, 0x4d90ebd192feca9d,
        0x9304bde111bcb14b, 0x233be88dcdd929ea, 0xa27c1622d1417215, 0x1dd4a914bfb989c0, 0xe9a19b0927d932cf, 0x19863f428cc452c5,
    ];

    #[test]
    fn xs_golden_frames() {
        let mut ps = Vec::new();
        for theme in Theme::ALL {
            for status in STATUSES {
                for (magic, tick, seed) in XS_GOLDEN_CASES {
                    ps.push(SpiritParams { magic, cols: STD_COLS, ..xs(theme, status, tick, seed) });
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

    /// Prints `g` as rows, in 24-bit colour when `ansi`.
    fn print_grid(g: &SpiritGrid, ansi: bool) {
        for row in 0..g.rows {
            let mut line = String::new();
            for c in 0..g.cols {
                let x = cell(g, c, row);
                match (ansi, x.fg) {
                    (true, Some((r, gg, b))) => line += &format!("\x1b[38;2;{r};{gg};{b}m{}\x1b[0m", x.ch),
                    _ => line.push(x.ch),
                }
            }
            println!("|{line}|");
        }
    }

    /// XS frames per element: at rest (idle), casting at Normal and at Big,
    /// in the standard card (`cargo test dump_xs -- --ignored --nocapture`;
    /// `SPIRIT_ANSI=1` adds colour, `SPIRIT_SEED=…`).
    #[test]
    #[ignore]
    fn dump_xs() {
        let ansi = std::env::var("SPIRIT_ANSI").is_ok();
        let seed = std::env::var("SPIRIT_SEED").ok().and_then(|v| v.parse().ok()).unwrap_or(4);
        let cast = first_cast(seed);
        for theme in Theme::ALL {
            let after = match theme {
                Theme::Poison => 8,
                Theme::Lightning => 0,
                _ => 5,
            };
            println!("{theme:?} rest");
            print_grid(&render(&SpiritParams { cols: STD_COLS, ..xs(theme, Status::Idle, cast + after, seed) }), ansi);
            for (magic, stage) in [(Magic::Normal, Stage::Lively), (Magic::Big, Stage::Struggling)] {
                println!("{theme:?} casting {magic:?}");
                print_grid(&render(&SpiritParams { cols: STD_COLS, magic, stage, ..xs(theme, Status::Running, cast + after, seed) }), ansi);
            }
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

    /// Prints running frames of every element at each context stage (stage
    /// and magic follow the same ctx level), several ticks apart
    /// (`cargo test dump_stages -- --ignored --nocapture`; `SPIRIT_STATUS=idle|input|done`,
    /// `SPIRIT_COLS=…` for the card's magic area).
    #[test]
    #[ignore]
    fn dump_stages() {
        let status = match std::env::var("SPIRIT_STATUS").as_deref() {
            Ok("idle") => Status::Idle,
            Ok("input") => Status::NeedsInput,
            Ok("done") => Status::Done,
            _ => Status::Running,
        };
        let cols = std::env::var("SPIRIT_COLS").ok().and_then(|v| v.parse().ok()).unwrap_or(CARD_COLS);
        let ticks: Vec<u64> = std::env::var("SPIRIT_TICK").unwrap_or_else(|_| "60,61,62,63,64,66".into()).split(',').filter_map(|v| v.trim().parse().ok()).collect();
        for theme in Theme::ALL {
            for (stage, magic) in [(Stage::Relaxed, Magic::Small), (Stage::Lively, Magic::Normal), (Stage::Struggling, Magic::Big)] {
                println!("{theme:?} {stage:?} {status:?}");
                let grids: Vec<SpiritGrid> = ticks.iter().map(|&t| render(&SpiritParams { stage, magic, cols, ..params(theme, status, Size::S, t, 4) })).collect();
                for row in 0..4 {
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

    /// Prints the stage-spec sample frames in the standard card: the slow
    /// Normal water vortex, staggered Normal and Big lightning, and the Big
    /// poison skull (`cargo test dump_stage_specs -- --ignored --nocapture`;
    /// `SPIRIT_SEED=…`).
    #[test]
    #[ignore]
    fn dump_stage_specs() {
        let seed = std::env::var("SPIRIT_SEED").ok().and_then(|v| v.parse().ok()).unwrap_or(4);
        particles::NO_AMBIENT.with(|c| c.set(true));
        let at = |theme, magic, t| render_with(&SpiritParams { magic, cols: CARD_COLS, ..params(theme, Status::Running, Size::S, t, seed) }, false);
        let show = |label: String, frames: Vec<(String, SpiritGrid)>| {
            println!("{label}");
            println!("{}", frames.iter().map(|f| format!("{:<19}", f.0)).collect::<String>());
            for row in 0..frames[0].1.rows {
                let line: String = frames.iter().map(|(_, g)| (0..g.cols).map(|c| cell(g, c, row).ch).collect::<String>() + " ").collect();
                println!("{line}");
            }
            println!();
        };
        let casts: Vec<u64> = (1..800).filter(|&t| state::casts_at(seed, t)).collect();
        let c = casts[2];
        let ticks = |ts: &[u64], theme, magic| ts.iter().map(|&t| (format!("+{}", t - c), at(theme, magic, t))).collect::<Vec<_>>();
        show(format!("water Normal, cast at {c}"), ticks(&[c + 4, c + 6, c + 8, c + 10, c + 12, c + 14], Theme::Water, Magic::Normal));
        show(format!("water Big, cast at {c}"), ticks(&[c + 4, c + 6, c + 8, c + 10, c + 12, c + 14], Theme::Water, Magic::Big));
        for magic in [Magic::Normal, Magic::Big] {
            show(format!("lightning {magic:?}, cast at {c}"), ticks(&[c, c + 1, c + 2, c + 3, c + 4, c + 5], Theme::Lightning, magic));
        }
        let c = skull_casts(seed)[0];
        let ticks = |ts: &[u64], theme, magic| ts.iter().map(|&t| (format!("+{}", t - c), at(theme, magic, t))).collect::<Vec<_>>();
        show(format!("poison Big, cast at {c}"), ticks(&[c + 6, c + 16, c + 22, c + 23, c + 24, c + 25], Theme::Poison, Magic::Big));
        show(format!("poison Big, cast at {c}"), ticks(&[c + 28, c + 31, c + 32, c + 34, c + 36, c + 37], Theme::Poison, Magic::Big));
        particles::NO_AMBIENT.with(|c| c.set(false));
    }

    /// Per-level magic measurements in the standard card (stage 2, seeds
    /// 4/11/23 x 240 running ticks, ambient off): mean painted cells per
    /// frame, mean width and height (cells) of the painted frames, mean and
    /// max farthest painted column, union width
    /// (`cargo test measure_magic -- --ignored --nocapture`).
    #[test]
    #[ignore]
    fn measure_magic() {
        for theme in Theme::ALL {
            for magic in Magic::ALL {
                let m = level_metrics(theme, magic);
                println!(
                    "{theme:?} {magic:?}: cells/frame {:.1} (lit {:.1}) width {:.1} height {:.2} (max {}) far col {:.1} (max {}) union {} dots/frame {:.1}",
                    m.cells, m.lit_cells, m.width, m.height, m.max_height, m.far, m.max_far, m.union, m.dots
                );
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
