//! RUNNING magic: what a spirit casts to the right of its body, scaled by
//! the magic level. Pure in (seed, tick): the cast schedule comes from
//! `state::casts_at` and every shape from stateless noise.
//!
//! - fire: a continuous breath of flame with embers and a little smoke
//! - water: a spinning spiral vortex
//! - poison: a creeping toxic fog
//! - lightning: a zig-zag bolt striking down with a spark burst

use std::f32::consts::PI;

use crate::model::Status;

use super::canvas::{lerp, Canvas, Rgb, CARD_BG};
use super::noise::{hf, seed_phase, slot};
use super::theme::Theme;
use super::{state, Ctx, Magic, BASE_R, CY};

/// A stream (breath, fog) holds this long after a cast. Casts come at most
/// 39 ticks apart, so a running spirit's stream never breaks.
const HOLD: u64 = 44;
/// How far back the first cast of a stream is searched (its ramp-up is shorter).
const LOOKBACK: u64 = 64;

/// Ticks since the latest cast and since the first cast of the unbroken
/// stream it belongs to (capped by [`LOOKBACK`]); None when nothing was cast
/// within [`HOLD`] ticks.
fn stream(seed: u32, t: u64) -> Option<(u64, u64)> {
    let lo = t.saturating_sub(LOOKBACK).max(1);
    let (mut last, mut head) = (None, t);
    let mut u = t;
    while u >= lo {
        if state::casts_at(seed, u) {
            if last.is_none() {
                last = Some(u);
            } else if head - u > HOLD {
                break;
            }
            head = u;
        } else if last.is_none() && t - u >= HOLD {
            return None;
        }
        u -= 1;
    }
    last.map(|l| (t - l, t - head))
}

/// Where the magic goes, in real sub-pixels: the mouth (`ox`, `oy`) at the
/// body's right edge and the right end of the reach (exclusive).
struct Area {
    ox: f32,
    oy: f32,
    end: f32,
}

impl Area {
    fn new(cv: &Canvas, m: Magic, cy: f32) -> Self {
        let g = cv.g;
        let ox = g.rcx + (BASE_R - 0.5) * g.ppu;
        let end = match m {
            Magic::Small => ox + 3.5,
            _ => g.w as f32,
        };
        Area { ox, oy: g.rcy + (cy - CY) * g.ppu, end: end.min(g.w as f32) }
    }

    /// Paints one magic dot, never on the body and never past the reach.
    fn dot(&self, cv: &mut Canvas, x: i32, y: i32, c: Rgb, prio: u8) {
        if (x as f32) < self.end {
            cv.set_px(x, y, c, prio, true);
        }
    }
}

pub(super) fn draw(cv: &mut Canvas, ctx: &Ctx, cy: f32) {
    if ctx.state != Status::Running {
        return;
    }
    let Some((since_cast, since_start)) = stream(ctx.seed, ctx.t) else { return };
    let a = Area::new(cv, ctx.magic, cy);
    match ctx.elem {
        Theme::Fire => breath(cv, ctx, &a, since_cast, since_start),
        Theme::Water => vortex(cv, ctx, &a, since_cast),
        Theme::Poison => fog(cv, ctx, &a, since_start),
        Theme::Lightning => bolt(cv, ctx, &a, since_cast),
    }
}

