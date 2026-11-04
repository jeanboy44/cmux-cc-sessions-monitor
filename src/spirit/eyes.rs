//! Eyes: each eye clears its whole cell(s) of body dots (a dark hole where
//! the card shows through) and keeps ONE white dot as the catchlight. On a
//! blink the hole shows a thin dash of muted body-tone dots instead (no
//! white) for 2 frames, every 3-5 s. Every state uses the same eye cells;
//! while casting, the dot looks one sub-pixel toward the magic (right).

use super::canvas::{mul, Canvas, Rgb, EYE_PRIO, WHITE};
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

fn eye_set(cv: &Canvas) -> EyeSet {
    let g = &cv.g;
    let k = g.ppu * BASE_R / 5.5;
    let (rx, ry) = (k, 1.6 * k);
    let rr = BASE_R * g.ppu;
    let ew = (rx.round() as i32).max(1);
    let eh = ((2.0 * ry / 4.0 - 0.15).ceil() as i32).max(1);
    let sep = 0.38 * rr;
    let lcol = ((g.rcx - sep) / 2.0 - ew as f32 / 2.0).round() as i32;
    let rcol = g.rcx as i32 - lcol - ew;
    let row0 = ((g.rcy - 0.25 * rr) / 4.0 - eh as f32 / 2.0).round() as i32;
    EyeSet { eyes: [(lcol, row0), (rcol, row0)], ew, eh }
}

/// Punches both eyes into the canvas. `facing`: the dot looks one sub-pixel
/// right (casting); `body` is the body's dark tone (for the blink dash),
/// `bright` the state brightness.
pub(super) fn draw(cv: &mut Canvas, facing: bool, blink: bool, body: Rgb, bright: f32) {
    let es = eye_set(cv);
    let dash = mul(body, 0.55 * bright);
    for &(col0, row0) in &es.eyes {
        let (x0, y0) = (col0 * 2, row0 * 4);
        let (w, h) = (es.ew * 2, es.eh * 4);
        for py in y0..y0 + h {
            for px in x0..x0 + w {
                cv.clear_px(px, py);
            }
        }
        if blink {
            let y = y0 + h / 2;
            for px in x0..x0 + w {
                cv.set_px(px, y, dash, 2, false);
            }
        } else {
            // the dot stays inside the cleared box
            let x = (x0 + w / 4 + i32::from(facing)).min(x0 + w - 1);
            cv.set_px(x, y0 + 1, WHITE, EYE_PRIO, false);
        }
    }
}
