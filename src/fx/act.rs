//! An act: an effect performed, a port of pixelfx `core/act.js`. The character winds up, lets go and recovers, a target
//! takes the hit, and everything freezes for a moment on contact. Acts are data ([`Act`]); [`create_act`] plays one with
//! its effect, [`render_move`] renders a [`Move`] at a time. The freeze, the shake, the springy target and poses held on
//! 12 fps steps are how Hollow Crown plays its skills (https://github.com/rehan-remade/hollow-crown), MIT License,
//! Copyright (c) 2026 rehan-remade.
//!
//! Port notes:
//! - Upstream moves hard-code a target distance `TX`; here a [`Move`] builds its act and chain from the scene's `tx`
//!   (`act: fn(tx) -> Act`, `create: fn(tx) -> Chain`), and [`render_move`] takes `tx` (use `fx::SCENE_TX`).
//! - Upstream's default target x (66 when a target has no x) is gone: targets are explicit x positions.
//! - Act placement (`Options::x/y`) and target x are whole pixels here (upstream would index typed arrays with fractions).
//! - The `onHit` callback is not ported.

pub use super::rig::{posed, Pose, STAND};

use super::actor::{DUMMY_PAL, HERO2};
use super::chain::Chain;
use super::color::{hex, mix};
use super::effect::{create_fx, Effect, FxInstance, Meta, Options};
use super::frame::Frame;
use super::math::{clamp, Rng};
use super::painter::{Layer, Painter, SpriteOpts};
use super::rig::{js_hypot, js_round, to_i32};
use std::sync::OnceLock;

// ---- poses over time

/// How a key's pose arrives: smoothly (default), fast then settling (`Out`), slowly then fast (`In`), linearly, or
/// `Snap` (the previous pose is held and jumps on the key).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum How {
    #[default]
    Smooth,
    Out,
    In,
    Line,
    Snap,
}
impl How {
    fn ease(self, u: f64) -> f64 {
        match self {
            How::Out => 1.0 - (1.0 - u) * (1.0 - u),
            How::In => u * u,
            How::Line => u,
            How::Smooth | How::Snap => u * u * (3.0 - 2.0 * u),
        }
    }
}
/// A key on a pose track: upstream `[t, pose, how]`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Key {
    pub t: f64,
    pub pose: Pose,
    pub how: How,
}
pub fn key(t: f64, pose: Pose) -> Key {
    Key { t, pose, how: How::Smooth }
}
pub fn key_how(t: f64, pose: Pose, how: How) -> Key {
    Key { t, pose, how }
}

fn lerp(a: f64, b: f64, u: f64) -> f64 {
    a + (b - a) * u
}
fn idx(name: &str) -> usize {
    Pose::NUM_KEYS.iter().position(|k| *k == name).unwrap()
}
/// Between two poses: numbers move, everything else stays as in `a` (it changes at the later key).
pub fn lerp_pose(a: &Pose, b: &Pose, u: f64) -> Pose {
    let mut out = STAND.merge(a);
    let bb = STAND.merge(b);
    let mut on = out.nums();
    let bn = bb.nums();
    for i in 0..on.len() {
        if let Some(v) = bn[i] {
            on[i] = Some(match on[i] {
                Some(o) => lerp(o, v, u),
                None => v,
            });
        }
    }
    // what the later pose doesn't mention eases back to its default
    for k in ["hx", "hy", "lean", "crouch"] {
        let i = idx(k);
        if bn[i].is_none() {
            if let Some(o) = on[i] {
                on[i] = Some(lerp(o, 0.0, u));
            }
        }
    }
    let i = idx("wl");
    if bn[i].is_none() {
        if let Some(o) = on[i] {
            on[i] = Some(lerp(o, 1.0, u));
        }
    }
    out.set_nums(&on);
    out
}

/// The pose at time t on a track. Limbs move in whole 12 fps steps counted from each key, so they read as held
/// drawings; the feet's position over the floor (x, y) stays smooth.
pub fn pose_at(track: &[Key], t: f64) -> Pose {
    let Some(first) = track.first() else { return STAND };
    if t <= first.t {
        return first.pose;
    }
    for w in track.windows(2) {
        let (k0, k1) = (&w[0], &w[1]);
        let (t0, t1) = (k0.t, k1.t);
        if t >= t1 {
            continue;
        }
        if k1.how == How::Snap {
            return k0.pose;
        }
        let e = |u: f64| k1.how.ease(u);
        let stepped = t0 + ((t - t0) * 12.0 + 1e-6).floor() / 12.0;
        let mut out = lerp_pose(&k0.pose, &k1.pose, e((stepped - t0) / (t1 - t0)));
        let u = e((t - t0) / (t1 - t0));
        out.x = Some(lerp(k0.pose.x.unwrap_or(0.0), k1.pose.x.unwrap_or(0.0), u));
        out.y = Some(lerp(k0.pose.y.unwrap_or(0.0), k1.pose.y.unwrap_or(0.0), u));
        return out;
    }
    track[track.len() - 1].pose
}

/// Is the body between keys that hardly differ, or past the last?
fn is_held(track: &[Key], t: f64) -> bool {
    let (Some(first), Some(last)) = (track.first(), track.last()) else { return true };
    if t >= last.t || t < first.t {
        return true;
    }
    for w in track.windows(2) {
        if t >= w[1].t {
            continue;
        }
        let (a, b) = (STAND.merge(&w[0].pose), STAND.merge(&w[1].pose));
        let (an, bn) = (a.nums(), b.nums());
        return w[1].t - w[0].t > 0.4
            && ["fx", "fy", "bx", "by", "crouch", "lean", "x", "y"].iter().all(|k| {
                let i = idx(k);
                (an[i].unwrap_or(0.0) - bn[i].unwrap_or(0.0)).abs() < 1.5
            });
    }
    true
}

// ---- the target

const TW: usize = 38;
const TH: usize = 52;

/// What a hit can leave on the target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Skin {
    Flash,
    Char,
    Ice,
    Shadow,
    Holy,
    Red,
    Shock,
    Poison,
}
/// Each skin's color and how much of it: upstream `SKINS`.
pub const SKINS: [(Skin, &str, f64); 8] = [
    (Skin::Flash, "#ffffff", 1.0),
    (Skin::Char, "#1a0c0e", 0.72),
    (Skin::Ice, "#a8e8ff", 0.72),
    (Skin::Shadow, "#3a1a70", 0.5),
    (Skin::Holy, "#fff4c0", 0.5),
    (Skin::Red, "#ff2040", 0.45),
    (Skin::Shock, "#8fd6ff", 0.6),
    (Skin::Poison, "#58c43a", 0.5),
];
impl Skin {
    pub fn name(self) -> &'static str {
        match self {
            Skin::Flash => "flash",
            Skin::Char => "char",
            Skin::Ice => "ice",
            Skin::Shadow => "shadow",
            Skin::Holy => "holy",
            Skin::Red => "red",
            Skin::Shock => "shock",
            Skin::Poison => "poison",
        }
    }
    /// (color, amount).
    pub fn tint(self) -> (u32, f64) {
        let (_, h, k) = SKINS.iter().find(|(s, _, _)| *s == self).unwrap();
        (hex(h), *k)
    }
}
/// What a hit does to the target's skin: leave it (upstream: no `skin` key), clear it (`skin: null`), or set one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SkinChange {
    #[default]
    Keep,
    Clear,
    Set(Skin),
}

