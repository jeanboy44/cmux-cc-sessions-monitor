//! Eyes: each eye clears its whole cell(s) of body dots (a dark hole where
//! the card shows through) and keeps ONE white dot as the catchlight. On a
//! blink the hole shows a thin dash of muted body-tone dots instead (no
//! white) for 2 frames, every 3-5 s. Every state uses the same eye cells;
//! during a burst the dot looks one sub-pixel to the right.
//!
//! The small spirit (`Geom::dot_eyes`) has eyes in proportion to its body:
//! each is a single white dot with a one-dot dark halo (only the body dot
//! beside it is cleared), in its own cell with body between the two eyes;
//! a blink is a two-dot dash. A braille cell has one colour, so the cell
//! holding the white dot shows only that dot.

use super::canvas::{mul, Canvas, Geom, Rgb, EYE_PRIO, WHITE};
use super::noise::slot;
use super::BASE_R;

/// One blink per window, at a seeded offset: starts are 46..=74 ticks apart.
const BLINK_WINDOW: u64 = 60;
const BLINK_JITTER: u32 = 15;
const BLINK_TICKS: u64 = 2;

/// Whether the eyes are shut on tick `t`.
pub(super) fn blinking(seed: u32, t: u64) -> bool {
    let ph = t % BLINK_WINDOW;
    let start = slot(seed, 0xB117, t / BLINK_WINDOW, BLINK_JITTER);
    (start..start + BLINK_TICKS).contains(&ph)
}

/// Eye boxes in terminal cells: (col0, row0) of each eye, box width and height.
struct EyeSet {
    eyes: [(i32, i32); 2],
    ew: i32,
    eh: i32,
}

fn eye_set(g: &Geom) -> EyeSet {
    let k = g.ppu * BASE_R / 5.5;
    let (rx, ry) = (k, 1.6 * k);
    let rr = BASE_R * g.ppu;
    let ew = (rx.round() as i32).max(1);
    let eh = ((2.0 * ry / 4.0 - 0.15).ceil() as i32).max(1);
    let sep = 0.38 * rr;
    let lcol = ((g.rcx - sep) / 2.0 - ew as f32 / 2.0).round() as i32;
    let mut rcol = g.rcx as i32 - lcol - ew;
    if g.dot_eyes {
        // a body cell between the two eyes
        rcol = rcol.max(lcol + ew + 1);
    }
    let row0 = ((g.rcy - 0.25 * rr) / 4.0 - eh as f32 / 2.0).round() as i32;
    EyeSet { eyes: [(lcol, row0), (rcol, row0)], ew, eh }
}

/// Both eye boxes in terminal cells: (col0, row0, width, height).
pub(super) fn boxes(cv: &Canvas) -> [(i32, i32, i32, i32); 2] {
    let es = eye_set(&cv.g);
    es.eyes.map(|(c, r)| (c, r, es.ew, es.eh))
}

/// What one eye does to the canvas, in sub-pixels: the body dots it clears,
/// its white dot (none while blinking) and the dash dots of a blink.
#[cfg_attr(test, derive(Debug))]
pub(super) struct EyePlan {
    pub(super) clear: Vec<(i32, i32)>,
    pub(super) white: Option<(i32, i32)>,
    pub(super) dash: Vec<(i32, i32)>,
}

/// Both eyes on a canvas of geometry `g`. `facing`: the dot looks one
/// sub-pixel right (casting).
pub(super) fn plan(g: &Geom, facing: bool, blink: bool) -> [EyePlan; 2] {
    let es = eye_set(g);
    es.eyes.map(|(col0, row0)| {
        let (x0, y0) = (col0 * 2, row0 * 4);
        let (w, h) = (es.ew * 2, es.eh * 4);
        if g.dot_eyes {
            // the dot sits on the cell's second dot row; the halo is the
            // body dot beside it in the same cell
            let (x, y) = (x0 + i32::from(facing), y0 + 1);
            let beside = (x0 + i32::from(!facing), y);
            return if blink {
                EyePlan { clear: Vec::new(), white: None, dash: vec![(x0, y), (x0 + 1, y)] }
            } else {
                EyePlan { clear: vec![beside], white: Some((x, y)), dash: Vec::new() }
            };
        }
        let clear = (y0..y0 + h).flat_map(|py| (x0..x0 + w).map(move |px| (px, py))).collect();
        if blink {
            let y = y0 + h / 2;
            EyePlan { clear, white: None, dash: (x0..x0 + w).map(|px| (px, y)).collect() }
        } else {
            // the dot stays inside the cleared box
            let x = (x0 + w / 4 + i32::from(facing)).min(x0 + w - 1);
            EyePlan { clear, white: Some((x, y0 + 1)), dash: Vec::new() }
        }
    })
}

/// Punches both eyes into the canvas. `facing`: the dot looks one sub-pixel
/// right (casting); `body` is the body's dark tone (for the blink dash),
/// `bright` the state brightness.
pub(super) fn draw(cv: &mut Canvas, facing: bool, blink: bool, body: Rgb, bright: f32) {
    let dash = mul(body, 0.55 * bright);
    for eye in plan(&cv.g, facing, blink) {
        for &(px, py) in &eye.clear {
            cv.clear_px(px, py);
        }
        for &(px, py) in &eye.dash {
            cv.set_px(px, py, dash, 2, false);
        }
        if let Some((x, y)) = eye.white {
            cv.set_px(x, y, WHITE, EYE_PRIO, false);
        }
    }
}
