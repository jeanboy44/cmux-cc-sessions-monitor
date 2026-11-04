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
//! - lightning: zig-zag bolts striking down with spark bursts: one thin
//!   bolt, four thin ones, or a thick one among three thin ones
//!
//! Water, poison and lightning are centred in the magic area: the columns
//! from just right of the body to the right edge of the canvas (which the
//! card widens to its inner width, see `SpiritParams::cols`). Fire keeps
//! breathing from the mouth, its length set by the level (Big roars to the
//! right edge of the room).
//!
//! The small spirit (`Ctx::xs`) casts simpler, bolder shapes sized for
//! readability rather than scaled down: a short bright flame, a solid disc
//! with a clear highlight, dense round puffs (and the skull, which fits the
//! standard card's area), and the same bolts as the S spirit.

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

/// Horizontal centre (real sub-pixels) of the magic area: from the first
/// whole cell right of the body (its outline radius, rounded up to a cell)
/// to the right edge of the canvas.
pub(super) fn area_mid(g: &Geom) -> f32 {
    (area_start(g) + g.w as f32) / 2.0
}

/// Left edge (real sub-pixels) of the magic area: the first whole cell
/// right of the body.
fn area_start(g: &Geom) -> f32 {
    ((g.rcx + BASE_R * g.ppu) / 2.0).ceil() * 2.0
}