struct Tp {
    ink: u32,
    wood: u32,
    wood_l: u32,
    wood_d: u32,
    straw: u32,
    straw_l: u32,
    burlap_d: u32,
    burlap: u32,
    burlap_l: u32,
    red: u32,
    red_l: u32,
}
fn tp() -> Tp {
    Tp {
        ink: hex("#0b0a1a"),
        wood: hex("#724c2e"),
        wood_l: hex("#a0703a"),
        wood_d: hex("#4e331f"),
        straw: hex("#d2aa55"),
        straw_l: hex("#f0d98a"),
        burlap_d: hex("#86643a"),
        burlap: hex("#b38b57"),
        burlap_l: hex("#c9a26c"),
        red: hex("#9a2f3c"),
        red_l: hex("#c4434f"),
    }
}
/// The straw target sprite, TW x TH, built like upstream `buildDummy`.
fn dummy() -> &'static [u32] {
    static D: OnceLock<Vec<u32>> = OnceLock::new();
    D.get_or_init(|| {
        let tp = tp();
        let mut d = vec![0u32; TW * TH];
        let mut px = |x: i32, y: i32, c: u32| {
            if x >= 0 && y >= 0 && (x as usize) < TW && (y as usize) < TH {
                d[y as usize * TW + x as usize] = c;
            }
        };
        fn r(px: &mut impl FnMut(i32, i32, u32), x: i32, y: i32, w: i32, h: i32, c: u32) {
            for j in 0..h {
                for i in 0..w {
                    px(x + i, y + j, c);
                }
            }
        }
        // the post and the crossbar
        r(&mut px, 15, 38, 8, 14, tp.ink);
        r(&mut px, 16, 34, 6, 18, tp.wood);
        r(&mut px, 16, 34, 2, 18, tp.wood_l);
        r(&mut px, 20, 34, 2, 18, tp.wood_d);
        r(&mut px, 1, 18, 36, 1, tp.ink);
        r(&mut px, 1, 23, 36, 1, tp.ink);
        r(&mut px, 1, 19, 36, 4, tp.wood);
        r(&mut px, 1, 19, 36, 1, tp.wood_l);
        r(&mut px, 1, 22, 36, 1, tp.wood_d);
        // straw bursting out of the ends of the arms
        for (bx, s) in [(0, -1), (33, 1)] {
            r(&mut px, bx, 17, 5, 8, tp.straw);
            for (dx, dy, c) in [
                (2, 16, tp.straw),
                (3, 15, tp.straw_l),
                (1, 17, tp.straw_l),
                (2, 25, tp.straw),
                (1, 26, tp.straw_l),
                (3, 25, tp.burlap_d),
                (2, 19, tp.straw_l),
                (3, 21, tp.straw_l),
                (1, 22, tp.burlap_d),
                (2, 23, tp.straw_l),
            ] {
                px(bx + dx, dy, c);
            }
            px(if s < 0 { bx - 1 } else { bx + 5 }, 19, tp.straw);
            px(if s < 0 { bx - 1 } else { bx + 5 }, 22, tp.straw_l);
        }
        let sack = |px: &mut dyn FnMut(i32, i32, u32), x: i32, y: i32, w: i32, h: i32| {
            let mut rr = |x: i32, y: i32, w: i32, h: i32, c: u32| {
                for j in 0..h {
                    for i in 0..w {
                        px(x + i, y + j, c);
                    }
                }
            };
            rr(x + 1, y, w - 2, 1, tp.ink);
            rr(x + 1, y + h - 1, w - 2, 1, tp.ink);
            rr(x, y + 1, 1, h - 2, tp.ink);
            rr(x + w - 1, y + 1, 1, h - 2, tp.ink);
            rr(x + 1, y + 1, w - 2, h - 2, tp.burlap);
            rr(x + w - 4, y + 1, 3, h - 2, tp.burlap_d);
            rr(x + 1, y + h - 3, w - 2, 2, tp.burlap_d);
            rr(x + 1, y + 1, 1, h - 4, tp.burlap_l);
            rr(x + 1, y + 1, w - 6, 1, tp.burlap_l);
            // the weave
            let mut j = y + 2;
            while j < y + h - 2 {
                let mut i = x + 3 + (j % 2);
                while i < x + w - 4 {
                    px(i, j, tp.burlap_d);
                    i += 4;
                }
                j += 3;
            }
        };
        sack(&mut px, 9, 17, 20, 24);
        // the rings on its chest
        for y in 19..36 {
            for x in 11..27 {
                let dd = js_hypot(&[x as f64 - 18.5, y as f64 - 27.0]);
                if (dd > 4.0 && dd < 5.4) || dd < 1.9 {
                    px(x, y, if x + y < 45 { tp.red_l } else { tp.red });
                }
            }
        }
        // the rope belt, and straw coming out under it
        for x in 10..28 {
            px(x, 36, if x & 1 == 1 { tp.wood_l } else { tp.straw });
            px(x, 37, if x & 1 == 1 { tp.wood_d } else { tp.wood });
        }
        for (x, y, c) in [
            (11, 41, tp.straw),
            (12, 42, tp.straw_l),
            (14, 41, tp.straw_l),
            (15, 42, tp.straw),
            (23, 41, tp.straw),
            (24, 42, tp.straw_l),
            (26, 41, tp.straw),
            (13, 43, tp.straw),
            (25, 43, tp.straw),
        ] {
            px(x, y, c);
        }
        sack(&mut px, 10, 1, 18, 16);
        // X for eyes and a stitched mouth
        for ex in [14, 23] {
            for dd in -2..=2 {
                px(ex + dd, 7 + dd, tp.ink);
                px(ex - dd, 7 + dd, tp.ink);
            }
        }
        for x in 14..24 {
            px(x, 12, if x & 1 == 1 { tp.ink } else { tp.burlap_d });
        }
        for x in [15, 18, 21] {
            px(x, 11, tp.ink);
            px(x, 13, tp.ink);
        }
        // the rope at its neck, and straw for hair
        for x in 12..26 {
            px(x, 16, if x & 1 == 1 { tp.wood_l } else { tp.straw });
            px(x, 17, if x & 1 == 1 { tp.wood } else { tp.wood_d });
        }
        for (x, y, c) in [
            (13, 0, tp.straw),
            (15, 0, tp.straw_l),
            (18, 0, tp.straw),
            (21, 0, tp.straw_l),
            (24, 0, tp.straw),
            (9, 4, tp.straw),
            (28, 6, tp.straw),
            (8, 7, tp.straw_l),
            (29, 3, tp.straw_l),
        ] {
            px(x, y, c);
        }
        d
    })
}
fn straw_ramp() -> [u32; 5] {
    let tp = tp();
    [tp.straw_l, tp.straw, tp.straw, tp.burlap_d, tp.burlap_d]
}
fn dust_ramp() -> [u32; 4] {
    [hex("#efe7dc"), hex("#c9bfb8"), hex("#8f8693"), hex("#5a5468")]
}

/// A piece of straw shed by the target, or a speck of dust kicked up by the feet (local space, y up is negative).
#[derive(Clone, Copy, Debug)]
pub struct Straw {
    pub x: f64,
    pub y: f64,
    pub vx: f64,
    pub vy: f64,
    pub age: f64,
    pub life: f64,
    pub big: bool,
}

