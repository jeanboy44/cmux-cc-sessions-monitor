//! Elements: palette, outline shape, decorations and ambient particles per
//! theme. The RUNNING magic cast to the right lives in `magic`.

use std::f32::consts::{PI, TAU};

use crate::model::Status;

use super::canvas::{lerp, mul, Canvas, Rgb, CARD_BG, WHITE};
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

/// How far a poison bubble's dots blend from the colour under them toward
/// the pale lime highlight: the bead, ring and swell brighten as they grow,
/// the gloss dot is brighter still, and the pop sparkles fade back into
/// their surroundings frame by frame.
const BEAD_MIX: f32 = 0.4;
const RING_MIX: f32 = 0.5;
const SWELL_MIX: f32 = 0.62;
const GLOSS_MIX: f32 = 0.85;
const POP_MIX: [f32; 3] = [0.7, 0.4, 0.08];
/// Last frame of a pop (the sparkles have faded into their surroundings).
pub(super) const LAST_POP_FRAME: u8 = POP_MIX.len() as u8 - 1;

/// Life stage of a poison bubble; each lasts one to three ticks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Stage {
    /// a single light dot
    Bead,
    /// a tiny ring
    Ring,
    /// a slightly bigger ring
    Swell,
    /// sparkle dots flying outward and fading ([`Bubble::frame`] 0..=[`LAST_POP_FRAME`])
    Pop,
}

/// A poison bubble: offset from the body centre (design units), stage and
/// frame within the stage (only a pop has more than one).
pub(super) struct Bubble {
    pub dx: f32,
    pub dy: f32,
    pub stage: Stage,
    pub frame: u8,
}

/// Stage and frame by age (ticks) within a bubble's cycle.
fn stage_at(age: u64) -> Option<(Stage, u8)> {
    match age {
        0 | 1 => Some((Stage::Bead, 0)),
        2 | 3 => Some((Stage::Ring, 0)),
        4 => Some((Stage::Swell, 0)),
        5..=7 => Some((Stage::Pop, (age - 5) as u8)),
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
        let Some((stage, frame)) = stage_at(age) else { continue };
        let c = cycle as u32 ^ (cycle >> 32) as u32;
        if hf(key ^ 0xB0B1, c) >= chance {
            continue;
        }
        // in the lower half of the body, below the eyes
        let a = PI * (0.15 + 0.7 * hf(key ^ 0xB0B2, c));
        let r = BASE_R * (0.35 + 0.3 * hf(key ^ 0xB0B3, c));
        out.push(Bubble { dx: r * a.cos(), dy: r * a.sin() * 0.8, stage, frame });
    }
    // casting: one more bubble races through its stages and pops on the release
    if ctx.state == Status::Running {
        use super::state::CastPhase::{Release, WindUp};
        let (t, f) = (ctx.t, |f: u8| u64::from(f));
        let stage = match super::state::cast_phase(ctx.seed, t) {
            Some(WindUp(0)) => Some((Stage::Bead, 0, t + 3)),
            Some(WindUp(1)) => Some((Stage::Ring, 0, t + 2)),
            Some(WindUp(k)) => Some((Stage::Swell, 0, t + 3 - f(k))),
            Some(Release(k)) => Some((Stage::Pop, k, t - f(k))),
            // the last pop frame, the tick after the release
            None if t >= 2 && super::state::casts_at(ctx.seed, t - 2) => Some((Stage::Pop, 2, t - 2)),
            None => None,
        };
        if let Some((stage, frame, cast)) = stage {
            let c = cast as u32 ^ (cast >> 32) as u32;
            let a = PI * (0.15 + 0.7 * hf(ctx.seed ^ 0xCA5B, c));
            let r = BASE_R * (0.35 + 0.3 * hf(ctx.seed ^ 0xCA5C, c));
            out.push(Bubble { dx: r * a.cos(), dy: r * a.sin() * 0.8, stage, frame });
        }
    }
    out
}

/// Bubbles at sub-pixel centres: `cx`/`cy` is the body centre (design units,
/// after bobbing and the casting pose). A bubble never reaches the eye rows:
/// one that would is lowered just below them.
pub(super) fn bubble_spots(cv: &Canvas, ctx: &Ctx, cx: f32, cy: f32) -> Vec<(i32, i32, Bubble)> {
    let sx = sway(ctx);
    let eyes_bottom = super::eyes::boxes(cv).iter().map(|b| (b.1 + b.3) * 4).max().unwrap_or(0);
    bubbles(ctx)
        .into_iter()
        .map(|b| {
            let (px, py) = cv.to_px(cx + sx + b.dx, cy + b.dy);
            (px, py.max(eyes_bottom + 2), b)
        })
        .collect()
}

/// Draws one bubble centred on sub-pixel (px, py) as an overlay: every dot
/// lightens what is under it toward `light` (the body keeps its dots), so a
/// bubble is a glossy ring on the body and a faint ring off it, never a hole.
fn draw_bubble(cv: &mut Canvas, px: i32, py: i32, b: &Bubble, light: Rgb) {
    const DIAMOND: [(i32, i32); 4] = [(-1, 0), (1, 0), (0, -1), (0, 1)];
    const RING: [(i32, i32); 8] = [(-2, 0), (2, 0), (0, -2), (0, 2), (-1, -1), (1, -1), (-1, 1), (1, 1)];
    // the last pop frame has faded into the card: no new dots on empty cells
    let spawn = !(b.stage == Stage::Pop && b.frame >= LAST_POP_FRAME);
    let mut put = |dots: &[(i32, i32)], m: f32| {
        for &(x, y) in dots {
            cv.overlay_px(px + x, py + y, light, m, spawn);
        }
    };
    match b.stage {
        Stage::Bead => put(&[(0, 0)], BEAD_MIX),
        Stage::Ring => {
            put(&DIAMOND, RING_MIX);
            put(&[(-1, -1)], GLOSS_MIX);
        }
        Stage::Swell => {
            put(&RING, SWELL_MIX);
            put(&[(-1, -1)], GLOSS_MIX);
        }
        Stage::Pop => {
            // the ring breaks into sparkles that fly 1-2 cells out and fade
            let k = usize::from(b.frame.min(LAST_POP_FRAME));
            let (d, h, v) = ([2, 3, 4][k], [3, 4, 5][k], [2, 3, 3][k]);
            put(&[(-d, -v), (d, -v), (-d, v), (d, v), (-h, 0), (h, 0)], POP_MIX[k]);
        }
    }
}

/// Poison: bubbles forming and popping on the body, drawn over everything
/// but the eyes. `cx`/`cy`: body centre in design units.
pub(super) fn draw_bubbles(cv: &mut Canvas, ctx: &Ctx, cx: f32, cy: f32, bright: f32) {
    let light = mul(lerp(ramp(ctx.elem)[3], WHITE, 0.45), bright);
    for (px, py, b) in bubble_spots(cv, ctx, cx, cy) {
        draw_bubble(cv, px, py, &b, light);
    }
}

/// Element decorations around the body: orbiting droplets and floor ripples
/// (water). Poison bubbles are drawn later, over the magic (see [`draw_bubbles`]).
/// `cy`: body centre y in design units (after bobbing and the casting pose).
pub(super) fn decorate(cv: &mut Canvas, ctx: &Ctx, cy: f32) {
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
