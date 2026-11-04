//! Elements: palette, outline shape, decorations and ambient particles per
//! theme. The RUNNING magic cast to the right lives in `magic`.

use std::f32::consts::{PI, TAU};

use crate::model::Status;

use super::canvas::{lerp, mul, Canvas, Rgb, CARD_BG};
use super::noise::{hf, seed_phase, slot, Rng};
use super::particles::Particle;
use super::theme::Theme;
use super::{Ctx, BASE_R, CX};

/// Colour ramp: rim -> dark -> mid -> light.
pub(super) fn ramp(e: Theme) -> [Rgb; 4] {
    match e {
        // red edges around an orange -> yellow core
        Theme::Fire => [[185., 28., 14.], [240., 82., 20.], [255., 155., 35.], [255., 228., 95.]],
        Theme::Water => [[25., 55., 150.], [35., 105., 215.], [70., 165., 245.], [150., 215., 255.]],
        // toxic lime with a dark violet-ish green rim
        Theme::Poison => [[64., 92., 82.], [72., 150., 62.], [122., 212., 58.], [192., 248., 108.]],
        Theme::Lightning => [[140., 100., 0.], [215., 170., 0.], [255., 220., 35.], [255., 250., 165.]],
    }
}

/// Lightning: rotation of the spike star.
pub(super) fn bolt_rot(ctx: &Ctx) -> f32 {
    seed_phase(ctx.seed, 0xB01) + ctx.tf * 0.01 * ctx.sp
}

/// Poison: slow horizontal sway of the body in design units (0 for other
/// elements). Smooth sinusoids, never faster than a small step per tick.
pub(super) fn sway(ctx: &Ctx) -> f32 {
    if ctx.elem != Theme::Poison {
        return 0.0;
    }
    let s = ctx.sp.clamp(0.8, 1.6) * ctx.tf;
    0.55 * (s * 0.03 + seed_phase(ctx.seed, 0x5A7)).sin() + 0.2 * (s * 0.017 + seed_phase(ctx.seed, 0x5A8)).sin()
}

/// Lightning: round core radius + star-burst spike length at angle `th`.
/// Long and short spikes alternate; their lengths flicker every few frames.
pub(super) fn bolt_shape(ctx: &Ctx, th: f32) -> (f32, f32) {
    let (t, tf, sp) = (ctx.t, ctx.tf, ctx.sp);
    let n = 8.0;
    let seg = TAU / n;
    let rot = bolt_rot(ctx);
    let a = (th - rot).rem_euclid(TAU);
    let k = (a / seg).floor();
    let local = a - (k + 0.5) * seg;
    let frame = (if ctx.state == Status::Running { t / 2 } else { t / 3 }) as u32;
    let jf = match ctx.state {
        Status::Running => 1.2,
        Status::NeedsInput => 1.0,
        Status::Idle => 0.5,
        Status::Done => 0.7,
        Status::Unknown => 0.4,
    };
    let small = if ctx.small { 1.3 } else { 1.0 };
    let long = if k as u32 % 2 == 0 { 1.0 } else { 0.45 };
    let len = (1.5 + 3.5 * hf(ctx.seed.wrapping_mul(131).wrapping_add(k as u32), frame)) * long * jf * small;
    let w = seg * 0.5 * if ctx.small { 1.2 } else { 1.0 };
    let tri = (1.0 - local.abs() / w).max(0.0);
    let core = 7.4 + 0.25 * (3.0 * th + 0.07 * tf * sp).sin();
    (core, tri.powf(2.2) * len)
}

/// Outline radius (design units) at angle `th` (screen coords, +y down).
pub(super) fn radius(ctx: &Ctx, th: f32) -> f32 {
    let (tf, sp) = (ctx.tf, ctx.sp);
    let wob = 0.6 * (3.0 * th + 0.07 * tf * sp).sin()
        + 0.4 * (5.0 * th - 0.11 * tf * sp + 1.3).sin()
        + 0.3 * (2.0 * th + 0.05 * tf * sp + 2.0).sin();
    let sn = th.sin();
    match ctx.elem {
        Theme::Fire => {
            let up = (-sn).max(0.0);
            let freq = if ctx.small { 4.0 } else { 6.0 };
            let tongue = (freq * th + tf * 0.2 * sp + 0.7 * (tf * 0.09).sin()).sin().max(0.0).powf(0.6);
            let flick = (2.8 + 1.3 * (tf * 0.37 * sp + th * 4.0).sin()) * if ctx.small { 1.8 } else { 1.0 };
            BASE_R + wob * 0.7 + up.powf(1.6) * tongue * flick
        }
        Theme::Water => BASE_R + wob * 0.4 + sn.max(0.0).powi(2) * 0.6,
        Theme::Poison => {
            // slow travelling undulation of the edge plus the sway (a shift
            // of the whole outline, to first order)
            let s = sp.clamp(0.8, 1.6) * tf;
            let crawl = 0.55 * (3.0 * th - s * 0.045).sin() + 0.3 * (2.0 * th + s * 0.03 + 1.0).sin() + 0.15 * (5.0 * th - s * 0.06 + 2.0).sin();
            BASE_R + crawl + sway(ctx) * th.cos() + sn.max(0.0) * 0.4
        }
        Theme::Lightning => {
            let (core, spike) = bolt_shape(ctx, th);
            core + spike
        }
    }
}

