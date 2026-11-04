//! Elements: palette, outline shape, decorations and particles per theme.

use std::f32::consts::{PI, TAU};

use crate::model::Status;

use super::canvas::{lerp, Canvas, Rgb, CARD_BG};
use super::noise::{hf, seed_phase, slot, Rng};
use super::particles::Particle;
use super::state::twinkle;
use super::theme::Theme;
use super::{Ctx, Magic, BASE_R, CX, DH, DW};

/// Colour ramp: rim -> dark -> mid -> light.
pub(super) fn ramp(e: Theme) -> [Rgb; 4] {
    match e {
        Theme::Fire => [[150., 25., 12.], [225., 65., 18.], [255., 140., 30.], [255., 215., 80.]],
        Theme::Water => [[25., 55., 150.], [35., 105., 215.], [70., 165., 245.], [150., 215., 255.]],
        Theme::Ice => [[80., 145., 190.], [135., 200., 232.], [190., 235., 250.], [240., 252., 255.]],
        Theme::Lightning => [[140., 100., 0.], [215., 170., 0.], [255., 220., 35.], [255., 250., 165.]],
    }
}

/// Ice: facet count and rotation.
pub(super) fn ice_facets(seed: u32, tf: f32, sp: f32) -> (f32, f32) {
    (7.0, seed_phase(seed, 0x1CE) + tf * 0.004 * sp)
}

/// Lightning: rotation of the spike star.
pub(super) fn bolt_rot(ctx: &Ctx) -> f32 {
    seed_phase(ctx.seed, 0xB01) + ctx.tf * 0.01 * ctx.sp
}

/// Ice: twinkle phase of frost star `i`.
pub(super) fn star_phase(ctx: &Ctx, i: usize) -> f32 {
    (ctx.tf * 0.08 + i as f32 * 1.7 + seed_phase(ctx.seed, 0x57A)).sin()
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
            let flick = (2.6 + 1.2 * (tf * 0.37 * sp + th * 4.0).sin()) * if ctx.small { 1.8 } else { 1.0 };
            BASE_R + wob * 0.7 + up.powf(1.6) * tongue * flick
        }
        Theme::Water => BASE_R + wob * 0.4 + sn.max(0.0).powi(2) * 0.6,
        Theme::Ice => {
            let (n, rot) = ice_facets(ctx.seed, tf, sp);
            let seg = TAU / n;
            let a = (th - rot).rem_euclid(seg) - seg / 2.0;
            9.6 * (PI / n).cos() / a.cos() + wob * 0.15
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
        Theme::Fire if tone == 3 && hf(rx as u32 * 31 + ry as u32, ((t / 2) as u32).wrapping_add(ctx.seed)) < 0.12 => [255., 245., 170.],
        _ => c,
    }
}

/// Element decorations around the body: orbiting droplets and floor ripples
/// (water), twinkling frost stars (ice).
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
        Theme::Ice => {
            let stars = [(1.5, 4.0), (21.0, 10.0), (2.5, 22.0), (20.0, 23.0)];
            for (i, &(x, y)) in stars.iter().enumerate() {
                let ph = star_phase(ctx, i);
                twinkle(cv, x, y, ph, [230., 250., 255.]);
            }
        }
        _ => {}
    }
}

/// How a magic level scales a cast. `Normal` is exactly 1 everywhere, so the
/// approved look (and its random stream) is unchanged.
#[derive(Clone, Copy)]
pub(super) struct Scale {
    /// particles per burst
    pub count: f32,
    /// particle life (so: range)
    pub life: f32,
    /// launch speed (so: range)
    pub speed: f32,
    /// particle size: -1 = always 1 dot block, 0 = as designed, 1 = always big
    pub grow: i8,
}

pub(super) fn scale(m: Magic) -> Scale {
    match m {
        Magic::Small => Scale { count: 0.5, life: 0.5, speed: 0.8, grow: -1 },
        Magic::Normal => Scale { count: 1.0, life: 1.0, speed: 1.0, grow: 0 },
        Magic::Big => Scale { count: 1.6, life: 1.4, speed: 1.3, grow: 1 },
    }
}

impl Scale {
    fn n(self, n: u32) -> u32 {
        ((n as f32 * self.count).round() as u32).max(1)
    }

    fn life(self, l: u32) -> u32 {
        ((l as f32 * self.life).round() as u32).max(1)
    }