/// Fire: a dense jet of flame from the mouth that widens, flickers and
/// breaks up toward its end, with embers flying off and a little smoke.
fn breath(cv: &mut Canvas, ctx: &Ctx, a: &Area, since_cast: u64, since_start: u64) {
    let (w_end, embers, smokes) = match ctx.magic {
        Magic::Small => (1.4, 0, 0),
        Magic::Normal => (3.0, 3, 2),
        Magic::Big => (4.6, 7, 4),
    };
    // the first exhale grows in a few ticks; every cast roars a little
    let grow = ((since_start as f32 + 1.0) / 5.0).min(1.0);
    let roar = 1.0 + 0.12 * (-(since_cast as f32) / 3.0).exp();
    let flick = 1.0 + 0.05 * (ctx.tf * 0.9 + seed_phase(ctx.seed, 0xF1E)).sin();
    let full = a.end - a.ox;
    let len = (full * 0.93 * grow * roar * flick).clamp(1.0, full);
    let ph = seed_phase(ctx.seed, 0xB4E);
    let u_at = |x: f32| ((x - a.ox) / len).clamp(0.0, 1.0);
    // the jet waves more and rises toward its end
    let yc = |x: f32| {
        let u = u_at(x);
        a.oy - 0.5 + u * 0.9 * (x * 0.55 - ctx.tf * 0.8 + ph).sin() - u * u
    };
    let hw = |u: f32| 1.2 + (w_end - 1.2) * u;

    for k in 0..smokes {
        let key = ctx.seed ^ (k as u32).wrapping_mul(0x5A0C);
        let period = 14;
        let shifted = ctx.t + slot(key, 0x5A1, 0, period as u32);
        let (cycle, age) = (shifted / period, shifted % period);
        if age > since_start {
            continue;
        }
        let sx = a.ox + len * (0.65 + 0.35 * hf(key, cycle as u32));
        let sy = yc(sx) - hw(1.0);
        let af = age as f32;
        let c = lerp([110., 98., 94.], CARD_BG, af / period as f32);
        a.dot(cv, (sx + 0.3 * af).floor() as i32, (sy - 0.45 * af).floor() as i32, c, 1);
    }

    for x in a.ox.floor() as i32..(a.ox + len).ceil() as i32 {
        let xf = x as f32 + 0.5;
        let u = u_at(xf);
        let (c, w) = (yc(xf), hw(u));
        for y in (c - w).floor() as i32..=(c + w).ceil() as i32 {
            let dy = ((y as f32 + 0.5 - c) / w).abs();
            if dy > 1.0 {
                continue;
            }
            let n = hf((x as u32).wrapping_mul(31).wrapping_add((y as u32).wrapping_mul(7)).wrapping_add(ctx.seed), ctx.t as u32);
            let solid = dy < 0.75 - 0.25 * u;
            let ragged = n < 0.6 - 0.3 * u;
            // the end breaks up into tongues
            let tip = u < 0.8 || n > (u - 0.8) * 2.5;
            if !(solid || ragged) || !tip {
                continue;
            }
            let heat = (1.0 - dy) * (1.0 - 0.55 * u);
            let col = if heat > 0.62 {
                [255., 242., 168.]
            } else if heat > 0.4 {
                [255., 205., 65.]
            } else if heat > 0.2 {
                [255., 135., 30.]
            } else {
                [215., 52., 20.]
            };
            a.dot(cv, x, y, col, 1);
        }
    }

    for k in 0..embers as u32 {
        let key = ctx.seed ^ k.wrapping_mul(0x2F1B);
        let period = 8 + u64::from(k % 3);
        let shifted = ctx.t + slot(key, 0xE1, 0, period as u32);
        let (cycle, age) = (shifted / period, shifted % period);
        if age > since_start {
            continue;
        }
        let cyc = cycle as u32;
        let sx = a.ox + len * (0.45 + 0.45 * hf(key, cyc));
        let sy = yc(sx) + (hf(key ^ 1, cyc) - 0.5) * 2.0 * hw(0.7);
        let af = age as f32;
        let c = lerp([255., 238., 135.], [205., 60., 20.], af / period as f32);
        a.dot(cv, (sx + 0.8 * af).floor() as i32, (sy - 0.35 * af - 0.04 * af * af).floor() as i32, c, 2);
    }
}

/// Water: a two-armed spiral of dots spinning to the right of the body;
/// it opens after each cast and closes again.
fn vortex(cv: &mut Canvas, ctx: &Ctx, a: &Area, since_cast: u64) {
    let (dur, dx, r): (u64, f32, f32) = match ctx.magic {
        Magic::Small => (14, 1.9, 1.7),
        Magic::Normal => (22, 4.8, 4.2),
        Magic::Big => (30, 11.5, 7.0),
    };
    if since_cast >= dur {
        return;
    }
    let sc = since_cast as f32;
    let open = ((sc + 1.0) / 4.0).min(1.0) * ((dur as f32 - sc) / 4.0).min(1.0);
    let rr = r * open;
    let (cx, cy) = (a.ox + dx, a.oy);
    let rot = ctx.tf * 0.35 + seed_phase(ctx.seed, 0x5B1);
    let steps = ((rr * 6.0).ceil() as i32).max(3);
    for arm in 0..2 {
        for i in 0..=steps {
            let s = 0.15 + 0.85 * i as f32 / steps as f32;
            let ang = arm as f32 * PI + s * 2.4 * PI - rot;
            let (x, y) = (cx + rr * s * ang.cos(), cy + rr * s * 1.1 * ang.sin());
            let c = if s < 0.35 {
                [215., 245., 255.]
            } else if s < 0.7 {
                [95., 180., 250.]
            } else {
                [45., 110., 220.]
            };
            a.dot(cv, x.floor() as i32, y.floor() as i32, c, 2);
        }
    }
    a.dot(cv, cx.floor() as i32, cy.floor() as i32, [235., 250., 255.], 2);
}

