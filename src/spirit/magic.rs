//! RUNNING magic: what a spirit casts to the right of its body, scaled by
//! the magic level. Pure in (seed, tick): the cast schedule comes from
//! `state::casts_at` and every shape from stateless noise.
//!
//! - fire: a breath of flame that leaves the mouth one dot column away from
//!   the body, starts as a single dot and widens into a cone that flickers
//!   and breaks up into embers and smoke
//! - water: a glossy, shaded whirlpool disc seen at an angle, lit from the
//!   upper left, with spiral arms, droplets and foam
//! - poison: toxic puffs spreading out radially from a burst centre
//! - lightning: zig-zag bolts striking down with spark bursts; more and
//!   thicker bolts at higher magic levels

use std::f32::consts::TAU;

use crate::model::Status;

use super::canvas::{lerp, Canvas, Geom, Rgb, CARD_BG};
use super::noise::{hf, seed_phase, slot};
use super::theme::Theme;
use super::{state, Ctx, Magic, BASE_R, CY};

/// A stream (breath) holds this long after a cast. Casts come at most 39
/// ticks apart, so a running spirit's stream never breaks.
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

/// The mouth: the body's right edge in real sub-pixels.
fn mouth_x(g: &Geom) -> f32 {
    g.rcx + (BASE_R - 0.5) * g.ppu
}

/// Where the magic goes, in real sub-pixels: the mouth (`ox`, `oy`) at the
/// body's right edge and the right end of the reach (exclusive).
struct Area {
    ox: f32,
    oy: f32,
    end: f32,
}

