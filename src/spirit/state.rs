//! Session status as spirit mood: motion speed, activity, bobbing, brightness
//! and the state cues (`?` for input, `z` for idle, sparkles when done).

use crate::model::Status;

use super::canvas::{lerp, mul, Canvas, Rgb, CARD_BG};
use super::noise::{seed_phase, slot};
use super::theme::Theme;
use super::{Ctx, SpiritGrid, BASE_R};

/// Animation speed of the body outline.
pub(super) fn speed(s: Status) -> f32 {
    match s {
        Status::Running => 2.6,
        Status::NeedsInput => 1.4,
        Status::Idle => 0.3,
        Status::Done => 0.9,
        Status::Unknown => 0.25,
    }
}

/// Ambient particle rate multiplier.
pub(super) fn activity(s: Status) -> f32 {
    match s {
        Status::Running => 1.6,
        Status::NeedsInput => 1.0,
        Status::Idle => 0.12,
        Status::Done => 0.7,
        Status::Unknown => 0.0,
    }
}

/// Vertical body offset in design units.
pub(super) fn bob(s: Status, seed: u32, tf: f32) -> f32 {
    let ph = seed_phase(seed, 0xB0B);
    match s {
        Status::Running => (tf * 0.28 + ph).sin(),
        Status::NeedsInput => (tf * 0.2 + ph).sin(),
        Status::Idle => (tf * 0.05 + ph).sin(),
        Status::Done => 0.6 - 1.2 * (tf * 0.12 + ph).sin().abs(),
        Status::Unknown => 0.6 * (tf * 0.04 + ph).sin(),
    }
}

/// Glow pulse in [0, 1] while waiting for input, else 0.
pub(super) fn pulse(s: Status, tf: f32) -> f32 {
    if s == Status::NeedsInput {
        0.5 + 0.5 * (tf * 0.3).sin()
    } else {
        0.0
    }
}

pub(super) fn brightness(s: Status, pulse: f32) -> f32 {
    match s {
        Status::NeedsInput => 1.0 + 0.2 * pulse,
        Status::Idle => 0.6,
        Status::Unknown => 0.5,
        _ => 1.0,
    }
}

/// Ticks between casts are 12..=40: one cast per window, at a seeded offset.
const CAST_WINDOW: u64 = 26;
const CAST_JITTER: u32 = 14;

/// Whether a running spirit casts on tick `t`.
pub(super) fn casts_at(seed: u32, t: u64) -> bool {
    t % CAST_WINDOW == slot(seed, 0xCA57, t / CAST_WINDOW, CAST_JITTER)
}

/// Frames of the casting wind-up before each cast and of the release from it.
const WIND_UP: u64 = 3;
const RELEASE: u64 = 2;

/// Where a running spirit is in its cast timeline (the same schedule that
/// spawns the magic: the magic appears on `Release(0)`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CastPhase {
    /// frame 0..3 of the wind-up before a cast
    WindUp(u8),
    /// frame 0..2 from the cast tick on
    Release(u8),
}

/// Cast phase of a running spirit of `seed` on tick `t`; None at rest.
/// Casts are at least 12 ticks apart, so phases never overlap. Tick 0 never
/// casts (the magic and particles start at tick 1).
pub(super) fn cast_phase(seed: u32, t: u64) -> Option<CastPhase> {
    for k in 0..RELEASE {
        if t >= k + 1 && casts_at(seed, t - k) {
            return Some(CastPhase::Release(k as u8));
        }
    }
    (1..=WIND_UP).find(|&k| casts_at(seed, t + k)).map(|k| CastPhase::WindUp((WIND_UP - k) as u8))
}

/// Body pose while casting, in real sub-pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Pose {
    /// horizontal shift (+ right, toward the magic)
    pub dx: f32,
    /// vertical shift (+ down)
    pub dy: f32,
    /// extra shift of the top of the body (+ right), fading to the bottom
    pub tilt: f32,
    /// outline radius scale
    pub swell: f32,
    /// the eye dot looks one sub-pixel toward the magic
    pub facing: bool,
}

impl Pose {
    pub(super) const REST: Pose = Pose { dx: 0.0, dy: 0.0, tilt: 0.0, swell: 1.0, facing: false };
}