/// A straw target on a spring: it sways, flashes, takes a skin, shivers and sheds straw. `x`: the floor under it.
#[derive(Clone, Debug)]
pub struct Target {
    pub x: f64,
    pub theta: f64,
    pub omega: f64,
    pub flash_t: f64,
    pub skin: Option<Skin>,
    pub shiver: f64,
    pub clock: f64,
    rng: Rng,
    pub straw: Vec<Straw>,
}
impl Target {
    pub fn new(x: f64, seed: u32) -> Target {
        Target { x, theta: 0.0, omega: 0.0, flash_t: 0.0, skin: None, shiver: 0.0, clock: 0.0, rng: Rng::new(seed), straw: vec![] }
    }
    pub fn hit(&mut self, h: &Hit) {
        self.omega += h.push;
        self.flash_t = self.flash_t.max(h.flash);
        match h.skin {
            SkinChange::Keep => {}
            SkinChange::Clear => self.skin = None,
            SkinChange::Set(s) => self.skin = Some(s),
        }
        if h.shiver != 0.0 && !h.shiver.is_nan() {
            self.shiver = h.shiver;
        }
        let r = &mut self.rng;
        for _ in 0..h.straw {
            let back = r.next() < 0.3;
            let x = self.x + r.range(-3.0, 3.0);
            let y = h.y + r.range(-3.0, 3.0);
            let vx = if back { r.range(-60.0, -5.0) } else { r.range(10.0, 130.0) };
            let vy = r.range(-120.0, -5.0);
            let life = r.range(1.2, 2.2);
            let big = r.next() < 0.5;
            self.straw.push(Straw { x, y, vx, vy, age: 0.0, life, big });
        }
    }
    pub fn step(&mut self, dt: f64) {
        self.clock += dt;
        self.omega += (-60.0 * self.theta - 3.4 * self.omega) * dt;
        self.theta = clamp(self.theta + self.omega * dt, -0.4, 0.42);
        if self.flash_t > 0.0 {
            self.flash_t -= dt;
        }
        if self.shiver > 0.0 {
            self.shiver -= dt;
        }
        for s in self.straw.iter_mut() {
            s.age += dt;
            s.vy += 210.0 * dt;
            s.x += s.vx * dt;
            s.y += s.vy * dt;
            if s.y >= 0.0 {
                s.y = 0.0;
                s.vx *= 0.2;
                s.vy = 0.0;
            }
        }
        self.straw.retain(|s| s.age < s.life);
    }
    /// The skin showing now (a flash wins).
    pub fn showing(&self) -> Option<Skin> {
        if self.flash_t > 0.0 {
            Some(Skin::Flash)
        } else {
            self.skin
        }
    }
    /// Draw into a color buffer `w` x `h` with the act's origin at frame pixel (x0, y0).
    pub fn draw(&self, c: &mut [u32], w: usize, h: usize, x0: i32, y0: i32, flip: bool) {
        let tan = self.theta.tan();
        let sv = if self.shiver > 0.0 {
            if to_i32((self.clock * 30.0).floor()) & 1 == 1 {
                1
            } else {
                -1
            }
        } else {
            0
        };
        let sk = self.showing().map(Skin::tint);
        let tx = js_round(self.x) as i32;
        let base = x0 + if flip { -tx - 1 } else { tx };
        let d = dummy();
        for r in 0..TH {
            let o = js_round((TH - 1 - r) as f64 * tan * 0.8) as i32 + sv;
            for x in 0..TW {
                let col = d[r * TW + x];
                if col == 0 {
                    continue;
                }
                let lx = x as i32 - 19 + o;
                let px = base + if flip { -lx - 1 } else { lx };
                let py = y0 - TH as i32 + r as i32;
                if px >= 0 && py >= 0 && (px as usize) < w && (py as usize) < h {
                    c[py as usize * w + px as usize] = match sk {
                        Some((k, a)) => mix(col, k, a),
                        None => col,
                    };
                }
            }
        }
    }
    pub fn draw_straw(&self, c: &mut [u32], w: usize, h: usize, x0: i32, y0: i32, flip: bool) {
        let ramp = straw_ramp();
        for s in &self.straw {
            let col = ramp[((s.age / s.life) * 5.0).floor().clamp(0.0, 4.0) as usize];
            let (lx, ly) = (js_round(s.x) as i32, js_round(s.y) as i32 - 1);
            for i in 0..if s.big { 2 } else { 1 } {
                let px = x0 + if flip { -(lx + i) - 1 } else { lx + i };
                let py = y0 + ly;
                if px >= 0 && py >= 0 && (px as usize) < w && (py as usize) < h {
                    c[py as usize * w + px as usize] = col;
                }
            }
        }
    }
}

// ---- the act as data

/// A moment of contact. Defaults are upstream's: no freeze or shake, push 1.2, 8 straws, a 0.05 s flash, skin kept,
/// straw flies from y = -26, every target is hit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit {
    pub t: f64,
    /// Hit-stop: everything freezes this long.
    pub stop: f64,
    /// Shake duration; `amp` its size in px (2 when unset).
    pub shake: f64,
    pub amp: Option<f64>,
    pub push: f64,
    pub straw: u32,
    pub flash: f64,
    pub skin: SkinChange,
    pub shiver: f64,
    pub y: f64,
    /// false: the hit doesn't touch the target (upstream `target: false`).
    pub target: bool,
    /// Only this target (upstream `at`).
    pub at: Option<usize>,
}
impl Hit {
    pub fn new(t: f64) -> Hit {
        Hit { t, stop: 0.0, shake: 0.0, amp: None, push: 1.2, straw: 8, flash: 0.05, skin: SkinChange::Keep, shiver: 0.0, y: -26.0, target: true, at: None }
    }
    pub fn stop(mut self, v: f64) -> Hit {
        self.stop = v;
        self
    }
    pub fn shake(mut self, v: f64) -> Hit {
        self.shake = v;
        self
    }
    pub fn amp(mut self, v: f64) -> Hit {
        self.amp = Some(v);
        self
    }
    pub fn push(mut self, v: f64) -> Hit {
        self.push = v;
        self
    }
    pub fn straw(mut self, n: u32) -> Hit {
        self.straw = n;
        self
    }
    pub fn flash(mut self, v: f64) -> Hit {
        self.flash = v;
        self
    }
    pub fn skin(mut self, s: Skin) -> Hit {
        self.skin = SkinChange::Set(s);
        self
    }
    pub fn clear_skin(mut self) -> Hit {
        self.skin = SkinChange::Clear;
        self
    }
    pub fn shiver(mut self, v: f64) -> Hit {
        self.shiver = v;
        self
    }
    pub fn y(mut self, v: f64) -> Hit {
        self.y = v;
        self
    }
    pub fn no_target(mut self) -> Hit {
        self.target = false;
        self
    }
    pub fn only(mut self, i: usize) -> Hit {
        self.at = Some(i);
        self
    }
}

/// Dust kicked up at `x` on the floor: `n` specks (6), thrown towards `dir` (random each when unset), colored `c`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Dust {
    pub t: f64,
    pub x: f64,
    pub n: u32,
    pub dir: Option<f64>,
    pub c: Option<u32>,
}
impl Dust {
    pub fn new(t: f64, x: f64) -> Dust {
        Dust { t, x, n: 6, dir: None, c: None }
    }
    pub fn n(mut self, n: u32) -> Dust {
        self.n = n;
        self
    }
    pub fn dir(mut self, d: f64) -> Dust {
        self.dir = Some(d);
        self
    }
    pub fn color(mut self, c: u32) -> Dust {
        self.c = Some(c);
        self
    }
}

/// An act: upstream `export const act = { target(s), poses, hits, dust, ghost, onBody, follow }`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Act {
    /// Where the straw targets stand (floor x, local space).
    pub targets: Vec<f64>,
    /// The pose track (empty: standing easy).
    pub poses: Vec<Key>,
    pub hits: Vec<Hit>,
    pub dust: Vec<Dust>,
    /// After-image color (else 70% up the palette, else a cold blue).
    pub ghost: Option<u32>,
    /// The effect paints the standing body; carry that paint onto the pose.
    pub on_body: bool,
    /// How much of the lean and crouch an on-body effect follows (1).
    pub follow: Option<f64>,
}

/// When an act's poses begin and end, in effect time (a loop ends when it's told to).
pub fn act_span(meta: &Meta, act: &Act) -> (f64, f64) {
    let d = meta.duration;
    let from = (0.0f64).min(act.poses.first().map_or(0.0, |k| k.t));
    let to = if meta.looping { f64::INFINITY } else { d.max(act.poses.last().map_or(0.0, |k| k.t)) };
    (from, to)
}

/// An act as plain timing data (upstream `actTiming`).
#[derive(Clone, Debug, PartialEq)]
pub struct ActTiming {
    pub windup: f64,
    pub recovery: f64,
    pub on_body: bool,
    pub targets: Vec<f64>,
    pub hits: Vec<Hit>,
    pub poses: Vec<Key>,
}
pub fn act_timing(meta: &Meta, act: &Act) -> ActTiming {
    let (from, to) = act_span(meta, act);
    ActTiming {
        windup: -from,
        recovery: if meta.looping { 0.0 } else { (0.0f64).max(to - meta.duration) },
        on_body: act.on_body,
        targets: act.targets.clone(),
        hits: act.hits.iter().map(|h| if h.target { *h } else { h.push(0.0) }).collect(),
        poses: act.poses.clone(),
    }
}

