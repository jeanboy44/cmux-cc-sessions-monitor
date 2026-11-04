//! Session status as spirit mood: motion speed, activity, bobbing, brightness
//! and the state cues (`?` for input, `z` for idle, sparkles when done).

use crate::model::Status;

use super::canvas::{lerp, mul, Canvas, Rgb, CARD_BG};
use super::noise::{seed_phase, slot};
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