/// Casting pose of a running spirit: a short wind-up leaning toward the
/// magic, then a one sub-pixel recoil on release, flavoured per element.
pub(super) fn pose(elem: Theme, status: Status, seed: u32, t: u64) -> Pose {
    if status != Status::Running {
        return Pose::REST;
    }
    let Some(phase) = cast_phase(seed, t) else { return Pose::REST };
    let p = Pose { facing: true, ..Pose::REST };
    match (elem, phase) {
        // fire: leans in mouth-first, the top of the body ahead
        (Theme::Fire, CastPhase::WindUp(f)) => Pose { dx: if f == 0 { 0.0 } else { 1.0 }, tilt: 1.0, ..p },
        (Theme::Fire, CastPhase::Release(_)) => Pose { dx: -1.0, tilt: -0.4, ..p },
        // water: a gentle sway back, then forward before the swirl forms
        (Theme::Water, CastPhase::WindUp(0)) => Pose { tilt: -0.5, ..p },
        (Theme::Water, CastPhase::WindUp(_)) => Pose { dx: 1.0, tilt: 0.5, ..p },
        (Theme::Water, CastPhase::Release(f)) => Pose { dx: -1.0, tilt: if f == 0 { -0.5 } else { 0.0 }, ..p },
        // poison: puffs up and leans in, then deflates back
        (Theme::Poison, CastPhase::WindUp(_)) => Pose { dx: 1.0, swell: 1.07, ..p },
        (Theme::Poison, CastPhase::Release(_)) => Pose { dx: -1.0, swell: 0.97, ..p },
        // lightning: crouches, then jolts up and back
        (Theme::Lightning, CastPhase::WindUp(_)) => Pose { dx: 1.0, dy: 1.0, ..p },
        (Theme::Lightning, CastPhase::Release(f)) => Pose { dx: -1.0, dy: if f == 0 { -1.0 } else { 0.0 }, ..p },
    }
}

/// A small four-point star that blinks with phase `ph`; never drawn over the body.
pub(super) fn twinkle(cv: &mut Canvas, x: f32, y: f32, ph: f32, c: Rgb) {
    let (px, py) = cv.to_px(x, y);
    let n = cv.n();
    let dot = |cv: &mut Canvas, ox: i32, oy: i32, c: Rgb| {
        for j in 0..n {
            for i in 0..n {
                cv.set_px(px + ox * n + i, py + oy * n + j, c, 2, true);
            }
        }
    };
    if ph > 0.75 {
        for (ox, oy) in [(0, 0), (-1, 0), (1, 0), (0, -1), (0, 1)] {
            dot(cv, ox, oy, c);
        }
    } else if ph > 0.3 {
        dot(cv, 0, 0, mul(c, 0.8));
    }
}

/// DONE: sparkles around the body.
pub(super) fn sparkles(cv: &mut Canvas, ctx: &Ctx) {
    if ctx.state != Status::Done {
        return;
    }
    let spots = [(2.0, 5.0), (20.0, 4.0), (21.0, 16.0)];
    for (i, &(x, y)) in spots.iter().enumerate() {
        let ph = (ctx.tf * 0.12 + i as f32 * 2.1).sin();
        twinkle(cv, x, y, ph, [255., 236., 150.]);
    }
}

/// Text-glyph marks: a bobbing `?` (NEEDS_INPUT) or a drifting `z`/`Z` (IDLE),
/// up and to the right of the body.
pub(super) fn marks(grid: &mut SpiritGrid, cv: &Canvas, ctx: &Ctx, pulse: f32) {
    let g = &cv.g;
    let rr = BASE_R * g.ppu;
    let qc = ((g.rcx + 0.95 * rr) / 2.0).floor() as i32;
    // at least row 1, so the mark always has a row above to bob into
    let qr = (((g.rcy - 1.05 * rr) / 4.0).floor() as i32).max(1);
    let mark = match ctx.state {
        Status::NeedsInput => {
            let r = if (ctx.tf * 0.12).sin() > 0.0 { (qr - 1).max(0) } else { qr };
            Some((qc, r, '?', lerp([255., 195., 50.], [255., 250., 200.], pulse)))
        }
        Status::Idle => {
            let zp = (ctx.t % 50) as f32 / 50.0;
            let col = lerp([180., 180., 210.], CARD_BG, zp);
            Some(if zp < 0.5 { (qc, qr, 'z', col) } else { (qc + 1, (qr - 1).max(0), 'Z', col) })
        }
        _ => None,
    };
    if let Some((c, r, ch, col)) = mark {
        grid.put(c, r, ch, col);
    }
}
