#![allow(dead_code)]

pub mod actor;
pub mod color;
pub mod frame;
pub mod math;
pub mod painter;
pub mod particles;
pub mod shapes;
pub mod sim;
pub mod theme;

use actor::{draw_actor, ActorKind};
use frame::Frame;

/// Scene frame size in pixels: 48 x 10 braille cells at native 1:1 scale.
pub const SCENE_W_PX: usize = 96;
pub const SCENE_H_PX: usize = 40;
/// Feet of both actors sit on this pixel row (the scene's bottom edge).
pub const GROUND_Y: i32 = SCENE_H_PX as i32;
/// Hero feet x (the hero stands at the left).
pub const HERO_X: i32 = 14;
/// Hero-to-dummy distance in pixels (upstream `TX` ~190, shortened to fit the card).
pub const SCENE_TX: f64 = 70.0;
/// Dummy feet x (the dummy stands at the right).
pub const DUMMY_X: i32 = HERO_X + SCENE_TX as i32;

/// The static standing scene: hero at the left, dummy at the right, feet on the ground line.
pub fn static_scene() -> Frame {
    let mut f = Frame::new(SCENE_W_PX, SCENE_H_PX, 0, 0);
    draw_actor(&mut f, HERO_X, GROUND_Y, ActorKind::Hero);
    draw_actor(&mut f, DUMMY_X, GROUND_Y, ActorKind::Dummy);
    f
}

#[cfg(test)]
mod tests {
    use super::*;

    /// (min x, max x, max y) of the drawn pixels in columns `x0..x1`.
    fn extent(f: &Frame, x0: usize, x1: usize) -> Option<(usize, usize, usize)> {
        let mut e: Option<(usize, usize, usize)> = None;
        for y in 0..f.h {
            for x in x0..x1 {
                if f.color[y * f.w + x] != 0 {
                    e = Some(match e {
                        None => (x, x, y),
                        Some((a, b, c)) => (a.min(x), b.max(x), c.max(y)),
                    });
                }
            }
        }
        e
    }

    #[test]
    fn static_scene_has_hero_left_dummy_right_on_one_ground_line() {
        let f = static_scene();
        assert_eq!((f.w, f.h), (SCENE_W_PX, SCENE_H_PX));
        let mid = (HERO_X + DUMMY_X) as usize / 2;
        let (hx0, hx1, hy) = extent(&f, 0, mid).expect("hero drawn");
        let (dx0, dx1, dy) = extent(&f, mid, f.w).expect("dummy drawn");
        assert_eq!(hy, SCENE_H_PX - 1, "hero feet on the ground");
        assert_eq!(dy, SCENE_H_PX - 1, "dummy feet on the ground");
        assert!(hx1 < dx0, "gap between them");
        assert!(hx0 > 0 && dx1 < SCENE_W_PX, "both fully inside the frame");
        assert_eq!(DUMMY_X - HERO_X, SCENE_TX as i32);
    }
}