/// A box in local space (feet at the origin), whole pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bounds {
    pub x0: i32,
    pub y0: i32,
    pub x1: i32,
    pub y1: i32,
}
/// The box an act needs so nothing gets cut off, sampled over its poses (renders them, so it fills the pose cache).
pub fn act_bounds(meta: &Meta, act: &Act) -> Bounds {
    let ((w, h), (ox, oy)) = (meta.size, meta.origin);
    let mut b = Bounds { x0: -ox, y0: -oy, x1: w - ox, y1: h - oy };
    let mut see = |p: &Pose| {
        let r = posed(p);
        let (x, y) = (js_round(p.x.unwrap_or(0.0)) as i32, js_round(p.y.unwrap_or(0.0)) as i32);
        b.x0 = b.x0.min(r.x0 + x - 1);
        b.x1 = b.x1.max(r.x1 + x + 1);
        b.y0 = b.y0.min(r.y0 + y - 1);
        b.y1 = b.y1.max(r.y1 + y + 1);
    };
    let k = &act.poses;
    if let (Some(first), Some(last)) = (k.first(), k.last()) {
        let mut t = first.t;
        while t <= last.t {
            see(&pose_at(k, t));
            t += 1.0 / 12.0;
        }
        for key in k {
            see(&key.pose);
        }
    } else {
        see(&STAND);
    }
    for &tx in &act.targets {
        b.x0 = b.x0.min((tx - 30.0).floor() as i32);
        b.x1 = b.x1.max((tx + 36.0).ceil() as i32);
        b.y0 = b.y0.min(-56);
    }
    b
}

// ---- the act being played

const GHOST_LIFE: f64 = 0.16;

struct Rest {
    mask: Vec<u8>,
    px: Vec<u32>,
    nearest: Vec<i16>,
}
/// The standing 24 x 38 sprite: its mask, its colors, and every cell mapped to the nearest cell on the body.
fn rest() -> &'static Rest {
    static R: OnceLock<Rest> = OnceLock::new();
    R.get_or_init(|| {
        let mut mask = vec![0u8; 24 * 38];
        let mut px = vec![0u32; 24 * 38];
        for (r, row) in HERO2.iter().enumerate() {
            for (c, ch) in row.chars().enumerate() {
                if ch != '.' {
                    mask[r * 24 + c] = 1;
                    px[r * 24 + c] = DUMMY_PAL.iter().find(|(k, _)| *k == ch).map_or(0, |(_, h)| hex(h));
                }
            }
        }
        let mut nearest = vec![-1i16; 24 * 38];
        for r in 0..38i32 {
            for c in 0..24i32 {
                let (mut best, mut bd) = (-1i32, 1e9f64);
                for y in (r - 7).max(0)..(r + 8).min(38) {
                    for x in (c - 7).max(0)..(c + 8).min(24) {
                        let d = ((x - c) * (x - c) + (y - r) * (y - r)) as f64;
                        if mask[(y * 24 + x) as usize] == 1 && d < bd {
                            bd = d;
                            best = y * 24 + x;
                        }
                    }
                }
                nearest[(r * 24 + c) as usize] = best as i16;
            }
        }
        Rest { mask, px, nearest }
    })
}
fn lum(c: u32) -> f64 {
    (0.3 * (c & 255) as f64 + 0.59 * ((c >> 8) & 255) as f64 + 0.11 * ((c >> 16) & 255) as f64) / 255.0
}
/// An effect's color over the standing body, moved onto a pixel of the pose: its color, at this pixel's own shading.
fn carry(v: u32, rest: u32, own: u32) -> u32 {
    if lum(v) > 0.9 || v >> 24 < 255 {
        return v;
    }
    let k = (0.45f64).max((1.5f64).min(lum(own) / (0.08f64).max(lum(rest))));
    let ch = |n: u32| (255.0f64).min(js_round(((v >> n) & 255) as f64 * k)) as u32;
    (255 << 24) | (ch(16) << 16) | (ch(8) << 8) | ch(0)
}

struct Ghost {
    q: Pose,
    age: f64,
}

/// An effect performed (upstream `createAct`): the hero, the targets and the effect, with hit-stop, shake and dust.
pub struct ActInstance {
    pub fx: FxInstance,
    pub meta: Meta,
    pub act: Act,
    pub targets: Vec<Target>,
    track: Vec<Key>,
    span: (f64, f64),
    x: f64,
    y: f64,
    flip: bool,
    hits: Vec<Hit>,
    rng: Rng,
    specks: Vec<Straw>,
    speck_c: Vec<u32>,
    ghosts: Vec<Ghost>,
    gc: u32,
    actor_alpha: Option<super::effect::ActorAlphaFn>,
    t: f64,
    stop: f64,
    stopped: bool,
    shake_t: f64,
    shake0: f64,
    amp: f64,
    nh: usize,
    nd: usize,
    loop_n: f64,
    g_acc: f64,
    last: Option<Pose>,
}

/// Play an act with its effect. `o`: the effect's options, with `x`, `y` placing the feet and `flip` mirroring it all.
pub fn create_act(fx: &Effect, act: Act, o: Options) -> ActInstance {
    let meta = fx.meta.clone();
    let seed = o.seed.or(meta.options.seed).unwrap_or(1);
    let ramp = o.palette.clone().or_else(|| meta.options.palette.clone());
    let gc = act.ghost.unwrap_or_else(|| match &ramp {
        Some(r) if !r.is_empty() => r[(r.len() as f64 * 0.7).floor() as usize],
        _ => hex("#8fb4ff"),
    });
    let (x, y, flip) = (o.x, o.y, o.flip);
    let inst = create_fx(fx, &o);
    let track = if act.poses.is_empty() { vec![key(0.0, STAND)] } else { act.poses.clone() };
    let span = act_span(&meta, &act);
    let targets = act.targets.iter().enumerate().map(|(i, &tx)| Target::new(tx, seed.wrapping_add(4 + i as u32))).collect();
    let mut hits = act.hits.clone();
    hits.sort_by(|a, b| a.t.partial_cmp(&b.t).unwrap_or(std::cmp::Ordering::Equal));
    let mut a = ActInstance {
        fx: inst,
        meta,
        act,
        targets,
        track,
        span,
        x,
        y,
        flip,
        hits,
        rng: Rng::new(seed.wrapping_add(9)),
        specks: vec![],
        speck_c: vec![],
        ghosts: vec![],
        gc,
        actor_alpha: fx.actor_alpha,
        t: span.0,
        stop: 0.0,
        stopped: false,
        shake_t: 0.0,
        shake0: 0.0,
        amp: 0.0,
        nh: 0,
        nd: 0,
        loop_n: 0.0,
        g_acc: 0.0,
        last: None,
    };
    a.pose_now();
    a
}