/// Lightning flashes white for 2 frames: every 18..=46 ticks while running,
/// else every 50..=140 ticks.
pub(super) fn flashing(ctx: &Ctx) -> bool {
    if ctx.elem != Theme::Lightning {
        return false;
    }
    let (window, jitter) = if ctx.state == Status::Running { (32, 14) } else { (95, 45) };
    [ctx.t, ctx.t.wrapping_sub(1)].iter().any(|&t| t % window == slot(ctx.seed, 0xF1A5, t / window, jitter))
}

/// Body tone override for one lit sub-pixel (rx, ry) of tone `tone`.
pub(super) fn tint(ctx: &Ctx, ramp: &[Rgb; 4], tone: usize, c: Rgb, d: f32, th: f32, rx: usize, ry: usize, flash: bool) -> Rgb {
    let t = ctx.t;
    match ctx.elem {
        Theme::Water if d > 0.48 && d < 0.66 && th > -2.75 && th < -1.95 => [235., 248., 255.],
        Theme::Lightning => {
            let mut c = c;
            if hf((rx as u32).wrapping_add(ry as u32 * 97).wrapping_add(ctx.seed.wrapping_mul(7919)), t as u32) < 0.05 {
                c = ramp[3];
            }
            if flash {
                c = lerp(c, [255., 255., 255.], 0.75);
            }
            c
        }
        // a white-hot core that flickers
        Theme::Fire if tone >= 2 && hf(rx as u32 * 31 + ry as u32, ((t / 2) as u32).wrapping_add(ctx.seed)) < 0.2 => {
            if tone == 3 {
                [255., 248., 185.]
            } else {
                ramp[3]
            }
        }
        _ => c,
    }
}

/// Colour of a forming poison bubble (bead and ring).
pub(super) const BUBBLE_RING: Rgb = [205., 255., 150.];
/// Colour of the sparkle when a poison bubble pops.
pub(super) const BUBBLE_POP: Rgb = [238., 255., 205.];

/// Life stage of a poison bubble; each lasts one or two ticks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Stage {
    /// a single dot
    Bead,
    /// a tiny ring
    Ring,
    /// a slightly bigger ring
    Swell,
    /// a burst of sparkle dots
    Pop,
}

/// A poison bubble: offset from the body centre (design units) and stage.
pub(super) struct Bubble {
    pub dx: f32,
    pub dy: f32,
    pub stage: Stage,
}

/// Stage by age (ticks) within a bubble's cycle.
fn stage_at(age: u64) -> Option<Stage> {
    match age {
        0 | 1 => Some(Stage::Bead),
        2 | 3 => Some(Stage::Ring),
        4 => Some(Stage::Swell),
        5 => Some(Stage::Pop),
        _ => None,
    }
}

/// Poison bubbles alive at `ctx.t`. A few slots each run a cycle of a seeded
/// length; a cycle holds a bubble with a chance that depends on the state, at
/// a random spot in the lower body. Pure in (seed, tick).
pub(super) fn bubbles(ctx: &Ctx) -> Vec<Bubble> {
    if ctx.elem != Theme::Poison {
        return Vec::new();
    }
    let (slots, chance): (u32, f32) = match ctx.state {
        Status::Running => (2, 0.9),
        Status::NeedsInput => (2, 0.6),
        Status::Done => (4, 0.95),
        Status::Idle => (1, 0.35),
        Status::Unknown => return Vec::new(),
    };
    let mut out = Vec::new();
    for k in 0..slots {
        let key = ctx.seed.wrapping_add(k.wrapping_mul(0x9E37));
        let period = 18 + slot(key, 0xBB1, 0, 14);
        let shifted = ctx.t + slot(key, 0xBB2, 0, period as u32);
        let (cycle, age) = (shifted / period, shifted % period);
        let Some(stage) = stage_at(age) else { continue };
        let c = cycle as u32 ^ (cycle >> 32) as u32;
        if hf(key ^ 0xB0B1, c) >= chance {
            continue;
        }
        // in the lower half of the body, below the eyes
        let a = PI * (0.15 + 0.7 * hf(key ^ 0xB0B2, c));
        let r = BASE_R * (0.35 + 0.3 * hf(key ^ 0xB0B3, c));
        out.push(Bubble { dx: r * a.cos(), dy: r * a.sin() * 0.8, stage });
    }
    out
}