    fn size(self, s: u32) -> u32 {
        match self.grow {
            -1 => 1,
            1 => 2,
            _ => s,
        }
    }
}

/// Longest particle life of an element at a magic level, in ticks.
pub(super) fn max_life_for(e: Theme, m: Magic) -> u64 {
    (max_life(e) as f32 * scale(m).life.max(1.0)).ceil() as u64
}

/// Longest particle life of an element, in ticks.
pub(super) fn max_life(e: Theme) -> u64 {
    match e {
        Theme::Fire => 22,
        Theme::Water => 40,
        Theme::Ice => 220,
        Theme::Lightning => 4,
    }
}

/// Ambient particles for one tick: embers, drips, falling snow, sparks and arcs.
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
        Theme::Ice => {
            if r.chance(0.06 * act) {
                let x = r.range(0.0, DW - 2.0);
                let (vx, vy) = (r.range(-0.05, 0.05), r.range(0.06, 0.14));
                let mut p = Particle::new(x, -1.0, vx, vy, 220, [205., 238., 252.], [170., 220., 245.]);
                p.behind = true;
                parts.push(p);
            }
        }
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

/// RUNNING: a burst of element particles shot to the right, scaled by the
/// magic level (`right` is the canvas edge in design units).
pub(super) fn cast(e: Theme, m: Magic, right: f32, cy: f32, r: &mut Rng, parts: &mut Vec<Particle>) {
    let k = scale(m);
    match e {
        Theme::Fire => {
            for _ in 0..k.n(r.int(3, 5)) {
                let y = cy + r.range(-2.5, 1.5);
                let (vx, vy) = (r.range(0.5, 0.9) * k.speed, r.range(-0.15, 0.03));
                let life = k.life(r.int(12, 20));
                let mut p = Particle::new(CX + BASE_R - 1.0, y, vx, vy, life, [255., 235., 120.], [185., 30., 10.]);
                p.size = k.size(2);
                parts.push(p);
            }
        }
        Theme::Water => {
            for _ in 0..k.n(r.int(4, 6)) {
                let y = cy + r.range(-2.0, 1.0);
                let (vx, vy) = (r.range(0.6, 1.0) * k.speed, r.range(-0.35, -0.1));
                let mut p = Particle::new(CX + BASE_R - 0.5, y, vx, vy, k.life(20), [175., 228., 255.], [40., 110., 220.]);
                // keep the arc's height: gravity scales with the life change
                p.ay = 0.035 / (k.life * k.life);
                p.vy /= k.life;
                p.size = k.size(if r.chance(0.3) { 2 } else { 1 });
                parts.push(p);
            }
        }
        Theme::Ice => {
            for _ in 0..k.n(r.int(3, 5)) {
                let y = cy + r.range(-3.0, 2.0);
                let (vx, vy) = (r.range(0.9, 1.3) * k.speed, r.range(-0.05, 0.05));
                let mut p = Particle::new(CX + BASE_R, y, vx, vy, k.life(18), [245., 255., 255.], [110., 190., 230.]);
                p.ax = -0.01;
                p.wide = k.grow >= 0;
                p.size = k.size(1);
                parts.push(p);
            }
        }
        Theme::Lightning => {
            let mut y = cy;
            let mut x = CX + BASE_R - 0.5;
            // the bolt's length is the magic's range
            let end = match m {
                Magic::Small => CX + BASE_R + 6.0,
                _ => right - 1.0,
            };
            while x < end {
                y = (y + r.int(0, 2) as f32 - 1.0).clamp(cy - 6.0, cy + 6.0);
                let mut p = Particle::new(x, y, 0.0, 0.0, k.life(3), [255., 255., 240.], [255., 215., 60.]);
                p.prio = 2;
                p.wide = k.grow > 0;
                parts.push(p);
                x += 1.0;
            }
            for _ in 0..k.n(3) {
                let (vx, vy) = (r.range(-0.4, 0.2) * k.speed, r.range(-0.4, 0.4) * k.speed);
                parts.push(Particle::new(x - 1.0, y, vx, vy, k.life(4), [255., 255., 220.], [255., 190., 30.]));
            }
        }
    }
}

/// Whether a particle is still alive and on the design canvas.
pub(super) fn keep(p: &Particle, right: f32) -> bool {
    p.life > 0 && p.x > -2.0 && p.x < right + 1.0 && p.y > -3.0 && p.y < DH + 1.0 && p.y < p.floor
}