impl ActInstance {
    /// Act time: from the first pose (before the effect's own t = 0) to the last.
    pub fn t(&self) -> f64 {
        self.t
    }
    /// How far the whole picture is thrown right now, in px.
    pub fn shake(&self) -> (i32, i32) {
        if self.shake_t <= 0.0 {
            return (0, 0);
        }
        let k = self.amp * (self.shake_t / self.shake0);
        let f = (self.shake_t * 60.0).floor();
        (js_round((f * 2.4).sin() * k) as i32, js_round((f * 3.7).cos() * k * 0.7) as i32)
    }
    pub fn frozen(&self) -> bool {
        self.stop > 0.0
    }
    /// The pose he's in right now.
    pub fn pose(&self) -> Option<Pose> {
        self.last
    }
    fn pose_now(&mut self) -> Pose {
        let d = self.meta.duration;
        let tt = if self.meta.looping && self.t >= d { self.t % d } else { self.t };
        let mut q = pose_at(&self.track, tt);
        // nothing is asked of him: he breathes
        if !self.meta.looping && is_held(&self.track, tt) {
            let up = (to_i32((tt * 3.0).floor()) & 1) as f64;
            let m = STAND.merge(&q);
            q = m.crouch(q.crouch.unwrap_or(0.0) + up).fy(q.fy.or(STAND.fy).unwrap() + up).by(q.by.or(STAND.by).unwrap() + up);
        }
        self.last = Some(q);
        q
    }
    pub fn update(&mut self, dt: f64) {
        if self.shake_t > 0.0 {
            self.shake_t -= dt;
        }
        if self.stop > 0.0 {
            self.stop -= dt;
            return;
        }
        let t0 = self.t;
        self.t += dt;
        let t = self.t;
        if t >= 0.0 {
            self.fx.update(if t0 < 0.0 { t } else { dt });
        }
        let d = self.meta.duration;
        if self.meta.looping && t >= d {
            let n = (t / d).floor();
            if n != self.loop_n {
                self.loop_n = n;
                self.nh = 0;
                self.nd = 0;
            }
        }
        let tt = if self.meta.looping && t >= d { t % d } else { t };
        while self.nh < self.hits.len() && self.hits[self.nh].t <= tt {
            let h = self.hits[self.nh];
            if h.stop > 0.0 {
                self.stop = self.stop.max(h.stop);
            }
            if h.shake != 0.0 && !h.shake.is_nan() {
                self.shake_t = h.shake;
                self.shake0 = h.shake;
                self.amp = h.amp.unwrap_or(2.0);
            }
            if h.target {
                for (i, g) in self.targets.iter_mut().enumerate() {
                    if h.at.is_none() || h.at == Some(i) {
                        g.hit(&h);
                    }
                }
            }
            self.nh += 1;
        }
        while self.nd < self.act.dust.len() && self.act.dust[self.nd].t <= tt {
            let dd = self.act.dust[self.nd];
            let r = &mut self.rng;
            for _ in 0..dd.n {
                let dir = dd.dir.unwrap_or_else(|| if r.next() < 0.5 { -1.0 } else { 1.0 });
                let x = dd.x + r.range(-3.0, 3.0);
                let y = -1.0 - r.range(0.0, 2.0);
                let vx = dir * r.range(22.0, 90.0);
                let vy = r.range(-34.0, -6.0);
                let life = r.range(0.16, 0.4);
                let big = r.next() < 0.45;
                self.specks.push(Straw { x, y, vx, vy, age: 0.0, life, big });
                self.speck_c.push(dd.c.unwrap_or(0));
            }
            self.nd += 1;
        }
        for k in self.specks.iter_mut() {
            k.age += dt;
            k.vx *= 1.0 - 5.0 * dt;
            k.vy += 110.0 * dt;
            k.x += k.vx * dt;
            k.y = (-1.0f64).min(k.y + k.vy * dt);
        }
        let mut i = 0;
        while i < self.specks.len() {
            if self.specks[i].age >= self.specks[i].life {
                self.specks.remove(i);
                self.speck_c.remove(i);
            } else {
                i += 1;
            }
        }
        let q = self.pose_now();
        for g in self.targets.iter_mut() {
            g.step(dt);
        }
        // after-images while a key asks for them
        self.g_acc += dt;
        let ghost = q.ghost.unwrap_or(0.0);
        if ghost != 0.0 && !ghost.is_nan() && self.g_acc >= 1.0 / 30.0 {
            self.g_acc = 0.0;
            self.ghosts.push(Ghost { q, age: 0.0 });
            if self.ghosts.len() > 7 {
                self.ghosts.remove(0);
            }
        }
        for g in self.ghosts.iter_mut() {
            g.age += dt;
        }
        while self.ghosts.first().is_some_and(|g| g.age > GHOST_LIFE) {
            self.ghosts.remove(0);
        }
    }