impl Area {
    fn new(g: &Geom, m: Magic, elem: Theme, cy: f32) -> Self {
        let ox = mouth_x(g);
        let end = match m {
            Magic::Small if elem == Theme::Lightning => ox + 7.0,
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

/// `cy`: body centre y in design units (bobbing, without the casting pose).
pub(super) fn draw(cv: &mut Canvas, ctx: &Ctx, cy: f32) {
    if ctx.state != Status::Running {
        return;
    }
    let Some((since_cast, since_start)) = stream(ctx.seed, ctx.t) else { return };
    let a = Area::new(&cv.g, ctx.magic, ctx.elem, cy);
    match ctx.elem {
        Theme::Fire => breath(cv, ctx, &a, since_cast, since_start),
        Theme::Water => vortex(cv, ctx, &a, since_cast),
        Theme::Poison => fog(cv, ctx, &a),
        Theme::Lightning => bolt(cv, ctx, &a, since_cast),
    }
}

/// Fire: a breath of flame from the mouth. One empty dot column separates
/// it from the body; it starts as a single dot and widens with distance into
/// a cone, densest along its middle, flickering and breaking up at the end
/// with embers flying off and a little smoke.
fn breath(cv: &mut Canvas, ctx: &Ctx, a: &Area, since_cast: u64, since_start: u64) {
    let (w_end, embers, smokes) = match ctx.magic {
        Magic::Small => (1.0, 0, 0),
        Magic::Normal => (3.0, 3, 2),
        Magic::Big => (4.6, 7, 4),
    };
    // the first flame column: one empty dot column right of the body
    let edge = cv.body_right().unwrap_or(a.ox.floor() as i32);
    let xs = (edge + 2) as f32;
    let full = a.end - xs;
    if full < 1.0 {
        return;
    }
    // the first exhale grows in a few ticks; every cast roars a little
    let grow = ((since_start as f32 + 1.0) / 5.0).min(1.0);
    let roar = 1.0 + 0.12 * (-(since_cast as f32) / 3.0).exp();
    let flick = 1.0 + 0.05 * (ctx.tf * 0.9 + seed_phase(ctx.seed, 0xF1E)).sin();
    let len = (full * 0.97 * grow * roar * flick).clamp(1.0, full);
    let ph = seed_phase(ctx.seed, 0xB4E);
    let u_at = |x: f32| ((x - xs) / len).clamp(0.0, 1.0);
    // straight out of the mouth, waving more and rising toward the end
    let yc = |x: f32| {
        let u = u_at(x);
        a.oy - 0.5 + u * 0.9 * (x * 0.55 - ctx.tf * 0.8 + ph).sin() - u * u
    };
    // half height of the cone at distance `d` (dots) from its first column
    let hw = |d: f32| (0.1 + 0.42 * d).min(w_end);

    for k in 0..smokes {
        let key = ctx.seed ^ (k as u32).wrapping_mul(0x5A0C);
        let period = 14;
        let shifted = ctx.t + slot(key, 0x5A1, 0, period as u32);
        let (cycle, age) = (shifted / period, shifted % period);
        if age > since_start {
            continue;
        }
        let sx = xs + len * (0.65 + 0.35 * hf(key, cycle as u32));
        let sy = yc(sx) - hw(sx - xs);
        let af = age as f32;
        let c = lerp([110., 98., 94.], CARD_BG, af / period as f32);
        a.dot(cv, (sx + 0.3 * af).floor() as i32, (sy - 0.45 * af).floor() as i32, c, 1);
    }

    for x in xs as i32..(xs + len).ceil() as i32 {
        let xf = x as f32 + 0.5;
        let (u, c) = (u_at(xf), yc(xf));
        let w = hw(xf - xs);
        let core = c.floor() as i32;
        for y in (c - w).floor() as i32..=(c + w).ceil() as i32 {
            let dy = (y as f32 + 0.5 - c).abs() / w.max(0.3);
            let centre = y == core && u < 0.78;
            if dy > 1.0 && !centre {
                continue;
            }
            let dy = dy.min(1.0);
            let n = hf((x as u32).wrapping_mul(31).wrapping_add((y as u32).wrapping_mul(7)).wrapping_add(ctx.seed), ctx.t as u32);
            // densest along the middle of the jet, ragged at its edges
            let dens = (0.5 + 0.45 * (std::f32::consts::PI * u).sin()) * (1.0 - 0.55 * dy) + 0.25 * (1.0 - dy);
            // the end breaks up into tongues
            let tip = u < 0.78 || n > (u - 0.78) * 2.6;
            if !centre && (!tip || n > dens) {
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
        let sx = xs + len * (0.5 + 0.45 * hf(key, cyc));
        let sy = yc(sx) + (hf(key ^ 1, cyc) - 0.5) * 2.0 * hw(sx - xs);
        let af = age as f32;
        let c = lerp([255., 238., 135.], [205., 60., 20.], af / period as f32);
        a.dot(cv, (sx + 0.8 * af).floor() as i32, (sy - 0.35 * af - 0.04 * af * af).floor() as i32, c, 2);
    }
}

/// Angle of the water disc's long axis (screen coordinates, up and to the
/// right): the whirlpool is a disc seen at an angle.
const DISC_TILT: f32 = 1.15;
/// Short over long axis of the disc (the perspective squash).
const DISC_SQUASH: f32 = 0.76;

/// Water: a glossy, translucent whirlpool. A dark deep-blue core, a sparse
/// mid-blue body, spiral arms that brighten toward their tips, a lighter rim
/// and a bright specular highlight from the upper left (it stays put while
/// the arms spin), with droplets trailing the arm tips and foam sparkles.
/// It opens after each cast and closes again.
fn vortex(cv: &mut Canvas, ctx: &Ctx, a: &Area, since_cast: u64) {
    let (dur, dx, major, arms): (u64, f32, f32, u32) = match ctx.magic {
        Magic::Small => (14, 1.9, 1.9, 1),
        Magic::Normal => (22, 4.8, 5.0, 2),
        Magic::Big => (30, 11.5, 7.6, 3),
    };
    if since_cast >= dur {
        return;
    }
    let sc = since_cast as f32;
    let open = ((sc + 1.0) / 4.0).min(1.0) * ((dur as f32 - sc) / 4.0).min(1.0);
    let (ra, rb) = ((major * open).max(0.8), (major * DISC_SQUASH * open).max(0.6));
    let (cx, cy) = (a.ox + dx, a.oy);
    let (ca, sa) = (DISC_TILT.cos(), DISC_TILT.sin());
    // screen half extents of the tilted disc (normalise the highlight spot)
    let hx = (ra * ra * ca * ca + rb * rb * sa * sa).sqrt();
    let hy = (ra * ra * sa * sa + rb * rb * ca * ca).sqrt();
    let rot = ctx.tf * 0.35 + seed_phase(ctx.seed, 0x5B1);
    let twist = 2.6;
    let arms_f = arms as f32;
    // disc coordinates (along the long axis, along the short axis) -> screen offset
    let to_screen = |p: f32, q: f32| (p * ra * ca + q * rb * sa, -p * ra * sa + q * rb * ca);

    for y in (cy - hy - 1.0).floor() as i32..=(cy + hy + 1.0).ceil() as i32 {
        for x in (cx - hx - 1.0).floor() as i32..=(cx + hx + 1.0).ceil() as i32 {
            let (px, py) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
            let p = (px * ca - py * sa) / ra;
            let q = (px * sa + py * ca) / rb;
            let rho = p.hypot(q);
            if rho > 1.0 {
                continue;
            }
            let phi = q.atan2(p);
            let (nx, ny) = (px / hx, py / hy);
            let spec = (nx + 0.36).powi(2) + (ny + 0.4).powi(2) < 0.1;
            let s = ((phi + twist * rho - rot) * arms_f / TAU).rem_euclid(1.0);
            let on_arm = s < 0.2 + 0.14 * rho;
            let n = hf((x as u32).wrapping_mul(97).wrapping_add((y as u32).wrapping_mul(13)), ctx.seed);
            let (c, paint) = if spec {
                ([228., 248., 255.], true)
            } else if rho > 0.86 {
                ([78., 150., 232.], n < 0.7)
            } else if on_arm && rho > 0.2 {
                (lerp([30., 85., 190.], [125., 200., 250.], (rho - 0.2) / 0.66), true)
            } else if rho < 0.34 {
                ([18., 42., 125.], n < 0.85)
            } else {
                ([32., 88., 185.], n < 0.4)
            };
            // the highlight wins its cell (prio 2), so it stays a clean glint
            if paint {
                a.dot(cv, x, y, c, if spec { 2 } else { 1 });
            }
        }
    }

    if !matches!(ctx.magic, Magic::Small) {
        for k in 0..arms {
            // the arm tip (s = 0 at rho = 1) and droplets trailing it
            let tip = rot + k as f32 * TAU / arms_f - twist;
            for j in 1..=2 {
                let (ang, rho) = (tip - 0.32 * j as f32, 1.1 + 0.16 * j as f32);
                let (ox, oy) = to_screen(rho * ang.cos(), rho * ang.sin());
                let c = lerp([150., 220., 255.], [60., 130., 220.], j as f32 / 2.0);
                a.dot(cv, (cx + ox).floor() as i32, (cy + oy).floor() as i32, c, 1);
            }
        }
        // an occasional sparkle of foam on the rim
        let slot_t = (ctx.t / 2) as u32;
        if hf(ctx.seed ^ 0xF0A, slot_t) < 0.4 {
            let ang = hf(ctx.seed ^ 0xF0B, slot_t) * TAU;
            let (ox, oy) = to_screen(0.95 * ang.cos(), 0.95 * ang.sin());
            a.dot(cv, (cx + ox).floor() as i32, (cy + oy).floor() as i32, [200., 232., 250.], 1);
        }
    }
}

/// Poison burst centre in real sub-pixels: right of the mouth at the body
/// centre's height, farther out for stronger magic.
pub(super) fn fog_anchor(g: &Geom, m: Magic) -> (f32, f32) {
    let d = match m {
        Magic::Small => 2.0,
        Magic::Normal => 5.0,
        Magic::Big => 9.5,
    };
    (mouth_x(g) + d, g.rcy)
}

/// Ticks one cast's toxic puffs live.
pub(super) fn fog_life(m: Magic) -> u64 {
    match m {
        Magic::Small => 16,
        Magic::Normal => 26,
        Magic::Big => 34,
    }
}

/// Poison: every cast bursts toxic puffs out of a fixed centre right of the
/// mouth. They swell radially, thinning and fading as they grow, with
/// edges wobbling in smooth noise; Normal and Big casts puff several
/// overlapping clouds. The dither pattern is fixed, so the fog creeps
/// instead of flickering.
fn fog(cv: &mut Canvas, ctx: &Ctx, a: &Area) {
    let (rmax, puffs, spread, thick): (f32, u32, f32, f32) = match ctx.magic {
        Magic::Small => (2.2, 1, 0.0, 0.95),
        Magic::Normal => (6.5, 2, 1.6, 1.15),
        Magic::Big => (9.5, 3, 2.6, 1.2),
    };
    let life = fog_life(ctx.magic);
    let (ax, ay) = fog_anchor(&cv.g, ctx.magic);
    // (centre x, centre y, radius, strength, wobble phase) of every live puff
    let mut live: Vec<(f32, f32, f32, f32, f32)> = Vec::new();
    for age in 0..life {
        let Some(c) = ctx.t.checked_sub(age) else { break };
        if c == 0 || !state::casts_at(ctx.seed, c) {
            continue;
        }
        let ck = c as u32 ^ (c >> 32) as u32;
        for k in 0..puffs {
            let delay = 3 * u64::from(k);
            if age < delay {
                continue;
            }
            let g = (age - delay) as f32 / (life - delay) as f32;
            let (ox, oy) = if k == 0 {
                (0.0, 0.0)
            } else {
                let ang = hf(ctx.seed ^ 0xF07 ^ k, ck) * TAU;
                (spread * ang.cos(), spread * 0.7 * ang.sin())
            };
            let r = rmax * (1.0 - 0.18 * k as f32) * (0.05 + 0.95 * g.powf(0.7));
            let amp = thick * (1.0 - g).powf(0.6) * (1.0 + 0.5 * (1.0 - g));
            live.push((ax + ox, ay + oy, r, amp, hf(ctx.seed ^ 0xF08 ^ k, ck) * TAU));
        }
    }
    if live.is_empty() {
        return;
    }
    let reach = rmax * 1.3 + spread;
    // like the breath, the fog keeps one empty dot column off the body
    let x0 = ((ax - reach).floor() as i32).max(cv.body_right().map_or(0, |e| e + 2));
    for y in 0..cv.g.h as i32 {
        for x in x0..(ax + reach).ceil() as i32 {
            let (xf, yf) = (x as f32 + 0.5, y as f32 + 0.5);
            let mut dens = 0.0f32;
            for &(px, py, r, amp, ph) in &live {
                let (dx, dy) = (xf - px, yf - py);
                let th = dy.atan2(dx);
                let wob = 1.0 + 0.16 * (3.0 * th + ph + ctx.tf * 0.05).sin() + 0.08 * (5.0 * th - ctx.tf * 0.035 + 2.0 * ph).sin();
                let d = dx.hypot(dy) / (r * wob);
                if d < 1.0 {
                    dens = dens.max(amp * (1.0 - d * d).powf(0.6));
                }
            }
            let q = hf((x as u32).wrapping_mul(131).wrapping_add((y as u32).wrapping_mul(17)), ctx.seed);
            if dens < q * 0.8 + 0.12 {
                continue;
            }
            a.dot(cv, x, y, lerp([48., 105., 52.], [165., 250., 115.], dens), 1);
        }
    }
}

/// Lightning layout per magic level: (flicker duration, main bolt distance
/// from the mouth, zig-zag amplitude, spark burst reach, bolt width in dots).
fn bolt_style(m: Magic) -> (u64, f32, f32, i32, i32) {
    match m {
        Magic::Small => (3, 5.5, 0.8, 1, 1),
        Magic::Normal => (5, 7.5, 2.2, 2, 2),
        Magic::Big => (7, 13.0, 3.4, 4, 3),
    }
}

/// One bolt: its zig-zag points from the top row to the bottom row and
/// whether it ends in a spark burst (forks do not).
struct Bolt {
    pts: Vec<(f32, f32)>,
    sparks: bool,
}

/// The bolts of the cast at `cast`: the main bolt first (the same path at
/// every level), then the extra bolts of higher levels at distinct x, and
/// at Big sometimes a fork off the main bolt.
fn bolts(ctx: &Ctx, ox: f32, bottom: f32, cast: u64) -> Vec<Bolt> {
    let (_, dx, amp, _, _) = bolt_style(ctx.magic);
    let ck = (cast as u32) ^ ((cast >> 32) as u32);
    let key = ctx.seed ^ 0xB017;
    // a zig-zag from (top x around `mid`) down to the bottom; `last` forces the final swing
    let path = |key: u32, mid: f32, amp: f32, last: Option<f32>| {
        let mut dir = if hf(key, ck) < 0.5 { -1.0 } else { 1.0 };
        let mut pts = vec![(mid + (hf(key ^ 1, ck) - 0.5) * amp, 0.0)];
        let (mut y, mut i) = (0.0, 0u32);
        while y < bottom {
            i += 1;
            let n = ck.wrapping_mul(7).wrapping_add(i);
            y = (y + 2.0 + (hf(key ^ 2, n) * 2.0).floor()).min(bottom);
            if y >= bottom {
                dir = last.unwrap_or(dir);
            }
            pts.push((mid + dir * amp * (0.5 + 0.5 * hf(key ^ 3, n)), y));
            dir = -dir;
        }
        pts
    };
    let main = ox + dx;
    let mut out = vec![Bolt { pts: path(key, main, amp, None), sparks: true }];
    match ctx.magic {
        Magic::Small => {}
        // a second, thinner-swinging bolt nearer the body on about half the casts
        Magic::Normal => {
            if hf(key ^ 5, ck) < 0.5 {
                out.push(Bolt { pts: path(key ^ 0x51, ox + 3.0, 0.8, Some(-1.0)), sparks: true });
            }
        }
        // a bolt on each side of the main one, sometimes a fork off it
        Magic::Big => {
            out.push(Bolt { pts: path(key ^ 0x52, main - 6.5, 1.4, Some(-1.0)), sparks: true });
            out.push(Bolt { pts: path(key ^ 0x53, main + 6.0, 1.4, Some(1.0)), sparks: true });
            if hf(key ^ 6, ck) < 0.6 {
                let from = out[0].pts[out[0].pts.len() / 2];
                let side = if hf(key ^ 7, ck) < 0.5 { -1.0 } else { 1.0 };
                let fork = vec![from, (from.0 + side * 2.0, from.1 + 2.0), (from.0 + side * 2.5, (from.1 + 4.0).min(bottom))];
                out.push(Bolt { pts: fork, sparks: false });
            }
        }
    }
    out
}

/// Strike x (real sub-pixels, clipped to the magic area) of every bolt of
/// the cast at or just before `ctx.t`; the main bolt first. Empty when no
/// bolt is showing.
#[cfg(test)]
pub(super) fn strikes(g: &Geom, ctx: &Ctx) -> Vec<f32> {
    let (dur, ..) = bolt_style(ctx.magic);
    let Some((since_cast, _)) = stream(ctx.seed, ctx.t) else { return Vec::new() };
    if since_cast >= dur {
        return Vec::new();
    }
    let a = Area::new(g, ctx.magic, Theme::Lightning, CY);
    bolts(ctx, a.ox, g.h as f32 - 1.0, ctx.t - since_cast)
        .iter()
        .filter(|b| b.sparks)
        .map(|b| b.pts.last().map_or(a.end - 1.0, |p| p.0.min(a.end - 1.0)))
        .collect()
}

/// Lightning: zig-zag bolts from the top of the area to the bottom with a
/// spark burst where they strike; every cast strikes new paths. Small casts
/// one thin bolt, Normal one or two bolts two dots wide, Big three or four
/// (with a fork) three dots wide and a bigger burst.
fn bolt(cv: &mut Canvas, ctx: &Ctx, a: &Area, since_cast: u64) {
    let (dur, _, _, burst, width) = bolt_style(ctx.magic);
    if since_cast >= dur {
        return;
    }
    let bottom = cv.g.h as f32 - 1.0;
    let all = bolts(ctx, a.ox, bottom, ctx.t - since_cast);

    // the bolts flicker: on, on, off, on (big: once more)
    let shown = matches!(since_cast, 0 | 1 | 3) || (width >= 3 && since_cast == 5);
    if shown {
        let lo = -(width - 1) / 2;
        for b in &all {
            for w in b.pts.windows(2) {
                let ((x0, y0), (x1, y1)) = (w[0], w[1]);
                let steps = (x1 - x0).abs().max(y1 - y0).ceil().max(1.0) as i32;
                for s in 0..=steps {
                    let f = s as f32 / steps as f32;
                    let (x, y) = ((x0 + (x1 - x0) * f).round() as i32, (y0 + (y1 - y0) * f).round() as i32);
                    for k in lo..lo + width {
                        let c = if k == 0 { [255., 255., 235.] } else { [255., 225., 90.] };
                        a.dot(cv, x + k, y, c, 2);
                    }
                }
            }
        }
    }

    // spark bursts at the impacts, spreading and fading
    let fade = since_cast as f32 / dur as f32;
    let rays: [(i32, i32); 7] = [(-1, 0), (1, 0), (-1, -1), (1, -1), (0, -1), (-2, -1), (2, -1)];
    for (i, b) in all.iter().enumerate().filter(|(_, b)| b.sparks) {
        // the main bolt bursts strongest
        let burst = if i == 0 { burst } else { (burst - 1).max(1) };
        let reach = burst.min(since_cast as i32 + 1);
        let (xb, yb) = *b.pts.last().unwrap_or(&(a.ox, bottom));
        let (bx, by) = (xb.round() as i32, yb.round() as i32);
        let half = (width - 1) / 2;
        for (rx, ry) in rays {
            for k in 1..=reach {
                let c = lerp(lerp([255., 250., 200.], [255., 170., 30.], k as f32 / (burst as f32 + 1.0)), CARD_BG, fade * 0.7);
                // rays start at the bolt's edge, not its centre
                let side = rx.signum() * half;
                a.dot(cv, bx + side + rx * k, by + ry * k, c, 2);
            }
        }
    }
}
