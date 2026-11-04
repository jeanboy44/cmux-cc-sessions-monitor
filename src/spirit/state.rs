//! Session status as spirit mood: motion speed, activity, bobbing, brightness
//! and the state cues (`?` for input, `z` for idle, sparkles when done).

use crate::model::Status;

use super::canvas::{lerp, mul, Canvas, Geom, Rgb, CARD_BG};
use super::noise::{hf, seed_phase, slot};
use super::theme::Theme;
use super::{Ctx, SpiritGrid, Stage, BASE_R, CY};

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

/// Ambient particle rate multiplier at a context stage: a relaxed spirit
/// gives off half as many bits.
pub(super) fn activity_at(stage: Stage, s: Status) -> f32 {
    match stage {
        Stage::Relaxed => activity(s) * 0.5,
        Stage::Lively | Stage::Struggling => activity(s),
    }
}

/// The body clock (integer tick, f32 phase) that drives the outline, the
/// element's churn and its decorations: real time at stage 2, 0.4x when
/// relaxed and 2x when struggling. Cast schedules, blinks and marks keep
/// real time.
pub(super) fn body_clock(stage: Stage, t: u64) -> (u64, f32) {
    let tf = super::phase_tick(t);
    match stage {
        Stage::Lively => (t, tf),
        Stage::Relaxed => (t / 5 * 2 + t % 5 * 2 / 5, tf * 0.4),
        Stage::Struggling => (t.wrapping_mul(2), tf * 2.0),
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

/// Vertical body offset in design units at a context stage: stage 2 is
/// [`bob`]; relaxed is a slow, tiny bob; struggling pants (faster, deeper)
/// and, when idle, slumps low with heavy slow breaths.
pub(super) fn bob_at(stage: Stage, s: Status, seed: u32, tf: f32) -> f32 {
    let ph = seed_phase(seed, 0xB0B);
    match (stage, s) {
        (Stage::Lively, _) | (_, Status::Unknown) => bob(s, seed, tf),
        (Stage::Relaxed, Status::Running) => 0.4 * (tf * 0.12 + ph).sin(),
        (Stage::Relaxed, Status::NeedsInput) => 0.4 * (tf * 0.1 + ph).sin(),
        (Stage::Relaxed, Status::Idle) => 0.5 * (tf * 0.035 + ph).sin(),
        (Stage::Relaxed, Status::Done) => 0.3 - 0.6 * (tf * 0.08 + ph).sin().abs(),
        (Stage::Struggling, Status::Running) => 1.5 * (tf * 0.62 + ph).sin(),
        (Stage::Struggling, Status::NeedsInput) => 1.4 * (tf * 0.45 + ph).sin(),
        (Stage::Struggling, Status::Idle) => 0.7 + 1.2 * (tf * 0.06 + ph).sin(),
        (Stage::Struggling, Status::Done) => bob(s, seed, tf),
    }
}

/// Vertical stretch of the body (1: none): a struggling spirit squashes and
/// stretches a little with each pant or heavy breath.
pub(super) fn squash(stage: Stage, s: Status, seed: u32, tf: f32) -> f32 {
    let ph = seed_phase(seed, 0xB0B);
    match (stage, s) {
        (Stage::Struggling, Status::Running) => 1.0 + 0.07 * (tf * 0.62 + ph).cos(),
        (Stage::Struggling, Status::NeedsInput) => 1.0 + 0.06 * (tf * 0.45 + ph).cos(),
        (Stage::Struggling, Status::Idle) => 1.0 + 0.05 * (tf * 0.06 + ph).cos(),
        _ => 1.0,
    }
}

/// Horizontal trembling of a struggling, busy body in real sub-pixels (0 or 1).
pub(super) fn tremble(stage: Stage, s: Status, seed: u32, t: u64) -> f32 {
    let busy = matches!(s, Status::Running | Status::NeedsInput);
    if stage == Stage::Struggling && busy && hf(seed ^ 0x7E3B, t as u32 ^ (t >> 32) as u32) >= 0.5 {
        1.0
    } else {
        0.0
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

/// Frames of the casting wind-up before each cast and of the release from it
/// (stage 2; a struggling spirit strains longer, see [`timing`]).
const WIND_UP: u64 = 3;
const RELEASE: u64 = 2;

/// (wind-up, release) frames at a context stage. Casts are at least 12 ticks
/// apart, so the phases of two casts never overlap.
fn timing(stage: Stage) -> (u64, u64) {
    match stage {
        Stage::Struggling => (5, 3),
        Stage::Relaxed | Stage::Lively => (WIND_UP, RELEASE),
    }
}

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
    cast_phase_at(Stage::Lively, seed, t)
}

/// [`cast_phase`] at a context stage (a struggling spirit winds up for 5
/// frames and recoils for 3).
pub(super) fn cast_phase_at(stage: Stage, seed: u32, t: u64) -> Option<CastPhase> {
    let (wind_up, release) = timing(stage);
    for k in 0..release {
        if t >= k + 1 && casts_at(seed, t - k) {
            return Some(CastPhase::Release(k as u8));
        }
    }
    (1..=wind_up).find(|&k| t.checked_add(k).is_some_and(|u| casts_at(seed, u))).map(|k| CastPhase::WindUp((wind_up - k) as u8))
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
#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn pose(elem: Theme, status: Status, seed: u32, t: u64) -> Pose {
    if status != Status::Running {
        return Pose::REST;
    }
    match cast_phase(seed, t) {
        Some(phase) => phase_pose(elem, phase),
        None => Pose::REST,
    }
}

/// [`pose`] at a context stage. Relaxed: on about half the casts only, at
/// half strength. Struggling: the wind-up holds its first (gathering) look
/// for two extra frames, and the recoil is twice as strong.
pub(super) fn pose_at(stage: Stage, elem: Theme, status: Status, seed: u32, t: u64) -> Pose {
    if status != Status::Running {
        return Pose::REST;
    }
    let Some(phase) = cast_phase_at(stage, seed, t) else { return Pose::REST };
    match stage {
        Stage::Lively => phase_pose(elem, phase),
        Stage::Relaxed => {
            let cast = match phase {
                CastPhase::WindUp(f) => t.saturating_add(WIND_UP - u64::from(f)),
                CastPhase::Release(k) => t - u64::from(k),
            };
            if hf(seed ^ 0xCA11, cast as u32 ^ (cast >> 32) as u32) >= 0.5 {
                return Pose::REST;
            }
            let p = phase_pose(elem, phase);
            Pose { dx: p.dx * 0.5, dy: p.dy * 0.5, tilt: p.tilt * 0.5, swell: 1.0 + (p.swell - 1.0) * 0.5, ..p }
        }
        Stage::Struggling => match phase {
            CastPhase::WindUp(f) => phase_pose(elem, CastPhase::WindUp(f.saturating_sub(2))),
            CastPhase::Release(k @ (0 | 1)) => {
                let p = phase_pose(elem, CastPhase::Release(k));
                Pose { dx: p.dx * 2.0, dy: p.dy * 2.0, tilt: p.tilt * 1.5, swell: 1.0 + (p.swell - 1.0) * 2.0, ..p }
            }
            CastPhase::Release(_) => phase_pose(elem, CastPhase::Release(1)),
        },
    }
}

/// Stage-2 pose for one cast phase, flavoured per element.
fn phase_pose(elem: Theme, phase: CastPhase) -> Pose {
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

/// Pale blue-white of a sweat drop (never pure white: the eyes stay unique).
const SWEAT: Rgb = [175., 215., 250.];
/// A struggling spirit sweats one drop per window, at a seeded offset.
const SWEAT_WINDOW: u64 = 19;
const SWEAT_LIFE: u64 = 7;

/// Sweat drops of a struggling, busy spirit as (x, y, fade in [0, 1]) in
/// real sub-pixels: a drop starts on the upper left of the body and slides
/// down its side (about one and a half cells) while it fades. `cy`: body
/// centre y in design units. Empty at the other stages and states.
pub(super) fn sweat_drops(ctx: &Ctx, g: &Geom, cy: f32) -> Vec<(i32, i32, f32)> {
    if ctx.stage != Stage::Struggling || !matches!(ctx.state, Status::Running | Status::NeedsInput) {
        return Vec::new();
    }
    let shifted = ctx.t.wrapping_add(slot(ctx.seed, 0x5E7, 0, SWEAT_WINDOW as u32));
    let (cycle, age) = (shifted / SWEAT_WINDOW, shifted % SWEAT_WINDOW);
    if age >= SWEAT_LIFE {
        return Vec::new();
    }
    let rr = BASE_R * g.ppu;
    let cy_px = g.rcy + (cy - CY) * g.ppu;
    let start = 0.75 - 0.15 * hf(ctx.seed ^ 0x5E8, cycle as u32 ^ (cycle >> 32) as u32);
    let y = cy_px - rr * start + 0.8 * age as f32;
    // just outside the round outline at that height
    let up = ((cy_px - y) / rr).clamp(-1.0, 1.0);
    let x = g.rcx - rr * (1.0 - up * up).sqrt() - 1.2;
    vec![(x.floor() as i32, y.floor() as i32, age as f32 / SWEAT_LIFE as f32)]
}

/// Paints the sweat drops beside the body (never over it).
pub(super) fn sweat(cv: &mut Canvas, ctx: &Ctx, cy: f32) {
    let g = cv.g;
    for (x, y, fade) in sweat_drops(ctx, &g, cy) {
        cv.set_px(x, y, lerp(SWEAT, CARD_BG, fade * fade * 0.8), 1, true);
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