    /// Paint it all into `frame`. `body: false` leaves the character out, `target: false` the targets.
    pub fn draw(&mut self, frame: &mut Frame, body: bool, target: bool) {
        let q = match self.last {
            Some(q) => q,
            None => self.pose_now(),
        };
        let (w, h) = (frame.w, frame.h);
        let active = self.t >= 0.0;
        let flip = self.flip;
        let fx0 = js_round(self.x) as i32 - frame.ox;
        let fy0 = js_round(self.y) as i32 - frame.oy;
        let draw_targets = |s: &Self, f: &mut Frame| {
            if target {
                for g in &s.targets {
                    g.draw(&mut f.color, w, h, fx0, fy0, flip);
                }
            }
        };
        let draw_straw = |s: &Self, f: &mut Frame| {
            if target {
                for g in &s.targets {
                    g.draw_straw(&mut f.color, w, h, fx0, fy0, flip);
                }
            }
        };
        let draw_dust = |s: &Self, f: &mut Frame| {
            let ramp = dust_ramp();
            for (k, &c) in s.specks.iter().zip(&s.speck_c) {
                let c = if c != 0 { c } else { ramp[((k.age / k.life) * 4.0).floor().clamp(0.0, 3.0) as usize] };
                let lx = js_round(k.x) as i32;
                let py = fy0 + js_round(k.y) as i32;
                for i in 0..if k.big { 2 } else { 1 } {
                    let px = if flip { fx0 - lx - i - 1 } else { fx0 + lx + i };
                    if px >= 0 && py >= 0 && (px as usize) < w && (py as usize) < h {
                        f.color[py as usize * w + px as usize] = c;
                    }
                }
            }
        };
        if !body {
            if active {
                self.fx.draw(frame, Some(Layer::Back));
            }
            draw_targets(self, frame);
            if active {
                self.fx.draw(frame, Some(Layer::Front));
            }
            draw_straw(self, frame);
            return;
        }
        let rx = js_round(q.x.unwrap_or(0.0)) as i32;
        let ry = js_round(q.y.unwrap_or(0.0)) as i32;
        let bx = fx0 + if flip { -rx } else { rx };
        let by = fy0 + ry;
        let r = posed(&q);
        let has_ghosts = !self.ghosts.is_empty();
        let paint = |f: &mut Frame, i: usize, color: u32| {
            let xs = r.xs[i] as i32;
            let px = if flip { bx - xs - 1 } else { bx + xs };
            let py = by + r.ys[i] as i32;
            if px < 0 || py < 0 || px as usize >= w || py as usize >= h {
                return;
            }
            let j = py as usize * w + px as usize;
            f.color[j] = color;
            // he stays crisp in front of his own after-images
            if has_ghosts {
                f.light[j] = 0;
            }
        };
        let draw_ghosts = |s: &Self, f: &mut Frame| {
            let gc = s.gc;
            for g in &s.ghosts {
                // after-images are light, not paint: they brighten what's behind them
                let gr = posed(&g.q);
                let gxr = js_round(g.q.x.unwrap_or(0.0)) as i32;
                let gx = fx0 + if flip { -gxr } else { gxr };
                let gy = fy0 + js_round(g.q.y.unwrap_or(0.0)) as i32;
                let k = 0.5 * (1.0 - g.age / GHOST_LIFE);
                let (r0, g0, b0) = ((gc & 255) as f64 * k, ((gc >> 8) & 255) as f64 * k, ((gc >> 16) & 255) as f64 * k);
                for i in 0..gr.n {
                    let xs = gr.xs[i] as i32;
                    let px = if flip { gx - xs - 1 } else { gx + xs };
                    let py = gy + gr.ys[i] as i32;
                    if px < 0 || py < 0 || px as usize >= w || py as usize >= h {
                        continue;
                    }
                    let j = py as usize * w + px as usize;
                    let d = f.light[j];
                    let ch = |n: u32, add: f64| (255.0f64).min(((d >> n) & 255) as f64 + add) as u32;
                    f.light[j] = (255 << 24) | (ch(16, b0) << 16) | (ch(8, g0) << 8) | ch(0, r0);
                }
            }
        };
        let shadow = |f: &mut Frame| {
            let sy = fy0;
            let cx = fx0 + if flip { -rx } else { rx };
            let feet = (q.ffx.unwrap_or(f64::NAN) - q.fbx.unwrap_or(f64::NAN)).abs();
            let hw = 10 + (to_i32(feet) >> 1) - 8.min(ry.abs() >> 1);
            if sy < 0 || sy as usize >= h {
                return;
            }
            for dy in 0..2 {
                for x in (-hw + dy * 2)..=(hw - dy * 2) {
                    let px = cx + x;
                    if px < 0 || px as usize >= w || (sy + dy) as usize >= h {
                        continue;
                    }
                    let i = (sy + dy) as usize * w + px as usize;
                    let c = f.color[i];
                    f.color[i] = (c & 0xff_ffff) | ((c >> 24).max(110) << 24);
                }
            }
        };
        if !self.act.on_body {
            // the effect doesn't care about the body: back, body, front
            if active {
                self.fx.draw(frame, Some(Layer::Back));
            }
            draw_targets(self, frame);
            shadow(frame);
            draw_ghosts(self, frame);
            for i in 0..r.n {
                paint(frame, i, r.color[i]);
            }
            if active {
                self.fx.draw(frame, Some(Layer::Front));
            }
            draw_straw(self, frame);
            draw_dust(self, frame);
            return;
        }
        // the effect paints the standing body: draw it there, then carry each body pixel to where the pose put it
        let rs = rest();
        let mut fa = Frame::new(w, h, frame.ox, frame.oy);
        if active {
            self.fx.draw(&mut fa, Some(Layer::Back));
        }
        let mut fb = fa.clone();
        let d = self.meta.duration;
        let tt = if self.meta.looping {
            if self.t < 0.0 {
                0.0
            } else {
                self.t % d
            }
        } else {
            clamp(self.t, 0.0, d)
        };
        let alpha = if active { self.actor_alpha.map_or(1.0, |f| f(tt, &self.fx.options)) } else { 1.0 };
        if alpha > 0.0 {
            // the stock 24 x 38 stand-in, feet at the act's origin (world coordinates)
            let pal: Vec<(char, u32)> = DUMMY_PAL.iter().map(|&(c, h)| (c, hex(h))).collect();
            let mut p = Painter::new(&mut fb);
            p.place(js_round(self.x), js_round(self.y), flip, 0.0);
            p.sprite(&HERO2, &pal, -12.0, -38.0, &SpriteOpts { a: alpha, ..SpriteOpts::default() });
        }
        if active {
            self.fx.draw(&mut fb, Some(Layer::Front));
        }
        let follow = self.act.follow.unwrap_or(1.0);
        let lean = q.lean.unwrap_or(0.0) * follow;
        let cr = q.crouch.unwrap_or(0.0) * follow;
        // what's up on the body goes with it; what's on the floor stays on the floor
        let shift_of = |ly: i32| {
            let k = clamp((-11.0 - ly as f64) / 7.0, 0.0, 1.0);
            let f = clamp((-1.0 - ly as f64) / 6.0, 0.0, 1.0);
            (js_round(lean * k + rx as f64 * f) as i32, js_round(cr * k + ry as f64 * f) as i32)
        };
        let is_rest = |sx: i32, sy: i32| {
            let c = (if flip { fx0 - sx - 1 } else { sx - fx0 }) + 12;
            let r0 = sy - fy0 + 38;
            (0..24).contains(&c) && (0..38).contains(&r0) && rs.mask[(r0 * 24 + c) as usize] == 1
        };
        let copy = |f: &mut Frame, light: bool, sx: i32, sy: i32, s: u32| {
            let (dx, dy) = shift_of(sy - fy0);
            let px = sx + if flip { -dx } else { dx };
            let py = sy + dy;
            if px < 0 || py < 0 || px as usize >= w || py as usize >= h {
                return;
            }
            let i = py as usize * w + px as usize;
            if light {
                let d = f.light[i];
                let ch = |n: u32| (255u32).min(((d >> n) & 255) + ((s >> n) & 255));
                f.light[i] = (255 << 24) | (ch(16) << 16) | (ch(8) << 8) | ch(0);
            } else if s >> 24 == 255 || f.color[i] == 0 {
                f.color[i] = s;
            } else {
                let k = (s >> 24) as f64 / 255.0;
                let d = f.color[i];
                let ch = |n: u32| js_round(((s >> n) & 255) as f64 * k + ((d >> n) & 255) as f64 * (1.0 - k)) as u32;
                f.color[i] = ((s >> 24).max(d >> 24) << 24) | (ch(16) << 16) | (ch(8) << 8) | ch(0);
            }
        };
        draw_targets(self, frame);
        shadow(frame);
        for sy in 0..h as i32 {
            for sx in 0..w as i32 {
                let c = fa.color[sy as usize * w + sx as usize];
                if c != 0 {
                    copy(frame, false, sx, sy, c);
                }
            }
        }
        draw_ghosts(self, frame);
        let rest_index = |s: usize| -> Option<usize> {
            let (c, r0) = ((s % 24) as i32, (s / 24) as i32);
            let sx = if flip { fx0 - (c - 12) - 1 } else { fx0 + c - 12 };
            let sy = fy0 + r0 - 38;
            (sx >= 0 && sy >= 0 && (sx as usize) < w && (sy as usize) < h).then(|| sy as usize * w + sx as usize)
        };
        for i in 0..r.n {
            let s = if r.src[i] >= 0 { rs.nearest[r.src[i] as usize] } else { -1 };
            if s < 0 {
                if alpha > 0.5 {
                    paint(frame, i, r.color[i]);
                }
                continue;
            }
            let s = s as usize;
            let v = rest_index(s).map_or(rs.px[s], |j| fb.color[j]);
            if v == 0 {
                continue;
            }
            // the effect left this bit of body alone, or changed it: carry the change, on top of this pixel's own shading
            paint(frame, i, if v == rs.px[s] { r.color[i] } else { carry(v, rs.px[s], r.color[i]) });
        }
        for sy in 0..h as i32 {
            for sx in 0..w as i32 {
                let j = sy as usize * w + sx as usize;
                if fb.color[j] != 0 && fb.color[j] != fa.color[j] && !is_rest(sx, sy) {
                    copy(frame, false, sx, sy, fb.color[j]);
                }
                if fb.light[j] & 0xff_ffff != 0 {
                    copy(frame, true, sx, sy, fb.light[j]);
                }
            }
        }
        draw_straw(self, frame);
        draw_dust(self, frame);
    }
    /// Let a loop end: it fades out and `done()` goes true.
    pub fn stop(&mut self) {
        self.fx.stop();
        self.stopped = true;
    }
    /// One-shots end when the last pose is over.
    pub fn done(&self) -> bool {
        if self.stopped {
            self.fx.done()
        } else {
            self.t >= self.span.1 && self.stop <= 0.0
        }
    }
}

// ---- moves

/// A move: a hero act with a chain of effects (upstream `src/fx/moves/*.js`: `meta`, `act`, `create = chain([...])`).
/// `act` and `create` are built for a scene's target distance `tx` (upstream `TX`).
#[derive(Clone)]
pub struct Move {
    pub meta: Meta,
    pub act: fn(f64) -> Act,
    pub create: fn(f64) -> Chain,
}
impl Move {
    /// The move as one effect (its chain), for `tx`.
    pub fn effect(&self, tx: f64) -> Effect {
        Effect::composite(self.meta.clone(), (self.create)(tx))
    }
    /// The local-space box [`render_move`] renders, for `tx`.
    pub fn bounds(&self, tx: f64) -> Bounds {
        act_bounds(&self.meta, &(self.act)(tx))
    }
    /// Start playing the move (feet at local (0, 0)); step it with `update(1 / fps)` and `draw` it yourself to animate
    /// without replaying from the start every frame.
    pub fn start(&self, seed: u32, tx: f64) -> ActInstance {
        create_act(&self.effect(tx), (self.act)(tx), Options::new().with_seed(seed))
    }
}

