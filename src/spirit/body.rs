//! The body: a filled, shaded silhouette whose outline comes from the element.

use crate::model::Status;

use super::canvas::{lerp, mul, Canvas, Rgb, CARD_BG};
use super::element::{self, bolt_shape};
use super::theme::Theme;
use super::{Ctx, CX, CY};

/// Red the body flickers toward while the context window is nearly full.
const WARN_RED: Rgb = [255., 45., 35.];
const WARN_MIX: f32 = 0.55;

pub(super) struct Look {
    /// body centre y in design units (after bobbing)
    pub cy: f32,
    /// NEEDS_INPUT glow pulse in [0, 1]
    pub pulse: f32,
    pub bright: f32,
    /// lightning white flash
    pub flash: bool,
    /// red warning tint is on this frame
    pub warn: bool,
}

/// Shade tone (index into the ramp) of a body dot.
/// `d`/`dl`: distance from the centre / the light spot, relative to the outline.
fn tone(ctx: &Ctx, th: f32, dist: f32, d: f32, dl: f32, lx: f32, ly: f32) -> usize {
    let ramp_tone = |d: f32, dl: f32, rim: usize| {
        if d > 0.86 {
            rim
        } else if dl > 0.78 {
            1
        } else if dl > 0.42 {
            2
        } else {
            3
        }
    };
    match ctx.elem {
        // hottest in the middle: yellow core, orange, red edges
        Theme::Fire => {
            if d > 0.84 {
                0
            } else if d > 0.66 {
                1
            } else if d > 0.4 {
                2
            } else {
                3
            }
        }
        Theme::Lightning => {
            let (core, spike) = bolt_shape(ctx, th);
            if dist > core {
                // spikes: bright, with white-hot tips
                if dist > core + 0.55 * spike {
                    3
                } else {
                    2
                }
            } else {
                ramp_tone(dist / core, (lx * lx + ly * ly).sqrt() / core, 1)
            }
        }
        _ => ramp_tone(d, dl, 0),
    }
}

pub(super) fn draw(cv: &mut Canvas, ctx: &Ctx, look: &Look) {
    let g = cv.g;
    let ramp = element::ramp(ctx.elem);
    let cy = look.cy;
    for ry in 0..g.h {
        for rx in 0..g.w {
            let u = CX + (rx as f32 + 0.5 - g.rcx) / g.ppu;
            let v = CY + (ry as f32 + 0.5 - g.rcy) / g.ppu;
            let (dx, dy) = (u - CX, v - cy);
            let dist = (dx * dx + dy * dy).sqrt();
            let th = dy.atan2(dx);
            let r = element::radius(ctx, th);
            if dist > r {
                // NEEDS_INPUT: a faint halo just outside the outline
                if look.pulse > 0.35 && dist < r + 1.4 {
                    cv.set(rx, ry, lerp(CARD_BG, ramp[1], 0.45 * look.pulse), 1, false);
                }
                continue;
            }
            let d = dist / r;
            let (lx, ly) = (u - (CX - 2.5), v - (cy - 2.5));
            let dl = (lx * lx + ly * ly).sqrt() / r;
            let tone = tone(ctx, th, dist, d, dl, lx, ly);
            let mut c = element::tint(ctx, &ramp, tone, ramp[tone], d, th, rx, ry, look.flash);
            if ctx.state == Status::Unknown {
                let grey = (c[0] + c[1] + c[2]) / 3.0;
                c = lerp(c, [grey, grey, grey], 0.65);
            }
            if look.warn {
                c = lerp(c, WARN_RED, WARN_MIX);
            }
            cv.set(rx, ry, mul(c, look.bright), 1, true);
        }
    }
}