/// Poison: a soft, wavy cloud of dim-to-bright green dots creeping slowly
/// to the right; the dot pattern drifts with the cloud instead of
/// re-rolling every tick.
fn fog(cv: &mut Canvas, ctx: &Ctx, a: &Area, since_start: u64) {
    let (half, thick) = match ctx.magic {
        Magic::Small => (2.2, 0.85),
        Magic::Normal => (5.0, 1.15),
        Magic::Big => (8.0, 1.4),
    };
    let grow = ((since_start as f32 + 1.0) / 10.0).min(1.0);
    let full = a.end - a.ox;
    let len = full * grow;
    let drift = ctx.tf * 0.1;
    let shift = drift.floor() as i32;
    let ph = seed_phase(ctx.seed, 0xF06);
    for x in a.ox.floor() as i32..(a.ox + len).ceil() as i32 {
        let xf = x as f32 + 0.5;
        let u = ((xf - a.ox) / full).clamp(0.0, 1.0);
        let ex = 1.0 - 0.5 * u * u;
        let yc = a.oy + 1.2 * (xf * 0.3 - ctx.tf * 0.04 + ph).sin();
        for y in 0..cv.g.h as i32 {
            let yf = y as f32 + 0.5;
            let ey = 1.0 - ((yf - yc) / half).powi(2);
            if ey <= 0.0 {
                continue;
            }
            let swirl = 1.4 * (yf * 0.45 + ctx.tf * 0.03 + ph).sin();
            let cloud = 0.55 + 0.45 * ((xf - drift) * 0.5 + swirl).sin();
            let dens = (cloud * ex * ey.sqrt() * thick).min(1.0);
            let q = hf(((x - shift) as u32).wrapping_mul(131).wrapping_add((y as u32).wrapping_mul(17)), ctx.seed);
            if dens < q * 0.85 + 0.15 {
                continue;
            }
            a.dot(cv, x, y, lerp([48., 105., 52.], [165., 250., 115.], dens), 1);
        }
    }
}

/// Lightning: a zig-zag bolt from the top of the area to the bottom with a
/// spark burst where it strikes; every cast strikes a new path.
fn bolt(cv: &mut Canvas, ctx: &Ctx, a: &Area, since_cast: u64) {
    let (dur, dx, amp, burst, wide): (u64, f32, f32, i32, bool) = match ctx.magic {
        Magic::Small => (3, 2.3, 0.8, 1, false),
        Magic::Normal => (5, 6.0, 2.2, 2, false),
        Magic::Big => (7, 13.0, 3.4, 3, true),
    };
    if since_cast >= dur {
        return;
    }
    let cast = ctx.t - since_cast;
    let ck = (cast as u32) ^ ((cast >> 32) as u32);
    let key = ctx.seed ^ 0xB017;
    let bottom = cv.g.h as f32 - 1.0;
    let mid = a.ox + dx;
    let mut dir = if hf(key, ck) < 0.5 { -1.0 } else { 1.0 };
    let mut pts = vec![(mid + (hf(key ^ 1, ck) - 0.5) * amp, 0.0)];
    let mut y = 0.0;
    let mut i = 0u32;
    while y < bottom {
        i += 1;
        let n = ck.wrapping_mul(7).wrapping_add(i);
        y = (y + 2.0 + (hf(key ^ 2, n) * 2.0).floor()).min(bottom);
        pts.push((mid + dir * amp * (0.5 + 0.5 * hf(key ^ 3, n)), y));
        dir = -dir;
    }
    let (xb, yb) = *pts.last().unwrap_or(&(mid, bottom));

    // the bolt itself flickers: on, on, off, on (big: once more)
    let shown = matches!(since_cast, 0 | 1 | 3) || (wide && since_cast == 5);
    if shown {
        for w in pts.windows(2) {
            let ((x0, y0), (x1, y1)) = (w[0], w[1]);
            let steps = (x1 - x0).abs().max(y1 - y0).ceil().max(1.0) as i32;
            for s in 0..=steps {
                let f = s as f32 / steps as f32;
                let (x, y) = ((x0 + (x1 - x0) * f).round() as i32, (y0 + (y1 - y0) * f).round() as i32);
                a.dot(cv, x, y, [255., 255., 235.], 2);
                if wide {
                    a.dot(cv, x + 1, y, [255., 225., 90.], 2);
                }
            }
        }
    }

    // spark burst at the impact, spreading and fading
    let fade = since_cast as f32 / dur as f32;
    let reach = burst.min(since_cast as i32 + 1);
    let rays: [(i32, i32); 7] = [(-1, 0), (1, 0), (-1, -1), (1, -1), (0, -1), (-2, -1), (2, -1)];
    let (bx, by) = (xb.round() as i32, yb.round() as i32);
    for (rx, ry) in rays {
        for k in 1..=reach {
            let c = lerp(lerp([255., 250., 200.], [255., 170., 30.], k as f32 / (burst as f32 + 1.0)), CARD_BG, fade * 0.7);
            a.dot(cv, bx + rx * k, by + ry * k, c, 2);
        }
    }
}
