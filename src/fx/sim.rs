//! Simulated particles, debris and lights: a port of pixelfx `core/sim.js` (adapted upstream from Sprite
//! Fusion's open-source pixel art VFX, MIT, Copyright (c) 2026 Hugo Duprez). Units are art pixels and seconds;
//! everything draws through a `Painter`. The order of `Rng` draws matches upstream, so a seeded system
//! evolves the same way.
//!
//! Differences from JS: `Blazes` does not hold the particle system, `add` / `update` take `&mut Particles`
//! instead; `on_land` receives the system too (so it can feed a `Blazes` it shares through `Rc<RefCell<_>>`);
//! kinds and flags are `u8` / `u32` associated constants (`P::SPARK`, `PF::BIG`, `B::SHARD`); colors are
//! packed `u32` rather than hex strings; a `col` of 0 still means "use the kind's own ramp".

use crate::fx::color::{hex, mix, scale, WHITE};
use crate::fx::math::{clamp, Rng, TAU};
use crate::fx::painter::Painter;

/// px/s².
pub const GRAVITY: f64 = 950.0;
const CROSS: [(f64, f64); 4] = [(1.0, 0.0), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0)];
const DIAG: [(f64, f64); 4] = [(1.0, 1.0), (1.0, -1.0), (-1.0, 1.0), (-1.0, -1.0)];

/// `0xRRGGBB` to the packed `0xAABBGGRR` used everywhere else, usable in consts.
const fn c(rgb: u32) -> u32 {
    0xFF00_0000 | ((rgb & 0xFF) << 16) | (rgb & 0xFF00) | ((rgb >> 16) & 0xFF)
}
const HOT: [u32; 4] = [c(0xfff3a6), c(0xffe26b), c(0xff8a2a), c(0xf2561d)];
/// Fire from white-hot to smouldering, and smoke from fresh to thin: the ramps FIRE puffs and SMOKE walk down.
pub const FIRE_RAMP: [u32; 9] = [
    c(0xffffff),
    c(0xfff3a6),
    c(0xffe26b),
    c(0xffc23d),
    c(0xff8a2a),
    c(0xf2561d),
    c(0xc9281f),
    c(0x8a1f17),
    c(0x4a1712),
];
pub const SMOKE_RAMP: [u32; 5] = [c(0x7b6f72), c(0x5d5660), c(0x433d48), c(0x2b2630), c(0x1a171e)];

fn ramp_at(r: &[u32], t: f64) -> u32 {
    r[(t * r.len() as f64).floor().max(0.0).min((r.len() - 1) as f64) as usize]
}

/// Particle kinds.
pub struct P;
impl P {
    pub const SPARK: u8 = 0;
    pub const MOTE: u8 = 1;
    pub const STREAK: u8 = 2;
    pub const FLAKE: u8 = 3;
    pub const VAPOUR: u8 = 4;
    pub const DUST: u8 = 5;
    pub const STAR: u8 = 6;
    pub const EMBER: u8 = 7;
    pub const SMOKE: u8 = 8;
    pub const FIRE: u8 = 9;
    pub const BRAND: u8 = 10;
    pub const GRIT: u8 = 11;
}
/// Particle flags: SMOKES (a FIRE puff leaves smoke when it dies), BIG (bigger, slower smoke), BACK (draw behind).
pub struct PF;
impl PF {
    pub const SMOKES: u32 = 1;
    pub const BIG: u32 = 2;
    pub const BACK: u32 = 4;
}
fn soft(k: u8) -> bool {
    matches!(k, P::VAPOUR | P::DUST | P::SMOKE)
}

#[derive(Clone, Copy, Debug)]
pub struct Particle {
    pub k: u8,
    pub x: f64,
    pub y: f64,
    pub vx: f64,
    pub vy: f64,
    pub life: f64,
    pub max: f64,
    pub col: u32,
    pub size: f64,
    pub seed: f64,
    pub delay: f64,
    pub flag: u32,
}

/// Which half of the particles `Particles::draw` paints: `Back` the soft kinds (vapour, dust, smoke) and
/// anything flagged BACK, `Front` the rest, `Both` all (upstream: no layer).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DrawLayer {
    Both,
    Back,
    Front,
}

/// Options for `Particles::new`. `seed` 0 is the same as 1 (upstream's default); `rng` wins over `seed`.
#[derive(Default)]
pub struct ParticlesOpts {
    pub seed: u32,
    pub rng: Option<Rng>,
    /// Floor y for a given x (local px); sparks and grit bounce on it. Default: no floor.
    pub ground: Option<Box<dyn Fn(f64) -> f64>>,
    /// SPARK / EMBER colors, head to tail. Default: the hot ramp.
    pub sparks: Option<Vec<u32>>,
    /// The color vapour glows with.
    pub haze: Option<u32>,
    /// Called as `(system, x, y)` when a BRAND hits the floor, in the middle of `update`.
    #[allow(clippy::type_complexity)]
    pub on_land: Option<Box<dyn FnMut(&mut Particles, f64, f64)>>,
}

/// A particle system. `spawn(kind, ...)`, `update(dt)`, `draw(p, layer)`.
pub struct Particles {
    pub r: Rng,
    pub list: Vec<Particle>,
    pub t: f64,
    ground: Box<dyn Fn(f64) -> f64>,
    pub sparks: Vec<u32>,
    pub haze: Option<u32>,
    #[allow(clippy::type_complexity)]
    on_land: Option<Box<dyn FnMut(&mut Particles, f64, f64)>>,
}