#[cfg(test)]
thread_local! {
    /// Tests isolating the magic switch it off to diff a frame against.
    pub(super) static NO_MAGIC: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Where the magic goes, in real sub-pixels: the mouth (`ox`, `oy`) at the
/// body's right edge, the centre of the magic area (`mid`) and the right end
/// of the reach (exclusive).
#[derive(Clone, Copy)]
struct Area {
    ox: f32,
    oy: f32,
    mid: f32,
    end: f32,
}

impl Area {
    fn new(g: &Geom, cy: f32) -> Self {
        Area { ox: mouth_x(g), oy: g.rcy + (cy - CY) * g.ppu, mid: area_mid(g), end: g.w as f32 }
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
    #[cfg(test)]
    if NO_MAGIC.with(std::cell::Cell::get) {
        return;
    }
    let Some((since_cast, since_start)) = stream(ctx.seed, ctx.t) else { return };
    let a = Area::new(&cv.g, cy);
    match ctx.elem {
        Theme::Fire => breath(cv, ctx, &a, since_cast, since_start),
        Theme::Water => vortex(cv, ctx, &a, since_cast),
        Theme::Poison => fog(cv, ctx, &a),
        Theme::Lightning => bolt(cv, ctx, &a, since_cast),
    }
}

/// Sub-pixels from the mouth to the first flame column (the one-dot gap),
/// unscaled: the gap is one dot at every size.
const BREATH_GAP: f32 = 1.8;
/// How far embers and smoke of a short flame may drift past its tip
/// (sub-pixels); the big flame's tip is the card area's edge.
const EMBER_DRIFT: f32 = 2.0;

/// Fire breath per magic level: (flame reach past the gap in sub-pixels,
/// half height of the cone's end in sub-pixels, wave amplitude in
/// sub-pixels, embers, smoke puffs). Small is a short steady jet about three
/// columns long, one or two rows tall and without smoke; Normal about five
/// to six columns, two (at most three) rows at its end; Big roars to the
/// right edge of the room (the card's inner edge). The small spirit's
/// flame is two, four and all the columns long, slimmer, with fewer embers.
fn breath_style(m: Magic, xs: bool) -> (f32, f32, f32, u32, u32) {
    match (m, xs) {
        (Magic::Small, false) => (6.2, 0.8, 0.35, 0, 0),
        (Magic::Normal, false) => (9.3, 1.0, 0.55, 2, 1),
        (Magic::Big, false) => (f32::INFINITY, 4.6, 0.9, 7, 4),
        (Magic::Small, true) => (4.4, 0.7, 0.3, 0, 0),
        (Magic::Normal, true) => (7.4, 1.1, 0.45, 1, 0),
        (Magic::Big, true) => (f32::INFINITY, 3.4, 0.7, 4, 2),
    }
}

/// Fire: a breath of flame from the mouth. One empty dot column separates
/// it from the body; it starts as a single dot and widens with distance into
/// a cone, densest along its middle, flickering and breaking up at the end
/// with embers flying off and a little smoke.
fn breath(cv: &mut Canvas, ctx: &Ctx, a: &Area, since_cast: u64, since_start: u64) {
    let (reach, w_end, wave, embers, smokes) = breath_style(ctx.magic, ctx.xs);
    // the first flame column: one empty dot column right of the body
    let edge = cv.body_right().unwrap_or(a.ox.floor() as i32);
    let xs = (edge + 2) as f32;
    // the tip of the level's flame, measured from the mouth past the gap
    // (so the casting pose does not stretch it); embers of the shorter
    // flames drift a little past it
    let tip = (a.ox + BREATH_GAP + reach).min(a.end).round();
    let drift = if ctx.magic == Magic::Big { 0.0 } else { EMBER_DRIFT };
    let a = &Area { end: (tip + drift).min(a.end), ..*a };
    let full = tip - xs;
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
        a.oy - 0.5 + u * wave * (x * 0.55 - ctx.tf * 0.8 + ph).sin() - u * u
    };
    // half height of the cone at distance `d` (dots) from its first column
    let hw = |d: f32| (0.1 + 0.42 * d).min(w_end);

    for k in 0..smokes {
        let key = ctx.seed ^ (k as u32).wrapping_mul(0x5A0C);
        let period = 14;
        let shifted = ctx.t.wrapping_add(slot(key, 0x5A1, 0, period as u32));
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
            // the end breaks up into tongues; the small spirit's cone is
            // solid (no ragged edges) and hotter, so it reads at its size
            let tip = u < 0.78 || n > (u - 0.78) * 2.6;
            if !centre && (!tip || (!ctx.xs && n > dens)) {
                continue;
            }
            let heat = if ctx.xs { (1.0 - 0.6 * dy) * (1.0 - 0.35 * u) } else { (1.0 - dy) * (1.0 - 0.55 * u) };
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

    for k in 0..embers {
        let key = ctx.seed ^ k.wrapping_mul(0x2F1B);
        let period = 8 + u64::from(k % 3);
        let shifted = ctx.t.wrapping_add(slot(key, 0xE1, 0, period as u32));
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

/// Turn of the water vortex per tick (radians). Normal turns very slowly,
/// one revolution in about 8.4 s at 15 fps (a seventh of the others); Small
/// and Big keep the brisk spin.
fn vortex_spin(m: Magic) -> f32 {
    match m {
        Magic::Small | Magic::Big => 0.35,
        Magic::Normal => 0.05,
    }
}

/// Rotation angle of the vortex's spiral arms on a frame.
pub(super) fn vortex_rot(ctx: &Ctx) -> f32 {
    ctx.tf * vortex_spin(ctx.magic) + seed_phase(ctx.seed, 0x5B1)
}

/// Water: a glossy, translucent whirlpool. A dark deep-blue core, a sparse
/// mid-blue body, spiral arms that brighten toward their tips, a lighter rim
/// and a bright specular highlight from the upper left (it stays put while
/// the arms spin), with droplets trailing the arm tips and foam sparkles.
/// It opens after each cast and closes again, centred in the magic area.
fn vortex(cv: &mut Canvas, ctx: &Ctx, a: &Area, since_cast: u64) {
    let (dur, major, arms): (u64, f32, u32) = match (ctx.magic, ctx.xs) {
        (Magic::Small, false) => (20, 1.9, 1),
        (Magic::Normal, false) => (18, 3.6, 2),
        (Magic::Big, false) => (30, 7.6, 3),
        // the small spirit's disc keeps at least 3 x 2 cells at Normal
        (Magic::Small, true) => (20, 2.4, 1),
        (Magic::Normal, true) => (18, 4.0, 2),
        (Magic::Big, true) => (30, 6.4, 3),
    };
    if since_cast >= dur {
        return;
    }
    let sc = since_cast as f32;
    let open = ((sc + 1.0) / 4.0).min(1.0) * ((dur as f32 - sc) / 4.0).min(1.0);
    let (ra, rb) = ((major * open).max(0.8), (major * DISC_SQUASH * open).max(0.6));
    let (cx, cy) = (a.mid, a.oy);
    let (ca, sa) = (DISC_TILT.cos(), DISC_TILT.sin());
    // screen half extents of the tilted disc (normalise the highlight spot)
    let hx = (ra * ra * ca * ca + rb * rb * sa * sa).sqrt();
    let hy = (ra * ra * sa * sa + rb * rb * ca * ca).sqrt();
    let rot = vortex_rot(ctx);
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
            let spec = (nx + 0.36).powi(2) + (ny + 0.4).powi(2) < if ctx.xs { 0.16 } else { 0.1 };
            let s = ((phi + twist * rho - rot) * arms_f / TAU).rem_euclid(1.0);
            let on_arm = s < 0.2 + 0.14 * rho;
            let n = hf((x as u32).wrapping_mul(97).wrapping_add((y as u32).wrapping_mul(13)), ctx.seed);
            let (c, paint) = if spec {
                ([228., 248., 255.], true)
            } else if ctx.xs {
                // the small disc: denser and lighter blues, so it reads as a solid shape
                if rho > 0.86 {
                    ([110., 185., 250.], n < 0.9)
                } else if on_arm && rho > 0.2 {
                    (lerp([60., 130., 225.], [170., 230., 255.], (rho - 0.2) / 0.66), true)
                } else if rho < 0.34 {
                    ([40., 95., 210.], n < 0.95)
                } else {
                    ([55., 120., 215.], n < 0.7)
                }
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

/// Poison burst centre in real sub-pixels: the centre of the magic area at
/// the body centre's height.
pub(super) fn fog_anchor(g: &Geom) -> (f32, f32) {
    (area_mid(g), g.rcy)
}

/// Ticks one cast's toxic puffs live.
pub(super) fn fog_life(m: Magic) -> u64 {
    match m {
        Magic::Small => 20,
        Magic::Normal => 20,
        Magic::Big => 34,
    }
}

/// The skull a Big poison cast's fog settles into at its end, on a 14 x 14
/// sub-pixel grid (7 x 3.5 cells, about the size of the big cloud): `#` is
/// bone, `.` empty. The eye sockets (4 x 4, one whole cell each), the nose
/// notch and the gaps between the teeth stay dark.
pub(super) const SKULL: [&str; 14] = [
    "....######....",
    "..##########..",
    ".############.",
    "##############",
    "#....####....#",
    "#....####....#",
    "#....####....#",
    "#....####....#",
    ".#####..#####.",
    "..####..####..",
    "...########...",
    "...##.##.##...",
    "...########...",
    "....######....",
];
/// Ages (ticks after its cast) at which the skull starts to form, is fully
/// formed, starts to fade and is gone.
const SKULL_FORM: u64 = 22;
const SKULL_HOLD: u64 = 25;
const SKULL_FADE: u64 = 32;
const SKULL_END: u64 = 37;
/// Bone (pale green-grey, never white) and its green glow.
const SKULL_BONE: Rgb = [196., 228., 186.];
const SKULL_GLOW: Rgb = [92., 205., 78.];

/// (fully formed, whole life) of the skull in ticks after its cast.
#[cfg(test)]
pub(super) fn skull_window() -> (std::ops::Range<u64>, std::ops::Range<u64>) {
    (SKULL_HOLD..SKULL_FADE, SKULL_FORM..SKULL_END)
}

/// How much of the skull shows `age` ticks after its cast (0..=1).
fn skull_alpha(age: u64) -> f32 {
    match age {
        a if a < SKULL_FORM || a >= SKULL_END => 0.0,
        a if a < SKULL_HOLD => (a - SKULL_FORM + 1) as f32 / (SKULL_HOLD - SKULL_FORM + 1) as f32,
        a if a < SKULL_FADE => 1.0,
        a => (SKULL_END - a) as f32 / (SKULL_END - SKULL_FADE + 1) as f32,
    }
}

/// Whether the skull fits the canvas: as tall as the skull, and the magic
/// area at least a column wider on each side.
fn skull_fits(g: &Geom) -> bool {
    g.h >= SKULL.len() && g.w as f32 - area_start(g) >= SKULL[0].len() as f32 + 2.0
}

/// Top-left sub-pixel of the skull: centred on the burst anchor, snapped so
/// each eye socket is one whole braille cell, inside the canvas.
pub(super) fn skull_origin(g: &Geom) -> (i32, i32) {
    let (ax, ay) = fog_anchor(g);
    let (w, h) = (SKULL[0].len() as f32, SKULL.len() as f32);
    let x = (ax - w / 2.0).round() as i32;
    // socket columns start at x + 1: x odd
    let x = if x.rem_euclid(2) == 1 { x } else { x - 1 };
    // socket rows start at y + 4: y on a cell row
    let y = ((ay - h / 2.0) / 4.0).round() as i32 * 4;
    (x, y.clamp(0, (g.h as i32 - SKULL.len() as i32).max(0)))
}

/// Whether skull grid point (c, r) is bone.
fn bone(c: i32, r: i32) -> bool {
    r >= 0 && c >= 0 && SKULL.get(r as usize).is_some_and(|row| row.as_bytes().get(c as usize) == Some(&b'#'))
}

/// Paints the skull at strength `alpha`: the fog settles into bone dots
/// (a few at first, all once formed, thinning and dimming as it fades), the
/// sockets, nose and gaps between the teeth cleared to the card, and a
/// sparse green glow just outside its box.
fn skull(cv: &mut Canvas, ctx: &Ctx, a: &Area, left: i32, alpha: f32) {
    let (x0, y0) = skull_origin(&cv.g);
    let (w, h) = (SKULL[0].len() as i32, SKULL.len() as i32);
    let col = lerp(CARD_BG, SKULL_BONE, 0.35 + 0.65 * alpha);
    let glow = lerp(CARD_BG, SKULL_GLOW, 0.75 * alpha);
    for r in -2..h + 2 {
        for c in -2..w + 2 {
            let (x, y) = (x0 + c, y0 + r);
            if x < left {
                continue;
            }
            let n = hf((c as u32).wrapping_mul(29).wrapping_add((r as u32).wrapping_mul(11)), ctx.seed ^ 0x5C0);
            let inside = (0..w).contains(&c) && (0..h).contains(&r);
            if bone(c, r) {
                if n < alpha {
                    a.dot(cv, x, y, col, 2);
                }
            } else if inside {
                if alpha >= 0.5 {
                    cv.clear_px(x, y);
                }
            } else if n < 0.55 * alpha && (-1..=1).any(|dy| (-1..=1).any(|dx| bone(c + dx, r + dy))) {
                a.dot(cv, x, y, glow, 1);
            }
        }
    }
}

/// Poison: every cast bursts toxic puffs out of a fixed centre in the magic
/// area. They swell radially, thinning and fading as they grow, with
/// edges wobbling in smooth noise; Normal and Big casts puff several
/// overlapping clouds. The dither pattern is fixed, so the fog creeps
/// instead of flickering. At the end of a Big cast the fog settles into a
/// [`SKULL`] that shows for a few ticks and fades out (only where it fits,
/// see [`skull_fits`]). The small spirit puffs smaller but denser, brighter
/// clouds.
fn fog(cv: &mut Canvas, ctx: &Ctx, a: &Area) {
    let (rmax, puffs, spread, thick): (f32, u32, f32, f32) = match (ctx.magic, ctx.xs) {
        (Magic::Small, false) => (2.5, 1, 0.0, 1.0),
        (Magic::Normal, false) => (5.0, 2, 3.4, 1.0),
        (Magic::Big, false) => (9.5, 3, 2.6, 1.2),
        (Magic::Small, true) => (3.0, 1, 0.0, 1.1),
        (Magic::Normal, true) => (4.6, 2, 2.6, 1.15),
        (Magic::Big, true) => (7.0, 3, 2.4, 1.3),
    };
    let life = fog_life(ctx.magic);
    let (ax, ay) = fog_anchor(&cv.g);
    // (centre x, centre y, radius, strength, wobble phase) of every live puff
    let mut live: Vec<(f32, f32, f32, f32, f32)> = Vec::new();
    let skulls = ctx.magic == Magic::Big && skull_fits(&cv.g);
    let mut alpha = 0.0f32;
    for age in 0..if skulls { life.max(SKULL_END) } else { life } {
        let Some(c) = ctx.t.checked_sub(age) else { break };
        if c == 0 || !state::casts_at(ctx.seed, c) {
            continue;
        }
        if skulls {
            alpha = alpha.max(skull_alpha(age));
        }
        if age >= life {
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
    // like the breath, the fog keeps one empty dot column off the body
    let left = cv.body_right().map_or(0, |e| e + 2);
    if !live.is_empty() {
        fog_puffs(cv, ctx, a, &mut live, (rmax, spread, alpha), left);
    }
    if alpha > 0.0 {
        skull(cv, ctx, a, left, alpha);
    }
}

/// Draws the live puffs (centre x, centre y, radius, strength, wobble
/// phase); `shape` is (largest radius, spread, skull strength): the fog
/// thins out inside the skull's box as the skull forms.
fn fog_puffs(cv: &mut Canvas, ctx: &Ctx, a: &Area, live: &mut [(f32, f32, f32, f32, f32)], shape: (f32, f32, f32), left: i32) {
    let (rmax, spread, alpha) = shape;
    let (ax, _) = fog_anchor(&cv.g);
    let (sx, sy) = skull_origin(&cv.g);
    let in_skull = |x: i32, y: i32| (sx - 1..sx + SKULL[0].len() as i32 + 1).contains(&x) && (sy - 1..sy + SKULL.len() as i32 + 1).contains(&y);
    // keep the cloud as a whole centred: shift the puffs so their weighted
    // mean x sits on the anchor
    let w: f32 = live.iter().map(|p| p.3 * p.2 * p.2).sum();
    if w > 0.0 {
        let off = live.iter().map(|p| p.3 * p.2 * p.2 * p.0).sum::<f32>() / w - ax;
        for p in live.iter_mut() {
            p.0 -= off;
        }
    }
    // clipped evenly on both sides of the anchor so it stays centred
    let left = left as f32;
    let reach = (rmax * 1.3 + spread).min(ax - left).min(cv.g.w as f32 - ax).max(0.0);
    for y in 0..cv.g.h as i32 {
        for x in (ax - reach).floor() as i32..(ax + reach).ceil() as i32 {
            let (xf, yf) = (x as f32 + 0.5, y as f32 + 0.5);
            let mut dens = 0.0f32;
            for &(px, py, r, amp, ph) in live.iter() {
                let (dx, dy) = (xf - px, yf - py);
                let th = dy.atan2(dx);
                let wob = 1.0 + 0.16 * (3.0 * th + ph + ctx.tf * 0.05).sin() + 0.08 * (5.0 * th - ctx.tf * 0.035 + 2.0 * ph).sin();
                let d = dx.hypot(dy) / (r * wob);
                if d < 1.0 {
                    dens = dens.max(amp * (1.0 - d * d).powf(0.6));
                }
            }
            if alpha > 0.0 && in_skull(x, y) {
                dens *= 1.0 - alpha;
            }
            // the dither mirrors about the anchor, so it never leans the cloud
            let mx = ((xf - ax).abs() * 2.0) as u32;
            let q = hf(mx.wrapping_mul(131).wrapping_add((y as u32).wrapping_mul(17)), ctx.seed);
            // the small spirit's puffs are denser and brighter
            let (cut, lo, hi) = if ctx.xs { (q * 0.45 + 0.1, [70., 150., 70.], [185., 255., 130.]) } else { (q * 0.8 + 0.12, [48., 105., 52.], [165., 250., 115.]) };
            if dens < cut {
                continue;
            }
            a.dot(cv, x, y, lerp(lo, hi, dens), 1);
        }
    }
}

/// Lightning per magic level: (flicker duration of one bolt, zig-zag
/// amplitude of the main bolt in sub-pixels, spark burst reach of the main
/// bolt, main bolt width in dots). Every size strikes the same bolts: thin
/// ones are one dot wide, the thick one three, which reads even at the
/// small spirit's size.
fn bolt_style(m: Magic) -> (u64, f32, i32, i32) {
    match m {
        Magic::Small => (3, 0.8, 1, 1),
        Magic::Normal => (3, 0.8, 1, 1),
        Magic::Big => (7, 1.0, 4, 3),
    }
}

/// Ticks a bolt may flash after its cast: the bolts of a Normal or Big cast
/// are staggered by up to this much so they never all flash at once.
const MAX_DELAY: u64 = 2;
/// Half the span (sub-pixels) the four Normal bolts spread over; the magic
/// area may cut it shorter.
const NORMAL_SPREAD: f32 = 5.5;
/// Zig-zag amplitude (sub-pixels) of a thin side bolt.
const THIN_AMP: f32 = 0.7;

/// One bolt of a cast: its zig-zag points from the top row to the bottom
/// row, its width in dots, the ticks after the cast it flashes and its spark
/// burst reach.
#[cfg_attr(test, derive(Debug))]
pub(super) struct PlannedBolt {
    pub(super) pts: Vec<(f32, f32)>,
    pub(super) width: i32,
    pub(super) delay: u64,
    pub(super) burst: i32,
    #[cfg(test)]
    pub(super) strike: f32,
}

/// The bolts of the cast at `cast` in the magic area `[lo, end)` around
/// `mid` (sub-pixels): Small strikes one thin bolt in the centre; Normal
/// four thin bolts at random distinct x spread over the area, centred as a
/// group; Big one thick bolt in the centre (first) and three thin ones at
/// random distances around it, two on one side and one on the other, clear
/// of it. Normal and Big bolts flash a random tick or two apart.
fn bolts(ctx: &Ctx, lo: f32, mid: f32, end: f32, bottom: f32, cast: u64) -> Vec<PlannedBolt> {
    let (_, amp, burst, width) = bolt_style(ctx.magic);
    let ck = (cast as u32) ^ ((cast >> 32) as u32);
    let key = ctx.seed ^ 0xB017;
    // keeps a bolt in the magic area (Small keeps its original path)
    let (lo, hi) = (lo, (end - 1.0).max(lo));
    // a zig-zag from (top x around `mid`) down to the bottom; `last` forces
    // the final swing, `keep` clamps every point
    let path = |key: u32, mid: f32, amp: f32, last: Option<f32>, keep: &dyn Fn(f32) -> f32| {
        let mut dir = if hf(key, ck) < 0.5 { -1.0 } else { 1.0 };
        let mut pts = vec![(keep(mid + (hf(key ^ 1, ck) - 0.5) * amp), 0.0)];
        let (mut y, mut i) = (0.0, 0u32);
        while y < bottom {
            i += 1;
            let n = ck.wrapping_mul(7).wrapping_add(i);
            y = (y + 2.0 + (hf(key ^ 2, n) * 2.0).floor()).min(bottom);
            if y >= bottom {
                dir = last.unwrap_or(dir);
            }
            pts.push((keep(mid + dir * amp * (0.5 + 0.5 * hf(key ^ 3, n))), y));
            dir = -dir;
        }
        pts
    };
    let delay = |k: u32| (hf(key ^ 0xDE1 ^ k, ck) * (MAX_DELAY + 1) as f32) as u64;
    let thin = |pts, delay| PlannedBolt { pts, width: 1, delay, burst: 1, #[cfg(test)] strike: 0.0 };
    let out = match ctx.magic {
        Magic::Small => {
            vec![PlannedBolt { pts: path(key, mid, amp, None, &|x| x), width, delay: 0, burst, #[cfg(test)] strike: 0.0 }]
        }
        Magic::Normal => {
            // four slots across the spread, a random offset in each, the
            // group shifted back onto the centre
            let half = NORMAL_SPREAD.min((hi - lo) / 2.0 - 0.5).max(0.0);
            let slot_w = 2.0 * half / 4.0;
            let mut xs: Vec<f32> = (0..4u32).map(|k| -half + (k as f32 + 0.5) * slot_w + (hf(key ^ 0x4E0 ^ k, ck) - 0.5) * 0.6 * slot_w).collect();
            let shift = xs.iter().sum::<f32>() / 4.0;
            for x in &mut xs {
                *x += mid - shift;
            }
            // one bolt (a random one) flashes on the cast tick, another a
            // tick or two later, the other two at random
            let first = (hf(key ^ 0x4E5, ck) * 4.0) as usize % 4;
            let late = (first + 1 + (hf(key ^ 0x4E6, ck) * 3.0) as usize % 3) % 4;
            let keep = |x: f32| x.clamp(lo, hi);
            xs.iter()
                .enumerate()
                .map(|(k, &x)| {
                    let d = if k == first {
                        0
                    } else if k == late {
                        1 + delay(k as u32) % MAX_DELAY
                    } else {
                        delay(k as u32)
                    };
                    thin(path(key ^ 0x60 ^ k as u32, x, amp, None, &keep), d)
                })
                .collect()
        }
        Magic::Big => {
            let keep = |x: f32| x.clamp(lo, hi);
            let thick = PlannedBolt { pts: path(key, mid, amp, None, &keep), width, delay: 0, burst, #[cfg(test)] strike: 0.0 };
            // thin bolts keep a clear dot column off the thick one's reach
            let half_w = ((width - 1) / 2) as f32;
            let clear = amp + half_w + 1.5;
            let d1 = clear + hf(key ^ 0xB10, ck);
            let d2 = d1 + 2.0 + hf(key ^ 0xB11, ck);
            // the lone bolt on the other side balances the pair
            let d3 = ((d1 + d2) / 2.0 + 0.5 + (hf(key ^ 0xB12, ck) - 0.5) * 1.5).max(clear + 0.5);
            let side = if hf(key ^ 0xB13, ck) < 0.5 { -1.0 } else { 1.0 };
            let mut out = vec![thick];
            for (k, (d, s)) in [(d1, side), (d2, side), (d3, -side)].into_iter().enumerate() {
                let near = mid + s * clear;
                let keep = move |x: f32| if s > 0.0 { x.max(near) } else { x.min(near) }.clamp(lo, hi);
                let fin = Some(s);
                let delay = delay(k as u32) % (MAX_DELAY + 1);
                out.push(thin(path(key ^ 0x70 ^ k as u32, mid + s * d, THIN_AMP, fin, &keep), delay));
            }
            // never all on the cast tick
            if out.iter().all(|b| b.delay == 0) {
                out[1 + (hf(key ^ 0xB14, ck) * 3.0) as usize % 3].delay = 1;
            }
            out
        }
    };
    #[cfg(test)]
    let out = {
        let mut out = out;
        for b in &mut out {
            b.strike = b.pts.last().map_or(end - 1.0, |p| p.0.min(end - 1.0));
        }
        out
    };
    out
}

/// The bolts of the cast showing at `ctx.t` (empty when none is), in the
/// canvas `g` (the main or thick bolt first).
#[cfg(test)]
pub(super) fn bolt_plan(g: &Geom, ctx: &Ctx) -> Vec<PlannedBolt> {
    let (dur, ..) = bolt_style(ctx.magic);
    let Some((since_cast, _)) = stream(ctx.seed, ctx.t) else { return Vec::new() };
    if since_cast >= dur + MAX_DELAY {
        return Vec::new();
    }
    let a = Area::new(g, CY);
    bolts(ctx, area_start(g), a.mid, a.end, g.h as f32 - 1.0, ctx.t - since_cast)
}

/// Lightning: zig-zag bolts from the top of the area to the bottom with a
/// spark burst where they strike; every cast strikes new paths. Small casts
/// one thin bolt; Normal four thin bolts spread over the area, flashing a
/// tick or two apart; Big one thick bolt (three dots) in the centre with a
/// strong burst and three thin ones around it.
fn bolt(cv: &mut Canvas, ctx: &Ctx, a: &Area, since_cast: u64) {
    let (dur, ..) = bolt_style(ctx.magic);
    if since_cast >= dur + MAX_DELAY {
        return;
    }
    let bottom = cv.g.h as f32 - 1.0;
    let all = bolts(ctx, area_start(&cv.g), a.mid, a.end, bottom, ctx.t - since_cast);

    for b in &all {
        // ticks since this bolt's own flash
        let Some(local) = since_cast.checked_sub(b.delay) else { continue };
        if local >= dur {
            continue;
        }
        // the bolts flicker: on, on, off, on (big: once more)
        let shown = matches!(local, 0 | 1 | 3) || (matches!(ctx.magic, Magic::Big) && local == 5);
        let lo = -(b.width - 1) / 2;
        if shown {
            for w in b.pts.windows(2) {
                let ((x0, y0), (x1, y1)) = (w[0], w[1]);
                let steps = (x1 - x0).abs().max(y1 - y0).ceil().max(1.0) as i32;
                for s in 0..=steps {
                    let f = s as f32 / steps as f32;
                    let (x, y) = ((x0 + (x1 - x0) * f).round() as i32, (y0 + (y1 - y0) * f).round() as i32);
                    for k in lo..lo + b.width {
                        let c = if k == 0 { [255., 255., 235.] } else { [255., 225., 90.] };
                        a.dot(cv, x + k, y, c, 2);
                    }
                }
            }
        }

        // a spark burst at the impact, spreading and fading
        let fade = local as f32 / dur as f32;
        let reach = b.burst.min(local as i32 + 1);
        let (xb, yb) = *b.pts.last().unwrap_or(&(a.ox, bottom));
        let (bx, by) = (xb.round() as i32, yb.round() as i32);
        let half = (b.width - 1) / 2;
        // thin bolts' bursts keep to their own column and the next; the
        // thick bolt's bursts wider and higher
        let rays: &[(i32, i32)] = if b.width > 1 {
            &[(-1, 0), (1, 0), (-1, -1), (1, -1), (0, -1), (-2, -1), (2, -1), (-3, -1), (3, -1), (-1, -2), (1, -2)]
        } else {
            &[(-1, 0), (1, 0), (-1, -1), (1, -1), (0, -1)]
        };
        for &(rx, ry) in rays {
            for k in 1..=reach {
                let c = lerp(lerp([255., 250., 200.], [255., 170., 30.], k as f32 / (b.burst as f32 + 1.0)), CARD_BG, fade * 0.7);
                // rays start at the bolt's edge, not its centre
                let side = rx.signum() * half;
                a.dot(cv, bx + side + rx * k, by + ry * k, c, 2);
            }
        }
    }
}