/// Draws one bubble centred on sub-pixel (px, py).
fn draw_bubble(cv: &mut Canvas, px: i32, py: i32, stage: Stage, bright: f32) {
    let ring = mul(BUBBLE_RING, bright);
    let pop = mul(BUBBLE_POP, bright);
    let (dots, hole, c): (&[(i32, i32)], &[(i32, i32)], Rgb) = match stage {
        Stage::Bead => (&[(0, 0)], &[], ring),
        Stage::Ring => (&[(-1, 0), (1, 0), (0, -1), (0, 1)], &[(0, 0)], ring),
        Stage::Swell => (&[(-2, 0), (2, 0), (0, -2), (0, 2), (-1, -1), (1, -1), (-1, 1), (1, 1)], &[(0, 0), (-1, 0), (1, 0), (0, -1), (0, 1)], ring),
        Stage::Pop => (&[(-2, -2), (2, -2), (-2, 2), (2, 2), (-3, 0), (3, 0)], &[(0, 0)], pop),
    };
    for &(x, y) in hole {
        cv.clear_px(px + x, py + y);
    }
    for &(x, y) in dots {
        cv.set_px(px + x, py + y, c, 2, false);
    }
}

/// Element decorations around the body: orbiting droplets and floor ripples
/// (water), bubbles forming and popping on the body (poison).
pub(super) fn decorate(cv: &mut Canvas, ctx: &Ctx, cy: f32, bright: f32) {
    let (tf, sp) = (ctx.tf, ctx.sp);
    let ramp = ramp(ctx.elem);
    match ctx.elem {
        Theme::Water => {
            for k in 0..2 {
                let a = tf * 0.07 * sp + k as f32 * PI;
                let x = CX + (BASE_R + 2.5) * a.cos();
                let y = cy + 1.5 + 2.2 * a.sin();
                cv.blk(x - 0.5, y - 0.5, [150., 215., 255.], 1, a.sin() <= 0.0);
            }
            for k in 0..2 {
                let p = (tf * 0.025 * sp.max(0.6) + k as f32 * 0.5).fract();
                if p < 0.2 {
                    continue;
                }
                let hw = 1.5 + p * 8.0;
                let vr = 0.4 + p * 1.2;
                let c = lerp(ramp[1], CARD_BG, p);
                for i in 0..20 {
                    let a = i as f32 / 20.0 * TAU;
                    cv.blk(CX + hw * a.cos() - 0.5, ctx.floor + vr * a.sin() - 0.5, c, 1, true);
                }
            }
        }
        Theme::Poison => {
            let sx = sway(ctx);
            for b in bubbles(ctx) {
                let (px, py) = cv.to_px(CX + sx + b.dx, cy + b.dy);
                draw_bubble(cv, px, py, b.stage, bright);
            }
        }
        _ => {}
    }
}

/// Longest ambient particle life of an element, in ticks.
pub(super) fn max_life(e: Theme) -> u64 {
    match e {
        Theme::Fire => 22,
        Theme::Water => 40,
        Theme::Poison => 1,
        Theme::Lightning => 4,
    }
}

/// Ambient particles for one tick: embers, drips, sparks and arcs. Poison
/// has none (its bubbles are decorations).
pub(super) fn ambient(e: Theme, act: f32, cy: f32, floor: f32, r: &mut Rng, parts: &mut Vec<Particle>) {
    match e {
        Theme::Fire => {
            if r.chance(0.22 * act) {
                let x = CX + r.range(-5.0, 5.0);
                let y = cy - BASE_R - r.range(0.0, 2.0);
                let (vx, vy) = (r.range(-0.08, 0.08), -r.range(0.15, 0.35));
                let life = r.int(10, 22);
                parts.push(Particle::new(x, y, vx, vy, life, [255., 225., 100.], [190., 35., 10.]));
            }
        }
        Theme::Water => {
            if r.chance(0.05 * act) {
                let x = CX + r.range(-3.0, 3.0);
                let mut p = Particle::new(x, cy + BASE_R + 0.3, 0.0, 0.05, 40, [130., 205., 255.], [60., 140., 235.]);
                p.ay = 0.035;
                p.floor = floor;
                parts.push(p);
            }
        }
        Theme::Poison => {}
        Theme::Lightning => {
            if r.chance(0.25 * act) {
                let a = r.range(0.0, TAU);
                let rr = BASE_R + 1.0;
                let v = r.range(0.25, 0.5);
                let life = r.int(2, 4);
                parts.push(Particle::new(CX + rr * a.cos(), cy + rr * a.sin(), v * a.cos(), v * a.sin(), life, [255., 255., 230.], [255., 200., 40.]));
            }
            if r.chance(0.08 * act) {
                let a0 = r.range(0.0, TAU);
                let span = r.range(0.7, 1.5);
                let steps = (span * (BASE_R + 2.0)) as usize;
                let mut rr = BASE_R + 1.5;
                let life = r.int(2, 3);
                for i in 0..steps {
                    let a = a0 + span * i as f32 / steps as f32;
                    rr = (rr + r.range(-0.8, 0.8)).clamp(BASE_R + 0.8, BASE_R + 3.0);
                    let mut p = Particle::new(CX + rr * a.cos(), cy + rr * a.sin(), 0.0, 0.0, life, [255., 252., 215.], [255., 235., 120.]);
                    p.prio = 2;
                    parts.push(p);
                }
            }
        }
    }
}

/// Whether a particle is still alive and on the design canvas.
pub(super) fn keep(p: &Particle, right: f32) -> bool {
    p.life > 0 && p.x > -2.0 && p.x < right + 1.0 && p.y > -3.0 && p.y < super::DH + 1.0 && p.y < p.floor
}