impl Particles {
    pub fn new(o: ParticlesOpts) -> Self {
        Particles {
            r: o.rng.unwrap_or_else(|| Rng::new(o.seed)),
            list: Vec::new(),
            t: 0.0,
            ground: o.ground.unwrap_or_else(|| Box::new(|_| f64::INFINITY)),
            sparks: o.sparks.unwrap_or_else(|| HOT.to_vec()),
            haze: o.haze,
            on_land: o.on_land,
        }
    }
    pub fn ground(&self, x: f64) -> f64 {
        (self.ground)(x)
    }
    /// `delay`: seconds before it appears; `flag`: PF bits. `col` 0 lets FIRE and SMOKE use their own ramps.
    #[allow(clippy::too_many_arguments)]
    pub fn spawn(&mut self, k: u8, x: f64, y: f64, vx: f64, vy: f64, life: f64, col: u32, size: f64, delay: f64, flag: u32) {
        let seed = self.r.next() * 100.0;
        self.list.push(Particle { k, x, y, vx, vy, life, max: life, col, size, seed, delay, flag });
    }
    pub fn count(&self) -> usize {
        self.list.len()
    }

    pub fn update(&mut self, dt: f64) {
        self.t += dt;
        let t = self.t;
        let mut i = self.list.len();
        while i > 0 {
            i -= 1;
            if i >= self.list.len() {
                continue; // an on_land callback removed something
            }
            let mut p = self.list[i];
            if p.delay > 0.0 {
                p.delay -= dt;
                self.list[i] = p;
                continue;
            }
            p.life -= dt;
            if p.life <= 0.0 {
                if p.k == P::FIRE && p.flag & PF::SMOKES != 0 {
                    let big = p.flag & PF::BIG != 0;
                    let life = if big { self.r.range(1.3, 2.3) } else { self.r.range(0.8, 1.4) };
                    let size = p.size * if big { 0.85 } else { 0.7 };
                    self.spawn(P::SMOKE, p.x, p.y, p.vx * 0.6, p.vy * 0.6 - 6.0, life, 0, size, 0.0, if big { PF::BIG } else { 0 });
                }
                self.list.remove(i);
                continue;
            }
            let mut integrate = true;
            match p.k {
                P::FIRE => {
                    p.vy -= 95.0 * dt;
                    p.vx += (t * 17.0 + p.seed).sin() * 30.0 * dt;
                    p.vx -= p.vx * 5.0 * dt;
                    p.vy -= p.vy * 3.2 * dt;
                }
                P::GRIT => {
                    integrate = false;
                    p.vy += GRAVITY * dt;
                    p.vx -= p.vx * 0.8 * dt;
                    let g = self.ground(p.x);
                    let ny = p.y + p.vy * dt;
                    p.x += p.vx * dt;
                    if p.vy > 0.0 && p.y <= g && ny >= g {
                        p.y = g - 0.25;
                        if p.vy > 130.0 && self.r.chance(0.4) {
                            p.vy = -p.vy * 0.25;
                            p.vx = p.vx * 0.5 + self.r.range(-10.0, 10.0);
                        } else {
                            p.vx = 0.0;
                            p.vy = 0.0;
                            p.life = p.life.min(0.5);
                        }
                    } else {
                        p.y = ny;
                    }
                }
                P::BRAND => {
                    // a burning chunk: arcs, trails flame, and starts a little fire where it lands
                    integrate = false;
                    p.vy += GRAVITY * 0.8 * dt;
                    p.vx -= p.vx * 0.9 * dt;
                    if self.r.chance(0.9 * (dt * 60.0).min(1.0)) {
                        let vx = p.vx * 0.1 + self.r.range(-7.0, 7.0);
                        let life = self.r.range(0.16, 0.32);
                        let size = self.r.range(0.9, 1.7) * (0.5 + (p.life / p.max) * 0.6);
                        let flag = if self.r.chance(0.4) { PF::SMOKES } else { 0 };
                        self.spawn(P::FIRE, p.x, p.y, vx, p.vy * 0.1 - 10.0, life, 0, size, 0.0, flag);
                    }
                    let g = self.ground(p.x);
                    let ny = p.y + p.vy * dt;
                    p.x += p.vx * dt;
                    if p.vy > 0.0 && p.y <= g && ny >= g {
                        if let Some(mut cb) = self.on_land.take() {
                            cb(self, p.x, g);
                            self.on_land = Some(cb);
                        }
                        self.list.remove(i);
                        continue;
                    }
                    p.y = ny;
                }
                P::SPARK => {
                    integrate = false;
                    p.vy += GRAVITY * 0.45 * dt;
                    p.vx -= p.vx * 2.2 * dt;
                    p.vy -= p.vy * 2.2 * dt;
                    let g = self.ground(p.x);
                    let ny = p.y + p.vy * dt;
                    p.x += p.vx * dt;
                    if p.vy > 0.0 && p.y <= g && ny >= g {
                        p.vy = -p.vy * 0.45;
                        p.vx *= 0.7;
                        p.y = g - 0.25;
                    } else {
                        p.y = ny;
                    }
                }
                P::STREAK => {
                    p.vx += p.vx * 3.0 * dt;
                    p.vy += p.vy * 3.0 * dt;
                }
                P::FLAKE => {
                    p.vx -= p.vx * 2.2 * dt;
                    p.vy -= p.vy * 2.2 * dt - 35.0 * dt;
                    p.x += (self.t * 2.5 + p.seed).sin() * 7.0 * dt;
                }
                P::EMBER => {
                    p.vy += (-30.0 + (t * 3.0 + p.seed).sin() * 20.0) * dt;
                    p.vx += (t * 2.3 + p.seed).cos() * 45.0 * dt;
                    p.vx -= p.vx * 1.6 * dt;
                    p.vy -= p.vy * 1.6 * dt;
                }
                P::SMOKE => {
                    let big = p.flag & PF::BIG != 0;
                    p.vy -= (if big { 23.0 } else { 16.0 }) * dt;
                    p.vx += (t * 1.3 + p.seed).sin() * 8.0 * dt;
                    p.vx -= p.vx * 1.6 * dt;
                    p.vy -= p.vy * 1.2 * dt;
                }
                P::MOTE => {
                    p.vx -= p.vx * 0.6 * dt;
                    p.vy -= p.vy * 0.6 * dt;
                }
                _ => {
                    let drag = if soft(p.k) { 4.0 } else { 1.6 };
                    p.vx -= p.vx * drag * dt;
                    p.vy -= p.vy * drag * dt + 5.0 * dt;
                }
            }
            if integrate {
                p.x += p.vx * dt;
                p.y += p.vy * dt;
            }
            self.list[i] = p;
        }
    }

