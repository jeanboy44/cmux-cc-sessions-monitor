//! Particles, simulated deterministically: every tick draws its spawns from a
//! generator keyed by (seed, tick), so replaying the last few ticks (as long
//! as the longest particle life) reproduces the live simulation exactly.

use crate::model::Status;

use super::canvas::{lerp, Canvas, Rgb, CARD_BG};
use super::noise::Rng;
use super::{element, state, Ctx, CY};

/// Ticks a running spirit keeps glancing at its target after a cast.
const LOOK_TICKS: u64 = 10;

pub(super) struct Particle {
    pub x: f32,
    pub y: f32,
    pub vx: f32,
    pub vy: f32,
    pub ax: f32,
    pub ay: f32,
    pub life: u32,
    pub max: u32,
    pub size: u32,
    pub wide: bool,
    pub c0: Rgb,
    pub c1: Rgb,
    /// drawn behind the body (skips body dots)
    pub behind: bool,
    pub prio: u8,
    /// dies when it falls below this y
    pub floor: f32,
}

impl Particle {
    pub(super) fn new(x: f32, y: f32, vx: f32, vy: f32, life: u32, c0: Rgb, c1: Rgb) -> Self {
        Particle { x, y, vx, vy, ax: 0.0, ay: 0.0, life, max: life, size: 1, wide: false, c0, c1, behind: false, prio: 1, floor: f32::MAX }
    }
}

pub(super) struct Sim {
    pub parts: Vec<Particle>,
    /// running and just cast: the eyes glance one cell toward the target
    pub looking: bool,
}

/// Particles alive at `ctx.t` (ticks start at 1; tick 0 has none).
pub(super) fn simulate(ctx: &Ctx) -> Sim {
    let window = element::max_life(ctx.elem).max(LOOK_TICKS) + 1;
    let start = ctx.t.saturating_sub(window);
    let act = state::activity(ctx.state);
    let running = ctx.state == Status::Running;
    let mut parts: Vec<Particle> = Vec::new();
    let mut last_cast = None;
    for u in start + 1..=ctx.t {
        for p in &mut parts {
            p.x += p.vx;
            p.y += p.vy;
            p.vx += p.ax;
            p.vy += p.ay;
            p.life = p.life.saturating_sub(1);
        }
        parts.retain(element::keep);
        let mut r = Rng::for_tick(ctx.seed, u);
        let cy = CY + state::bob(ctx.state, ctx.seed, super::phase_tick(u));
        element::ambient(ctx.elem, act, cy, &mut r, &mut parts);
        if running && state::casts_at(ctx.seed, u) {
            element::cast(ctx.elem, cy, &mut r, &mut parts);
            last_cast = Some(u);
        }
    }
    let looking = last_cast.is_some_and(|c| ctx.t - c < LOOK_TICKS);
    Sim { parts, looking }
}

pub(super) fn draw(cv: &mut Canvas, parts: &[Particle]) {
    for p in parts {
        let lf = p.life as f32 / p.max as f32;
        let mut c = lerp(p.c0, p.c1, 1.0 - lf);
        if lf < 0.35 {
            c = lerp(CARD_BG, c, lf / 0.35);
        }
        let (x, y) = (p.x - 0.5, p.y - 0.5);
        cv.blk(x, y, c, p.prio, p.behind);
        if p.wide {
            cv.blk(x + 1.0, y, c, p.prio, p.behind);
        }
        if p.size == 2 && lf > 0.5 {
            cv.blk(x + 1.0, y, c, p.prio, p.behind);
            cv.blk(x, y + 1.0, c, p.prio, p.behind);
            cv.blk(x + 1.0, y + 1.0, c, p.prio, p.behind);
        }
    }
}