/// The most updates [`render_move`] replays (two minutes at 30 fps); later times show that moment.
pub const MAX_STEPS: usize = 30 * 120;

/// Render a move `t` seconds (wall time, freezes included) after it starts: hero, target(s) at `tx`, effects.
///
/// The frame covers [`act_bounds`] in local space with the hero's feet at world (0, 0): pixel (i, j) is world
/// (`ox + i`, `oy + j`), so callers crop or place it with `ox`/`oy`. A hit's shake moves the origin by the shake offset
/// (`ox += sx`, `oy += sy`) instead of shifting pixels. The move is replayed from its first pose at `meta.fps`
/// (`floor(t * fps)` updates), so this costs O(t); animate with [`Move::start`] for long runs.
/// `t` < 0 or NaN renders the first pose.
pub fn render_move(mv: &Move, t: f64, seed: u32, tx: f64) -> Frame {
    let act = (mv.act)(tx);
    let b = act_bounds(&mv.meta, &act);
    let mut frame = Frame::new((b.x1 - b.x0).max(1) as usize, (b.y1 - b.y0).max(1) as usize, b.x0, b.y0);
    let mut a = create_act(&mv.effect(tx), act, Options::new().with_seed(seed));
    let fps = if mv.meta.fps.is_finite() && mv.meta.fps > 0.0 { mv.meta.fps } else { 30.0 };
    let steps = if t > 0.0 { ((t * fps + 1e-9).floor()).min(MAX_STEPS as f64) as usize } else { 0 };
    for _ in 0..steps {
        a.update(1.0 / fps);
    }
    a.draw(&mut frame, true, true);
    let (sx, sy) = a.shake();
    frame.ox += sx;
    frame.oy += sy;
    frame
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fx::chain::{chain, Cue, Pos};
    use crate::fx::color::hex;
    use crate::fx::effect::{Effect, Meta, Options};
    use crate::fx::frame::{composite_bytes, Bg, Frame};
    use crate::fx::painter::Painter;
    use crate::fx::poses::READY;
    use crate::fx::rig::{self, Pose, Weapon};
    use serde_json::Value;

    // ---- mirrors tools/golden/act.mjs
    const THROW: f64 = 0.95;
    const FLY: f64 = 0.46;
    const HIT: f64 = THROW + FLY;
    const TX: f64 = 70.0;

    fn hold() -> Pose {
        READY.x(-3.0).crouch(4.0).lean(2.0).fx(18.0).fy(-22.0).bx(11.0).by(-17.0).ffx(10.0).fbx(-8.0)
    }
    fn thrown() -> Pose {
        READY.x(9.0).crouch(4.0).lean(6.0).fx(19.0).fy(-22.0).bx(-6.0).by(-24.0).ffx(13.0).fbx(-10.0).toe_b(true)
    }
    fn flinch() -> Pose {
        Pose::EMPTY.turn(0.9).x(7.0).crouch(5.0).lean(-3.0).fx(-4.0).fy(-36.0).bx(5.0).by(-33.0).ffx(4.0).fbx(-4.0)
    }
    fn fireball_track() -> Vec<Key> {
        vec![
            key(-0.42, READY.x(-3.0)),
            key(0.08, hold()),
            key(0.5, hold()),
            key(THROW - 0.03, hold().crouch(5.0).lean(-2.0).fx(17.0).bx(5.0).by(-20.0)),
            key_how(THROW, thrown(), How::Snap),
            key(HIT - 0.02, thrown().crouch(3.0).lean(4.0)),
            key_how(HIT, flinch(), How::Snap),
            key(HIT + 0.7, flinch().crouch(3.0).lean(-2.0)),
            key(HIT + 1.4, READY.x(4.0)),
            key(HIT + 2.1, READY.x(-3.0)),
            key(HIT + 5.4, READY.x(-3.0)),
        ]
    }
    fn test_meta(duration: f64) -> Meta {
        Meta::new("test", (120, 70), (30, 62), duration)
    }
    fn ready_move() -> Move {
        Move {
            meta: test_meta(1.0),
            act: |tx| Act { targets: vec![tx], poses: vec![key(0.0, READY)], ..Act::default() },
            create: |_| chain(vec![]),
        }
    }
    fn throw_act(tx: f64) -> Act {
        Act {
            targets: vec![tx],
            poses: fireball_track(),
            hits: vec![
                Hit::new(HIT).stop(0.18).shake(0.38).amp(3.0).push(3.4).straw(34).skin(Skin::Char),
                Hit::new(HIT + 4.8).push(0.0).straw(0).flash(0.0).clear_skin(),
            ],
            dust: vec![Dust::new(THROW, 13.0).n(7).dir(1.0)],
            ..Act::default()
        }
    }
    fn throw_move() -> Move {
        Move { meta: test_meta(HIT + 5.4), act: throw_act, create: |_| chain(vec![]) }
    }

    fn pose_from_json(v: &Value) -> Pose {
        let mut p = Pose::EMPTY;
        for (k, x) in v.as_object().unwrap() {
            if let Some(i) = Pose::NUM_KEYS.iter().position(|n| n == k) {
                p.set_num(i, x.as_f64());
            } else if let Some(i) = Pose::FLAG_KEYS.iter().position(|n| n == k) {
                p.set_flag(i, x.as_bool());
            } else if k == "weapon" {
                p.weapon = Weapon::from_name(x.as_str().unwrap());
            } else if k == "offWeapon" {
                p.off_weapon = Weapon::from_name(x.as_str().unwrap());
            } else {
                panic!("unknown pose key {k}");
            }
        }
        p
    }
    fn check_pose(what: &str, want: &Value, got: &Pose) {
        let w = pose_from_json(want);
        let (a, b) = (w.nums(), got.nums());
        for i in 0..Pose::NUM_KEYS.len() {
            match (a[i], b[i]) {
                (None, None) => {}
                (Some(x), Some(y)) => assert!((x - y).abs() <= 1e-9, "{what}: {} want {x} got {y}", Pose::NUM_KEYS[i]),
                (x, y) => panic!("{what}: {} want {x:?} got {y:?}", Pose::NUM_KEYS[i]),
            }
        }
        assert_eq!(w.flags(), got.flags(), "{what}: flags");
        assert_eq!((w.weapon, w.off_weapon), (got.weapon, got.off_weapon), "{what}: weapons");
    }

    #[test]
    fn pose_at_matches_upstream() {
        let g: Value = serde_json::from_str(include_str!("../../tests/golden/act_poses.json")).unwrap();
        let track = fireball_track();
        let samples = g["samples"].as_array().unwrap();
        assert!(samples.len() > 100);
        for s in samples {
            let t = s["t"].as_f64().unwrap();
            check_pose(&format!("pose_at({t})"), &s["pose"], &pose_at(&track, t));
        }
        for l in g["lerps"].as_array().unwrap() {
            let (a, b, u) = (pose_from_json(&l["a"]), pose_from_json(&l["b"]), l["u"].as_f64().unwrap());
            check_pose(&format!("lerp_pose(u={u})"), &l["pose"], &lerp_pose(&a, &b, u));
        }
    }

    /// Compare a render against a fixture: same size and origin, at most 1% of pixels off by more than 2 per channel.
    fn check_frame(f: &Frame, golden: &[u8]) {
        let rd = |i: usize| u32::from_le_bytes(golden[i..i + 4].try_into().unwrap());
        let (w, h, ox, oy) = (rd(0) as usize, rd(4) as usize, rd(8) as i32, rd(12) as i32);
        assert_eq!((f.w, f.h, f.ox, f.oy), (w, h, ox, oy), "frame size and origin");
        let want = &golden[16..];
        let got = composite_bytes(f, Bg::Flat(hex("#201020")), 1.0);
        assert_eq!(got.len(), want.len());
        let bad = got.chunks(4).zip(want.chunks(4)).filter(|(g, x)| g.iter().zip(x.iter()).any(|(a, b)| (*a as i32 - *b as i32).abs() > 2)).count();
        let ratio = bad as f64 / (w * h) as f64;
        assert!(ratio <= 0.01, "{bad} of {} pixels differ ({ratio})", w * h);
        assert!(f.color.iter().filter(|&&c| c != 0).count() > 300, "hero and target drawn");
    }

    #[test]
    fn ready_frame_matches_upstream() {
        rig::clear_cache();
        check_frame(&render_move(&ready_move(), 0.0, 1, TX), include_bytes!("../../tests/golden/act_ready.bin"));
    }
    #[test]
    fn throw_frame_matches_upstream() {
        rig::clear_cache();
        check_frame(&render_move(&throw_move(), 45.0 / 30.0, 1, TX), include_bytes!("../../tests/golden/act_throw.bin"));
    }
    #[test]
    fn hit_frame_matches_upstream() {
        rig::clear_cache();
        check_frame(&render_move(&throw_move(), 66.0 / 30.0, 1, TX), include_bytes!("../../tests/golden/act_hit.bin"));
    }

    // ---- a hand-written move: 3 poses, 1 hit, one trivial effect module
    fn dot_draw(p: &mut Painter, _t: f64, o: &Options) {
        p.disc(0.0, 0.0, 3.0, o.palette()[0], 1.0);
    }
    const MARK: u32 = 0xFF00_FF00;
    fn dot_fx() -> Effect {
        Effect::stateless(Meta::new("dot", (16, 16), (8, 8), 1.0).options(Options::new().with_palette(&[MARK]).with_seed(3)), dot_draw)
    }
    fn mini_act(tx: f64) -> Act {
        Act {
            targets: vec![tx],
            poses: vec![
                key(-0.2, READY),
                key(0.1, READY.fx(19.0).fy(-24.0).lean(4.0)),
                key_how(0.6, READY.x(2.0), How::Out),
            ],
            hits: vec![Hit::new(0.3).stop(0.1).shake(0.2).push(3.0).straw(10).skin(Skin::Char)],
            ..Act::default()
        }
    }
    fn mini_move() -> Move {
        Move {
            meta: test_meta(1.0),
            act: mini_act,
            create: |tx| {
                chain(vec![Cue::new(dot_fx()).t(0.1).until(0.4).x(Pos::f(move |t| 10.0 + (t - 0.1) / 0.3 * (tx - 10.0))).y(-20.0)])
            },
        }
    }

    #[test]
    fn target_flinches_on_hit() {
        let mv = mini_move();
        let fx = mv.effect(TX);
        let mut a = create_act(&fx, (mv.act)(TX), Options::new().with_seed(1));
        let dt = 1.0 / 30.0;
        while a.t() + dt < 0.3 {
            a.update(dt);
        }
        let mut before = Frame::new(200, 80, -40, -70);
        a.draw(&mut before, true, true);
        assert_eq!(a.targets[0].theta, 0.0);
        assert!(a.targets[0].skin.is_none() && a.targets[0].straw.is_empty());
        a.update(dt);
        assert!(a.frozen(), "hit-stop after contact");
        assert_eq!(a.targets[0].skin, Some(Skin::Char));
        assert_eq!(a.targets[0].straw.len(), 10);
        assert!(a.targets[0].omega > 0.0);
        for _ in 0..6 {
            a.update(dt);
        }
        assert!(a.targets[0].theta > 0.0, "the target leans away");
        let mut after = Frame::new(200, 80, -40, -70);
        a.draw(&mut after, true, true);
        // the target region (around tx) changed: tilted and charred
        let region = |f: &Frame| -> Vec<u32> {
            let mut v = vec![];
            for y in 0..f.h {
                for x in 0..f.w {
                    let wx = x as i32 + f.ox;
                    if (wx - TX as i32).abs() < 20 {
                        v.push(f.color[y * f.w + x]);
                    }
                }
            }
            v
        };
        assert_ne!(region(&before), region(&after));
        let dark = |f: &Frame| region(f).iter().filter(|&&c| c != 0 && crate::fx::color::luma(c) < 0.2).count();
        assert!(dark(&after) > dark(&before), "char skin darkens the target");
    }

    #[test]
    fn effect_runs_between_t_and_until() {
        let mv = mini_move();
        let has = |t: f64| render_move(&mv, t, 1, TX).color.contains(&MARK);
        assert!(!has(0.1), "before the cue starts");
        assert!(has(0.2 + 0.2), "while the cue runs");
        assert!(!has(1.5), "after until");
    }

    #[test]
    fn render_move_frame_size_and_origin() {
        let mv = mini_move();
        let b = act_bounds(&mv.meta, &(mv.act)(TX));
        let f = render_move(&mv, 0.0, 1, TX);
        assert_eq!((f.w as i32, f.h as i32), (b.x1 - b.x0, b.y1 - b.y0));
        assert_eq!((f.ox, f.oy), (b.x0, b.y0));
        assert!(b.x0 <= -30 && b.y0 <= -62, "covers the meta box");
        assert!(b.x1 >= TX as i32 + 36 && b.y0 <= -56, "covers the target");
        // the hero's feet sit at world (0, 0): pixels around column -ox, just above row -oy
        let (fx, fy) = ((-f.ox) as usize, (-f.oy) as usize);
        let lit = (fy - 6..fy).any(|y| (fx - 6..fx + 6).any(|x| f.color[y * f.w + x] != 0));
        assert!(lit, "feet at the origin");
        // a longer scene distance gives a wider frame
        let far = render_move(&mv, 0.0, 1, 120.0);
        assert!(far.w > f.w);
    }

    #[test]
    fn render_move_survives_any_time() {
        let mv = mini_move();
        for t in [-1.0, -0.0, 0.0, 0.5, 3.0, 1e9, f64::INFINITY, f64::NEG_INFINITY, f64::NAN] {
            let f = render_move(&mv, t, 7, TX);
            assert!(f.color.iter().any(|&c| c != 0), "t = {t}");
        }
    }

    #[test]
    fn act_span_and_timing() {
        let mv = throw_move();
        let a = throw_act(TX);
        let (from, to) = act_span(&mv.meta, &a);
        assert_eq!(from, -0.42);
        assert!((to - (HIT + 5.4)).abs() < 1e-12);
        let tm = act_timing(&mv.meta, &a);
        assert!((tm.windup - 0.42).abs() < 1e-12);
        assert_eq!(tm.hits.len(), 2);
        assert_eq!(tm.targets, vec![TX]);
    }

    #[test]
    fn on_body_effect_follows_the_pose() {
        fn paint_body(p: &mut Painter, _t: f64, _o: &Options) {
            p.rect(-4.0, -30.0, 8.0, 6.0, 0xFF00_00FF, 1.0);
        }
        let fx = Effect::composite(
            test_meta(1.0),
            chain(vec![Cue::new(Effect::stateless(Meta::new("paint", (16, 16), (8, 8), 1.0), paint_body))]),
        );
        let act = Act { on_body: true, poses: vec![key(0.0, READY.x(6.0).lean(5.0))], ..Act::default() };
        let mut a = create_act(&fx, act, Options::new());
        a.update(1.0 / 30.0);
        let mut f = Frame::new(80, 70, -40, -60);
        a.draw(&mut f, true, true);
        assert!(
            f.color.iter().any(|&c| c & 0xFF > 0x60 && (c >> 8) & 0xFF < 0x40 && (c >> 16) & 0xFF < 0x40),
            "red paint carried with the body"
        );
    }

    #[test]
    fn ghosts_light_up_behind_a_dash() {
        let act = Act { poses: vec![key(0.0, READY.ghost(1.0)), key(0.5, READY.x(30.0).ghost(1.0))], ..Act::default() };
        let fx = Effect::composite(test_meta(1.0), chain(vec![]));
        let mut a = create_act(&fx, act, Options::new());
        for _ in 0..8 {
            a.update(1.0 / 30.0);
        }
        let mut f = Frame::new(120, 70, -40, -60);
        a.draw(&mut f, true, true);
        assert!(f.light.iter().any(|&l| l & 0xFF_FFFF != 0));
    }
}