    pub fn draw(&self, p: &mut Painter, layer: DrawLayer) {
        for q in &self.list {
            if q.delay > 0.0 {
                continue;
            }
            if layer != DrawLayer::Both {
                let l = if soft(q.k) || q.flag & PF::BACK != 0 { DrawLayer::Back } else { DrawLayer::Front };
                if l != layer {
                    continue;
                }
            }
            let k = q.life / q.max;
            let (x, y) = (q.x, q.y);
            let sp = q.vx.hypot(q.vy);
            let sp = if sp == 0.0 { 1.0 } else { sp };
            let (ux, uy) = (q.vx / sp, q.vy / sp);
            match q.k {
                P::SPARK => {
                    let s = &self.sparks;
                    let head = if k > 0.6 { s[0] } else if k > 0.3 { s[1] } else { s[2] };
                    let len = (1.0 + sp * 0.022).min(9.0);
                    p.line(x - ux * len, y - uy * len, x - ux * len * 0.4, y - uy * len * 0.4, if k > 0.5 { head } else { s[s.len() - 1] }, 1.0);
                    p.line(x - ux * len * 0.45, y - uy * len * 0.45, x, y, head, 1.0);
                    p.glow(x, y, 3.0, head, 0.45 * k);
                }
                P::MOTE => {
                    let len = (0.5 + sp * 0.026).min(16.0) * q.size;
                    let a = if k < 0.35 { k / 0.35 } else { 1.0 };
                    p.line(x - ux * len, y - uy * len, x - ux * len * 0.35, y - uy * len * 0.35, scale(q.col, 0.55), a);
                    p.line(x - ux * len * 0.4, y - uy * len * 0.4, x, y, q.col, a);
                    p.glow(x, y, 2.5 + q.size, q.col, 0.4 * k);
                }
                P::STREAK => {
                    let l = clamp(sp / 30.0, 1.0, 7.0);
                    let mut d = l;
                    while d >= 0.0 {
                        let col = if d < 1.0 {
                            WHITE
                        } else if d < l * 0.5 {
                            q.col
                        } else {
                            scale(q.col, 0.6)
                        };
                        p.dot(x - ux * d, y - uy * d, col, if d < l * 0.5 { 1.0 } else { 1.2 - d / l });
                        d -= 0.5;
                    }
                    p.glow(x, y, 4.0, q.col, 0.4);
                }
                P::FLAKE => {
                    let a = (k * 3.0).min(1.0);
                    p.dot(x, y, q.col, a);
                    if q.size > 1.3 {
                        for (dx, dy) in CROSS {
                            p.dot(x + dx, y + dy, scale(q.col, 0.8), a * 0.8);
                        }
                    }
                }
                P::VAPOUR => {
                    let r = q.size * (0.6 + (1.0 - k) * 0.9);
                    p.disc(x, y, r, q.col, k.powf(1.3) * 0.45);
                    if let Some(h) = self.haze {
                        p.glow(x, y, r * 1.8, h, 0.16 * k);
                    }
                }
                P::DUST => p.disc(x, y, q.size * (0.6 + (1.0 - k) * 0.9), q.col, k.powf(1.3) * 0.7),
                P::SMOKE => {
                    // three soft discs: a body, a denser core sitting low, a lit cap while it's young
                    let age = 1.0 - k;
                    let big = q.flag & PF::BIG != 0;
                    let r = q.size * (0.65 + (age * 2.5).min(1.0) * 0.45 + age * if big { 0.5 } else { 0.4 });
                    let a = (age * 12.0).min(1.0) * (k * 2.4).min(1.0) * if big { 0.7 } else { 0.58 };
                    let col = if q.col != 0 { mix(q.col, ramp_at(&SMOKE_RAMP, age), age) } else { ramp_at(&SMOKE_RAMP, age * 1.05) };
                    p.disc(x, y, r, col, a * 0.45);
                    p.disc(x + r * 0.05, y + r * 0.06, r * 0.74, col, a * 0.62);
                    if age < 0.5 {
                        p.disc(x - r * 0.22, y - r * 0.3, r * 0.5, mix(col, WHITE, 0.12), a * 0.4 * (1.0 - age * 2.0));
                    }
                }
                P::FIRE => {
                    // a ball of flame that cools down the ramp as it rises: a darker rim, a lit core, a tongue on top
                    let age = 1.0 - k;
                    let heat = (age.powf(0.62) + (q.seed % 1.0) * 0.1).min(0.999);
                    let f = &FIRE_RAMP;
                    let n = f.len();
                    let idx = ((n - 2) as f64).min((heat * (n - 1) as f64).floor()) as usize;
                    let r = q.size * (0.55 + 0.45 * (age * 7.0).min(1.0)) * (1.0 - age * 0.55);
                    let col = f[idx];
                    if idx >= n - 2 {
                        p.disc(x, y, r, col, (k * 3.0).min(1.0) * 0.85);
                    } else {
                        p.circle(x, y, r, f[(n - 1).min(idx + 2)], 1.0);
                        p.circle(x - r * 0.16, y - r * 0.22, r * 0.78, col, 1.0);
                        if r > 1.6 {
                            p.circle(x, y - r * 0.9, r * 0.42, col, 1.0);
                        }
                        if idx <= 3 && r > 1.8 {
                            p.circle(x - r * 0.28, y - r * 0.34, r * 0.36, f[idx.saturating_sub(1)], 1.0);
                        }
                        if idx < 6 {
                            p.glow(x, y, r * 2.4 + 2.0, col, 0.18 * k);
                        }
                    }
                }
                P::BRAND => {
                    p.circle(x, y, 1.6, HOT[3], 1.0);
                    p.px(x, y, if (self.t * 40.0 + q.seed).sin() > 0.0 { HOT[0] } else { HOT[1] });
                    p.px(x + 1.0, y, HOT[1]);
                    p.glow(x, y, 7.0, HOT[2], 0.45);
                }
                P::GRIT => {
                    if k > 0.25 || k * 4.0 > 0.5 {
                        p.px(x, y, q.col);
                        if q.size >= 2.0 {
                            p.px(x + 1.0, y, q.col);
                            p.px(x, y + 1.0, scale(q.col, 0.8));
                            p.px(x + 1.0, y + 1.0, scale(q.col, 0.7));
                        }
                    }
                }
                P::EMBER => {
                    let s = &self.sparks;
                    let col = if k > 0.6 { s[0] } else if k > 0.3 { s[1] } else { s[2] };
                    if k < 0.25 && (self.t * 40.0 + q.seed).sin() < 0.0 {
                        continue;
                    }
                    p.px(x, y, col);
                    if q.size > 1.5 {
                        p.px(x, y + 1.0, s[s.len() - 1]);
                    }
                    p.glow(x, y, 3.0 + q.size, col, 0.35 * k);
                }
                _ => {
                    // STAR: a twinkling cross, arms set by size
                    let tw = 0.6 + 0.4 * (self.t * (8.0 + (q.seed % 5.0)) + q.seed).sin();
                    if q.size < 1.0 {
                        if tw * k > 0.25 {
                            p.px(x, y, if tw > 0.8 { WHITE } else { q.col });
                        }
                        p.glow(x, y, 3.0, q.col, 0.4 * k * tw);
                        continue;
                    }
                    sparkle(p, x, y, q.size * tw * (k * 3.0).min(1.0), q.col, scale(q.col, 0.75), 0.5 * k * tw);
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Blaze {
    pub x: f64,
    pub y: f64,
    pub life: f64,
    pub max: f64,
    pub acc: f64,
    pub power: f64,
}

/// Little fires burning where something landed: `add(ps, x, y, life, power)` feeds FIRE puffs (and the odd
/// ember) into a particle system, fading over its life. `update(ps, dt)`; `draw_back(p)` adds a hot pixel and a
/// glow at its foot.
#[derive(Default)]
pub struct Blazes {
    pub list: Vec<Blaze>,
}

impl Blazes {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn add(&mut self, ps: &mut Particles, x: f64, y: f64, life: f64, power: f64) {
        self.list.push(Blaze { x, y, life, max: life, acc: ps.r.next(), power });
    }
    pub fn count(&self) -> usize {
        self.list.len()
    }
    pub fn update(&mut self, ps: &mut Particles, dt: f64) {
        let mut i = self.list.len();
        while i > 0 {
            i -= 1;
            let b = &mut self.list[i];
            b.life -= dt;
            if b.life <= 0.0 {
                self.list.remove(i);
                continue;
            }
            let k = b.life / b.max;
            let intensity = b.power * if k > 0.7 { 1.0 } else { 0.35 + (k / 0.7) * 0.65 };
            b.acc += dt * (6.0 + 16.0 * intensity);
            while b.acc > 1.0 {
                let r = &mut ps.r;
                let s = r.range(1.2, 2.8) * intensity;
                let sx = b.x + r.range(-1.5, 1.5);
                let vx = r.range(-6.0, 6.0);
                let vy = r.range(-30.0, -12.0);
                let life = r.range(0.28, 0.6);
                let flag = PF::BACK | if r.chance(0.3) { PF::SMOKES } else { 0 };
                ps.spawn(P::FIRE, sx, b.y - s * 0.6, vx, vy, life, 0, s, 0.0, flag);
                if ps.r.chance(0.06) {
                    let vx = ps.r.range(-15.0, 15.0);
                    let vy = ps.r.range(-45.0, -15.0);
                    let life = ps.r.range(0.6, 1.4);
                    ps.spawn(P::EMBER, b.x, b.y - 1.5, vx, vy, life, HOT[2], 1.0, 0.0, PF::BACK);
                }
                b.acc -= 1.0;
            }
        }
    }
    pub fn draw_back(&self, p: &mut Painter) {
        for b in &self.list {
            let k = b.life / b.max;
            p.px(b.x, b.y, if k > 0.4 { if (b.life * 31.0 + b.x).sin() > 0.0 { HOT[1] } else { HOT[2] } } else { HOT[3] });
            p.glow(b.x, b.y, 6.0 + 5.0 * k * b.power, HOT[2], 0.3 * k * b.power);
        }
    }
}

/// Debris kinds.
pub struct B;
impl B {
    pub const SHARD: u8 = 0;
    pub const CHUNK: u8 = 1;
    pub const DRIP: u8 = 2;
}

/// One piece of debris. `s` is its size, `c` its color, `ramp` the chunk shading (dark to bright; `None` is
/// a cool stone ramp), `g` the gravity scale. `Default` gives upstream's defaults (a white shard of size 1,
/// life 1).
#[derive(Clone, Debug)]
pub struct Bit {
    pub kind: u8,
    pub x: f64,
    pub y: f64,
    pub vx: f64,
    pub vy: f64,
    pub a: f64,
    pub va: f64,
    pub s: f64,
    pub c: u32,
    pub ramp: Option<Vec<u32>>,
    pub g: f64,
    pub drag: f64,
    pub bounce: f64,
    pub life: f64,
    pub max: f64,
    pub rest: bool,
}
impl Default for Bit {
    fn default() -> Self {
        Bit {
            kind: B::SHARD,
            x: 0.0,
            y: 0.0,
            vx: 0.0,
            vy: 0.0,
            a: 0.0,
            va: 0.0,
            s: 1.0,
            c: WHITE,
            ramp: None,
            g: 1.0,
            drag: 0.0,
            bounce: 0.0,
            life: 1.0,
            max: 1.0,
            rest: false,
        }
    }
}

#[derive(Default)]
pub struct BitsOpts {
    /// Floor y for a given x. Default: no floor.
    pub ground: Option<Box<dyn Fn(f64) -> f64>>,
}

/// Rigid debris that tumbles, bounces and settles.
pub struct Bits {
    pub list: Vec<Bit>,
    ground: Box<dyn Fn(f64) -> f64>,
}

impl Bits {
    pub fn new(o: BitsOpts) -> Self {
        Bits { list: Vec::new(), ground: o.ground.unwrap_or_else(|| Box::new(|_| f64::INFINITY)) }
    }
    pub fn add(&mut self, mut b: Bit) {
        b.max = b.life;
        b.rest = false;
        self.list.push(b);
    }
    pub fn count(&self) -> usize {
        self.list.len()
    }
    pub fn update(&mut self, dt: f64) {
        let mut i = self.list.len();
        while i > 0 {
            i -= 1;
            let ground = &self.ground;
            let b = &mut self.list[i];
            b.life -= dt;
            if b.life <= 0.0 {
                self.list.remove(i);
                continue;
            }
            if b.rest {
                continue;
            }
            b.vy += GRAVITY * b.g * dt;
            b.vx -= b.vx * b.drag * dt;
            b.vy -= b.vy * b.drag * dt;
            b.x += b.vx * dt;
            b.y += b.vy * dt;
            b.a += b.va * dt;
            let floor = ground(b.x);
            let sp = b.vy;
            if b.g <= 0.0 || b.y < floor || sp <= 0.0 {
                continue;
            }
            b.y = floor - 0.01;
            if b.kind == B::DRIP {
                b.life = 0.0;
            } else if b.bounce > 0.0 && sp > 70.0 {
                b.vy = -sp * b.bounce;
                b.vx *= 0.7;
                b.va *= 0.6;
            } else {
                b.rest = true;
                b.vx = 0.0;
                b.vy = 0.0;
            }
        }
    }
    pub fn draw(&self, p: &mut Painter) {
        for b in &self.list {
            let a = (b.life / 0.4).min(1.0);
            let (ca, sa) = (b.a.cos(), b.a.sin());
            if b.kind == B::SHARD {
                let mut d = -b.s / 2.0;
                while d <= b.s / 2.0 {
                    p.dot(b.x + ca * d, b.y + sa * d, if d > b.s / 2.0 - 0.6 { WHITE } else { b.c }, a);
                    d += 0.5;
                }
            } else if b.kind == B::DRIP {
                p.dot(b.x, b.y, b.c, a);
                p.dot(b.x, b.y - 1.0, b.c, a * 0.5);
            } else {
                // a shaded chunk: lit top-left, dark bottom-right, a dark rim
                let default = [hex("#1c2030"), hex("#3a4a66"), hex("#6e86a8"), hex("#a8c0dc"), hex("#e8f2ff")];
                let r_ = b.ramp.as_deref().unwrap_or(&default);
                let n = r_.len();
                let (x, y) = ((b.x + 0.5).floor(), (b.y + 0.5).floor());
                let r = b.s.ceil() as i32 + 1;
                for py in -r..=r {
                    for px in -r..=r {
                        let (fx, fy) = (px as f64 + 0.5, py as f64 + 0.5);
                        let u = fx * ca + fy * sa;
                        let v = -fx * sa + fy * ca;
                        let d = u.abs() / b.s + v.abs() / (b.s * 0.62);
                        if d > 1.3 {
                            continue;
                        }
                        let col = if d > 1.0 {
                            r_[0]
                        } else if v < 0.0 {
                            if u < 0.0 {
                                r_[n - 1]
                            } else {
                                r_[n - 2]
                            }
                        } else if u < 0.0 {
                            r_[n - 3]
                        } else {
                            r_[1.max(n as i32 - 4) as usize]
                        };
                        p.dot(x + px as f64, y + py as f64, col, a);
                    }
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Light {
    pub x: f64,
    pub y: f64,
    pub r: f64,
    pub color: u32,
    pub k: f64,
    pub life: f64,
    pub max: f64,
}

/// Lights that flare and die: `add(x, y, r, color, k, life)`; `update(dt)`; `draw(p)`.
#[derive(Default)]
pub struct Lights {
    pub list: Vec<Light>,
}

impl Lights {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn add(&mut self, x: f64, y: f64, r: f64, color: u32, k: f64, life: f64) {
        self.list.push(Light { x, y, r, color, k, life, max: life });
    }
    pub fn update(&mut self, dt: f64) {
        self.list.retain_mut(|l| {
            l.life -= dt;
            l.life > 0.0
        });
    }
    pub fn draw(&self, p: &mut Painter) {
        for l in &self.list {
            let k = l.life / l.max;
            p.glow(l.x, l.y, l.r * (0.55 + 0.45 * k), l.color, l.k * k * k);
        }
    }
}

fn js_round(x: f64) -> f64 {
    (x + 0.5).floor()
}

/// A star glint: arms of length `arm` in color `c` with tips in `tip`, a white heart, light `k`
/// (upstream defaults: `tip = c`, `k = 0.6`).
pub fn sparkle(p: &mut Painter, x: f64, y: f64, arm: f64, c: u32, tip: u32, k: f64) {
    let n = js_round(arm);
    if n < 1.0 {
        return;
    }
    let (fx, fy) = (x.floor(), y.floor());
    let mut d = 1.0;
    while d <= n {
        let cc = if d > n * 0.6 { tip } else { c };
        for (dx, dy) in CROSS {
            p.px(fx + dx * d, fy + dy * d, cc);
        }
        d += 1.0;
    }
    if n >= 3.0 {
        for (dx, dy) in DIAG {
            p.px(fx + dx, fy + dy, c);
        }
    }
    if n >= 6.0 {
        for (dx, dy) in DIAG {
            p.px(fx + dx * 2.0, fy + dy * 2.0, tip);
        }
    }
    p.px(fx, fy, WHITE);
    p.glow(x, y, 4.0 + n * 2.0, c, k);
}

/// An 8-ray flare: long rays on the axes, shorter between, fading out along their length. `ramp` dark to
/// bright (at least 3 entries).
pub fn flare(p: &mut Painter, x: f64, y: f64, l: f64, k: f64, ramp: &[u32]) {
    if l < 1.0 {
        return;
    }
    let n = ramp.len();
    for i in 0..8 {
        let an = (i as f64 * std::f64::consts::PI) / 4.0;
        let len = if i % 4 == 0 {
            l
        } else if i % 2 == 0 {
            l * 0.6
        } else {
            l * 0.3
        };
        let (ca, sa) = (an.cos(), an.sin());
        let mut d = 0.0;
        while d <= len {
            let u = d / len;
            let col = if u < 0.35 {
                WHITE
            } else if u < 0.7 {
                ramp[n - 2]
            } else {
                ramp[n - 3]
            };
            p.dot(x + ca * d, y + sa * d, col, if u < 0.6 { 1.0 } else { k * 1.5 * (1.0 - u) + 0.2 });
            d += 0.5;
        }
    }
}

/// A ring on a tilted plane (`sy` squashes it: 0.16 lies on the floor, 1 stands up), thickness `th` inward,
/// alpha `k`, colored by depth through the band with `ramp` (dark to bright, 4+ entries). `keep(angle)` picks
/// which part to draw, so the far half can go behind things and the near half in front: `|g| g.sin() < 0.0` is
/// the far half. Upstream defaults: `keep` all, `hot` false.
#[allow(clippy::too_many_arguments)]
pub fn band(p: &mut Painter, cx: f64, cy: f64, r: f64, sy: f64, th: f64, k: f64, ramp: &[u32], keep: impl Fn(f64) -> bool, hot: bool) {
    if r <= 0.0 || k <= 0.0 {
        return;
    }
    let n = ramp.len();
    let m = (r * TAU * 1.2).ceil() as i64;
    for j in 0..m {
        let ang = (j as f64 / m as f64) * TAU;
        let (ca, sa) = (ang.cos(), ang.sin());
        if !keep(ang) {
            continue;
        }
        let mut o = th;
        while o >= 0.0 {
            let u = o / th;
            let col = if u < 0.15 {
                if hot {
                    WHITE
                } else {
                    ramp[n - 1]
                }
            } else if u < 0.4 {
                ramp[n - 2]
            } else if u < 0.7 {
                ramp[n - 3]
            } else {
                ramp[n - 4]
            };
            p.dot(cx + ca * (r - o), cy + sa * (r - o) * sy, col, k * if u < 0.4 { 1.2 } else { 1.15 - u });
            o -= 0.5;
        }
    }
}

/// Options for `crystal`: `a` alpha, `white` flashes it white, `glint` is the 0..1 height of a sweeping
/// highlight (-1 for none). `Default` is upstream's: alpha 1, no flash, no glint.
#[derive(Clone, Copy, Debug)]
pub struct CrystalOpts {
    pub a: f64,
    pub white: bool,
    pub glint: f64,
}
impl Default for CrystalOpts {
    fn default() -> Self {
        CrystalOpts { a: 1.0, white: false, glint: -1.0 }
    }
}

/// A faceted crystal spike standing on (`x`, `base`), height `h`, base width `w`, leaning `lean` px at the tip,
/// shaded in `ramp` (dark to bright, 6+ steps): a lit ridge, a bright left face, a darker right face, a dark rim.
#[allow(clippy::too_many_arguments)]
pub fn crystal(p: &mut Painter, x: f64, base: f64, h: f64, w: f64, lean: f64, ramp: &[u32], o: &CrystalOpts) {
    if h < 1.5 {
        return;
    }
    let n = ramp.len() as i64;
    let col_at = |i: i64| ramp[(n - 1 - i).clamp(0, n - 1) as usize];
    let (a, top, ridge0, glint) = (o.a, base - h, x - w * 0.14, o.glint);
    let inside = |px: f64, py: f64| {
        let u = (base - (py + 0.5)) / h;
        (0.0..=1.0).contains(&u) && (px + 0.5 - (x + lean * u)).abs() <= (w / 2.0) * (1.0 - u) + 0.35
    };
    let mut py = top.floor() - 1.0;
    while py < base.ceil() {
        let mut px = (x - w / 2.0 - lean.abs() - 2.0).floor();
        while px <= (x + w / 2.0 + lean.abs() + 2.0).ceil() {
            if !inside(px, py) {
                if inside(px - 1.0, py) || inside(px + 1.0, py) || inside(px, py + 1.0) || (inside(px, py - 1.0) && py < base - 1.0) {
                    p.dot(px, py, ramp[0], a);
                }
                px += 1.0;
                continue;
            }
            if o.white {
                p.dot(px, py, WHITE, a);
                px += 1.0;
                continue;
            }
            let u = (base - (py + 0.5)) / h;
            let rx = ridge0 + (x + lean - ridge0) * u;
            let cc = x + lean * u;
            let hw = (w / 2.0) * (1.0 - u) + 0.35;
            let fx = px + 0.5;
            let mut col = if u > 0.9 {
                WHITE
            } else if (fx - rx).abs() < 0.55 {
                col_at(1)
            } else if fx < rx {
                if fx - (cc - hw) < 1.0 {
                    col_at(1)
                } else if (fx - (cc - hw)) / (rx - (cc - hw)).max(1.0) > 0.55 {
                    col_at(2)
                } else {
                    col_at(3)
                }
            } else if cc + hw - fx < 1.2 {
                col_at((n - 2).min(6))
            } else if (fx - rx) / (cc + hw - rx).max(1.0) > 0.5 {
                col_at(5)
            } else {
                col_at(4)
            };
            if glint >= 0.0 && fx <= rx + 0.6 {
                let g = (u - glint).abs();
                if g < 0.05 {
                    col = WHITE;
                } else if g < 0.1 {
                    col = col_at(1);
                }
            }
            p.dot(px, py, col, a);
            px += 1.0;
        }
        py += 1.0;
    }
}

#[cfg(test)]
mod const_tests {
    use super::*;

    #[test]
    fn const_colors_match_hex() {
        assert_eq!(FIRE_RAMP[4], hex("#ff8a2a"));
        assert_eq!(SMOKE_RAMP[0], hex("#7b6f72"));
        assert_eq!(HOT[3], hex("#f2561d"));
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::fx::color::{hex, ramp};
    use crate::fx::frame::{composite_bytes, Bg, Frame};
    use crate::fx::painter::Painter;
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    // Mirrors the scenes in tools/golden/sim.mjs.
    fn scene_particles() -> (Frame, usize, usize) {
        let blazes = Rc::new(RefCell::new(Blazes::new()));
        let landed = blazes.clone();
        let mut ps = Particles::new(ParticlesOpts {
            seed: 7,
            ground: Some(Box::new(|_| 20.0)),
            haze: Some(hex("#88aaff")),
            // upstream: onLand: (x, y) => blazes.add(x, y, 0.8, 1), called in the middle of update
            on_land: Some(Box::new(move |ps, x, y| landed.borrow_mut().add(ps, x, y, 0.8, 1.0))),
            ..Default::default()
        });
        let mut lights = Lights::new();
        let w = hex("#ffffff");
        let s: Vec<(u8, f64, f64, f64, f64, f64, u32, f64, f64, u32)> = vec![
            (P::SPARK, -20.0, -10.0, 50.0, -90.0, 1.2, w, 1.0, 0.0, 0),
            (P::SPARK, -18.0, -8.0, -40.0, -60.0, 1.0, w, 1.0, 0.0, 0),
            (P::MOTE, -10.0, -5.0, 90.0, -20.0, 0.8, hex("#7fd0ff"), 1.2, 0.0, 0),
            (P::STREAK, -25.0, 0.0, 120.0, 10.0, 0.9, hex("#ffd37f"), 1.0, 0.0, 0),
            (P::FLAKE, 0.0, -20.0, 5.0, 20.0, 1.5, hex("#ffffff"), 1.6, 0.0, 0),
            (P::VAPOUR, 10.0, 10.0, 0.0, -10.0, 1.4, hex("#a0b0c0"), 4.0, 0.0, 0),
            (P::DUST, 15.0, 15.0, 10.0, -5.0, 1.2, hex("#b08060"), 3.0, 0.0, 0),
            (P::STAR, 20.0, -15.0, 0.0, 0.0, 1.5, hex("#ffe26b"), 2.5, 0.0, 0),
            (P::EMBER, -5.0, 15.0, 10.0, -30.0, 1.4, hex("#ff8a2a"), 2.0, 0.0, 0),
            (P::SMOKE, 22.0, 5.0, 0.0, -8.0, 1.5, 0, 3.0, 0.0, PF::BIG),
            (P::FIRE, -12.0, 18.0, 0.0, -20.0, 0.5, 0, 3.0, 0.0, PF::SMOKES),
            (P::FIRE, -8.0, 18.0, 0.0, -20.0, 0.6, 0, 2.5, 0.1, 0),
            (P::BRAND, 5.0, -18.0, 30.0, -60.0, 1.5, w, 1.0, 0.0, 0),
            (P::GRIT, 25.0, -10.0, -60.0, -100.0, 1.2, hex("#c0a070"), 2.0, 0.0, 0),
            (P::GRIT, -28.0, -10.0, 30.0, -50.0, 1.2, hex("#c0a070"), 1.0, 0.0, 0),
        ];
        for t in s {
            ps.spawn(t.0, t.1, t.2, t.3, t.4, t.5, t.6, t.7, t.8, t.9);
        }
        lights.add(0.0, 0.0, 14.0, hex("#ff8a2a"), 0.6, 1.0);
        for _ in 0..20 {
            ps.update(1.0 / 30.0);
            blazes.borrow_mut().update(&mut ps, 1.0 / 30.0);
            lights.update(1.0 / 30.0);
        }
        let mut f = Frame::new(64, 48, 0, 0);
        {
            let mut p = Painter::new(&mut f);
            p.place(32.0, 24.0, false, 0.0);
            blazes.borrow().draw_back(&mut p);
            ps.draw(&mut p, DrawLayer::Back);
            ps.draw(&mut p, DrawLayer::Front);
            lights.draw(&mut p);
        }
        let n = blazes.borrow().count();
        (f, ps.count(), n)
    }

    fn scene_bits() -> (Frame, usize) {
        let mut bits = Bits::new(BitsOpts { ground: Some(Box::new(|x| 16.0 + x * 0.05)) });
        bits.add(Bit { kind: B::SHARD, x: -20.0, y: -10.0, vx: 40.0, vy: -80.0, a: 0.3, va: 6.0, s: 5.0, c: hex("#ffd37f"), bounce: 0.4, life: 2.0, ..Default::default() });
        bits.add(Bit { kind: B::CHUNK, x: 0.0, y: -12.0, vx: -20.0, vy: -60.0, a: 0.5, va: 3.0, s: 4.0, bounce: 0.3, life: 3.0, ..Default::default() });
        bits.add(Bit { kind: B::CHUNK, x: 14.0, y: -14.0, vx: 10.0, vy: -90.0, a: 1.1, va: -4.0, s: 3.0, g: 0.8, drag: 0.5, bounce: 0.5, life: 3.0, ..Default::default() });
        bits.add(Bit { kind: B::DRIP, x: 18.0, y: -6.0, c: hex("#4fa0ff"), life: 2.0, ..Default::default() });
        for _ in 0..20 {
            bits.update(1.0 / 30.0);
        }
        let mut f = Frame::new(64, 48, 0, 0);
        {
            let mut p = Painter::new(&mut f);
            p.place(32.0, 24.0, false, 0.0);
            bits.draw(&mut p);
            sparkle(&mut p, -22.0, -18.0, 4.0, hex("#ffe26b"), hex("#ffffff"), 0.5);
            let fl = ramp(&["#401010", "#a02020", "#ff6030", "#ffd080", "#fff0c0"]);
            flare(&mut p, 20.0, -16.0, 8.0, 0.6, &fl);
            band(&mut p, 0.0, 5.0, 14.0, 0.4, 3.0, 0.8, &fl, |g| g.sin() < 0.0, false);
            let cr = ramp(&["#14182c", "#2a3a6a", "#3d5fa0", "#5f8fd0", "#8fc0f0", "#cfeaff"]);
            crystal(&mut p, -12.0, 20.0, 16.0, 8.0, 2.0, &cr, &CrystalOpts { glint: 0.5, ..Default::default() });
        }
        (f, bits.count())
    }

    fn check(f: &Frame, golden: &[u8]) {
        let w = u32::from_le_bytes(golden[0..4].try_into().unwrap()) as usize;
        let h = u32::from_le_bytes(golden[4..8].try_into().unwrap()) as usize;
        assert_eq!((w, h), (f.w, f.h));
        let want = &golden[8..];
        let got = composite_bytes(f, Bg::Flat(hex("#201020")), 1.0);
        let mut bad = 0;
        for (g, x) in got.chunks(4).zip(want.chunks(4)) {
            if g.iter().zip(x).any(|(a, b)| (*a as i32 - *b as i32).abs() > 2) {
                bad += 1;
            }
        }
        let ratio = bad as f64 / (w * h) as f64;
        assert!(ratio <= 0.03, "{bad} of {} pixels differ ({ratio})", w * h);
        assert!(f.color.iter().any(|&c| c != 0));
    }

    #[test]
    fn particles_match_upstream() {
        let (f, alive, blazes) = scene_particles();
        assert_eq!((alive, blazes), (25, 1));
        check(&f, include_bytes!("../../tests/golden/sim_particles.bin"));
    }

    #[test]
    fn bits_and_helpers_match_upstream() {
        let (f, alive) = scene_bits();
        assert_eq!(alive, 3);
        check(&f, include_bytes!("../../tests/golden/sim_bits.bin"));
    }

    #[test]
    fn particles_remove_dead_and_honour_delay() {
        let mut ps = Particles::new(ParticlesOpts { seed: 1, ..Default::default() });
        ps.spawn(P::MOTE, 0.0, 0.0, 1.0, 0.0, 0.1, 0xffff_ffff, 1.0, 0.0, 0);
        ps.spawn(P::MOTE, 0.0, 0.0, 1.0, 0.0, 1.0, 0xffff_ffff, 1.0, 0.5, 0);
        assert_eq!(ps.count(), 2);
        ps.update(0.2);
        assert_eq!(ps.count(), 1, "the short-lived particle is gone, the delayed one is still waiting");
        let q = &ps.list[0];
        assert!(q.delay > 0.0 && q.life == 1.0, "a delayed particle neither ages nor moves");
    }

    #[test]
    fn dying_fire_leaves_smoke_and_brand_reports_landing() {
        let mut ps = Particles::new(ParticlesOpts { seed: 3, ground: Some(Box::new(|_| 10.0)), ..Default::default() });
        ps.spawn(P::FIRE, 0.0, 0.0, 0.0, 0.0, 0.05, 0, 2.0, 0.0, PF::SMOKES);
        ps.update(0.1);
        assert_eq!(ps.count(), 1);
        assert_eq!(ps.list[0].k, P::SMOKE);
        let landed = Rc::new(Cell::new(0));
        let l2 = landed.clone();
        let mut ps = Particles::new(ParticlesOpts {
            seed: 3,
            ground: Some(Box::new(|_| 10.0)),
            on_land: Some(Box::new(move |_, _, _| l2.set(l2.get() + 1))),
            ..Default::default()
        });
        ps.spawn(P::BRAND, 0.0, 9.0, 0.0, 200.0, 2.0, 0xffff_ffff, 1.0, 0.0, 0);
        ps.update(0.05);
        assert_eq!(landed.get(), 1);
        assert!(ps.list.iter().all(|p| p.k != P::BRAND));
    }

    #[test]
    fn bits_bounce_settle_and_drip_dies() {
        let mut bits = Bits::new(BitsOpts { ground: Some(Box::new(|_| 5.0)) });
        bits.add(Bit { kind: B::CHUNK, x: 0.0, y: 4.0, vy: 10.0, life: 10.0, ..Default::default() });
        bits.add(Bit { kind: B::DRIP, x: 3.0, y: 4.0, vy: 10.0, life: 10.0, ..Default::default() });
        bits.update(0.05);
        bits.update(0.05);
        assert_eq!(bits.count(), 1, "a drip dies on the floor");
        assert!(bits.list[0].rest && bits.list[0].vy == 0.0, "no bounce set, so it settles");
        let n = bits.count();
        bits.update(0.05);
        assert_eq!(n, bits.count());
    }

    #[test]
    fn lights_expire() {
        let mut l = Lights::new();
        l.add(0.0, 0.0, 5.0, 0xffff_ffff, 1.0, 0.1);
        l.update(0.05);
        assert_eq!(l.list.len(), 1);
        l.update(0.06);
        assert!(l.list.is_empty());
    }

    #[test]
    fn crystal_and_flare_ignore_tiny_input() {
        let mut f = Frame::new(16, 16, 0, 0);
        let mut p = Painter::new(&mut f);
        crystal(&mut p, 8.0, 12.0, 1.0, 4.0, 0.0, &ramp(&["#000", "#111", "#222", "#333", "#444", "#555"]), &CrystalOpts::default());
        flare(&mut p, 8.0, 8.0, 0.5, 1.0, &ramp(&["#000", "#111", "#222"]));
        sparkle(&mut p, 8.0, 8.0, 0.4, 0xffff_ffff, 0xffff_ffff, 0.6);
        assert!(f.color.iter().all(|&c| c == 0) && f.light.iter().all(|&c| c == 0));
    }
}
