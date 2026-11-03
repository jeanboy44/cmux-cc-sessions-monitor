//! Numbers, easing, randomness and noise: a port of pixelfx `core/math.js`.
//! Integer paths reproduce JS `Math.imul` / `>>> 0` semantics exactly.

use std::f64::consts::PI;

pub const TAU: f64 = PI * 2.0;

pub fn clamp(x: f64, a: f64, b: f64) -> f64 {
    if x < a {
        a
    } else if x > b {
        b
    } else {
        x
    }
}
/// Clamp to 0..1 (the JS default arguments).
pub fn clamp01(x: f64) -> f64 {
    clamp(x, 0.0, 1.0)
}
pub fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}
pub fn inv_lerp(a: f64, b: f64, x: f64) -> f64 {
    if b == a {
        0.0
    } else {
        (x - a) / (b - a)
    }
}
pub fn remap(x: f64, a0: f64, a1: f64, b0: f64, b1: f64) -> f64 {
    lerp(b0, b1, inv_lerp(a0, a1, x))
}
pub fn fract(x: f64) -> f64 {
    x - x.floor()
}
pub fn wrap(x: f64, m: f64) -> f64 {
    ((x % m) + m) % m
}
/// Progress 0..1 of t through the window [t0, t1]: 0 before it, 1 after it.
pub fn seg(t: f64, t0: f64, t1: f64) -> f64 {
    clamp01((t - t0) / (t1 - t0))
}
/// Hermite smoothstep of x between e0 and e1.
pub fn smooth(e0: f64, e1: f64, x: f64) -> f64 {
    let t = seg(x, e0, e1);
    t * t * (3.0 - 2.0 * t)
}
/// 0 -> 1 -> 0 over the window [t0, t1] (sine bump).
pub fn bump(t: f64, t0: f64, t1: f64) -> f64 {
    if t <= t0 || t >= t1 {
        0.0
    } else {
        (PI * seg(t, t0, t1)).sin()
    }
}
pub fn pingpong(x: f64) -> f64 {
    1.0 - (wrap(x, 2.0) - 1.0).abs()
}
pub fn dist(x0: f64, y0: f64, x1: f64, y1: f64) -> f64 {
    (x1 - x0).hypot(y1 - y0)
}
pub fn angle_to(x0: f64, y0: f64, x1: f64, y1: f64) -> f64 {
    (y1 - y0).atan2(x1 - x0)
}

/// Easing curves: input 0..1, output 0..1 (back and elastic overshoot).
pub mod ease {
    use std::f64::consts::PI;

    /// Upstream default overshoot for `in_back` / `out_back`.
    pub const BACK_S: f64 = 1.70158;

    pub fn linear(t: f64) -> f64 {
        t
    }
    pub fn in_quad(t: f64) -> f64 {
        t * t
    }
    pub fn out_quad(t: f64) -> f64 {
        1.0 - (1.0 - t) * (1.0 - t)
    }
    pub fn in_out_quad(t: f64) -> f64 {
        if t < 0.5 {
            2.0 * t * t
        } else {
            1.0 - (-2.0 * t + 2.0).powi(2) / 2.0
        }
    }
    pub fn in_cubic(t: f64) -> f64 {
        t * t * t
    }
    pub fn out_cubic(t: f64) -> f64 {
        1.0 - (1.0 - t).powi(3)
    }
    pub fn in_out_cubic(t: f64) -> f64 {
        if t < 0.5 {
            4.0 * t * t * t
        } else {
            1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
        }
    }
    pub fn in_quart(t: f64) -> f64 {
        t * t * t * t
    }
    pub fn out_quart(t: f64) -> f64 {
        1.0 - (1.0 - t).powi(4)
    }
    pub fn in_expo(t: f64) -> f64 {
        if t <= 0.0 {
            0.0
        } else {
            2f64.powf(10.0 * t - 10.0)
        }
    }
    pub fn out_expo(t: f64) -> f64 {
        if t >= 1.0 {
            1.0
        } else {
            1.0 - 2f64.powf(-10.0 * t)
        }
    }
    pub fn in_sine(t: f64) -> f64 {
        1.0 - (t * PI / 2.0).cos()
    }
    pub fn out_sine(t: f64) -> f64 {
        (t * PI / 2.0).sin()
    }
    pub fn in_out_sine(t: f64) -> f64 {
        -((PI * t).cos() - 1.0) / 2.0
    }
    /// Upstream default overshoot `s` is 1.70158.
    pub fn in_back(t: f64, s: f64) -> f64 {
        if t <= 0.0 {
            0.0
        } else if t >= 1.0 {
            1.0
        } else {
            (s + 1.0) * t * t * t - s * t * t
        }
    }
    /// Upstream default overshoot `s` is 1.70158.
    pub fn out_back(t: f64, s: f64) -> f64 {
        if t <= 0.0 {
            0.0
        } else if t >= 1.0 {
            1.0
        } else {
            1.0 + (s + 1.0) * (t - 1.0).powi(3) + s * (t - 1.0).powi(2)
        }
    }
    pub fn out_elastic(t: f64) -> f64 {
        if t <= 0.0 {
            0.0
        } else if t >= 1.0 {
            1.0
        } else {
            2f64.powf(-10.0 * t) * ((t * 10.0 - 0.75) * ((2.0 * PI) / 3.0)).sin() + 1.0
        }
    }
    pub fn out_bounce(mut t: f64) -> f64 {
        let (n, d) = (7.5625, 2.75);
        if t < 1.0 / d {
            n * t * t
        } else if t < 2.0 / d {
            t -= 1.5 / d;
            n * t * t + 0.75
        } else if t < 2.5 / d {
            t -= 2.25 / d;
            n * t * t + 0.9375
        } else {
            t -= 2.625 / d;
            n * t * t + 0.984375
        }
    }
}

/// JS ToInt32 (`n | 0`).
fn to_int32(n: f64) -> i32 {
    if !n.is_finite() {
        return 0;
    }
    n.trunc().rem_euclid(4294967296.0) as u64 as u32 as i32
}

/// 32-bit integer hash of any number of integers.
pub fn hash(ns: &[f64]) -> u32 {
    let mut h: u32 = 0x9e37_79b9;
    for &n in ns {
        h = (h ^ to_int32(n) as u32).wrapping_mul(0x85eb_ca6b);
        h ^= h >> 13;
        h = h.wrapping_mul(0xc2b2_ae35);
        h ^= h >> 16;
    }
    h
}
/// Deterministic random number in [0, 1) from integers (seed, particle index, which value...).
pub fn rand(ns: &[f64]) -> f64 {
    hash(ns) as f64 / 4294967296.0
}

/// Seeded random generator (mulberry32).
#[derive(Clone, Debug)]
pub struct Rng {
    s: u32,
}
impl Rng {
    pub fn new(seed: u32) -> Self {
        Rng { s: if seed == 0 { 1 } else { seed } }
    }
    // Not an Iterator: mirrors the upstream `next()` and never ends.
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> f64 {
        self.s = self.s.wrapping_add(0x6d2b_79f5);
        let mut t = self.s;
        t = (t ^ (t >> 15)).wrapping_mul(t | 1);
        t ^= t.wrapping_add((t ^ (t >> 7)).wrapping_mul(t | 61));
        (t ^ (t >> 14)) as f64 / 4294967296.0
    }
    pub fn range(&mut self, a: f64, b: f64) -> f64 {
        a + (b - a) * self.next()
    }
    /// Integer in [a, b] inclusive.
    pub fn int(&mut self, a: f64, b: f64) -> i64 {
        self.range(a, b + 1.0).floor() as i64
    }
    pub fn chance(&mut self, p: f64) -> bool {
        self.next() < p
    }
    pub fn sign(&mut self) -> f64 {
        if self.next() < 0.5 {
            -1.0
        } else {
            1.0
        }
    }
    pub fn pick<'a, T>(&mut self, list: &'a [T]) -> &'a T {
        &list[(self.next() * list.len() as f64).floor() as usize]
    }
    /// Roughly normal, mean 0, sd 1.
    pub fn gauss(&mut self) -> f64 {
        let a = self.next();
        let b = self.next();
        let c = self.next();
        let d = self.next();
        (a + b + c + d - 2.0) * 1.732
    }
}
/// A generator for one particle: rng(&[seed, i]) gives the same numbers every frame.
pub fn rng(ns: &[f64]) -> Rng {
    Rng::new(hash(ns))
}

/// Smooth 1D value noise in [-1, 1].
pub fn noise(x: f64, seed: f64) -> f64 {
    let i = x.floor();
    let f = x - i;
    let u = f * f * (3.0 - 2.0 * f);
    lerp(rand(&[i, seed]), rand(&[i + 1.0, seed]), u) * 2.0 - 1.0
}
/// Smooth 2D value noise in [-1, 1].
pub fn noise2(x: f64, y: f64, seed: f64) -> f64 {
    let (ix, iy) = (x.floor(), y.floor());
    let (fx, fy) = (x - ix, y - iy);
    let ux = fx * fx * (3.0 - 2.0 * fx);
    let uy = fy * fy * (3.0 - 2.0 * fy);
    let a = rand(&[ix, iy, seed]);
    let b = rand(&[ix + 1.0, iy, seed]);
    let c = rand(&[ix, iy + 1.0, seed]);
    let d = rand(&[ix + 1.0, iy + 1.0, seed]);
    lerp(lerp(a, b, ux), lerp(c, d, ux), uy) * 2.0 - 1.0
}
/// Fractal 2D noise in about [-1, 1].
pub fn fbm(mut x: f64, mut y: f64, seed: f64, octaves: u32) -> f64 {
    let (mut s, mut a, mut n) = (0.0, 0.5, 0.0);
    for o in 0..octaves {
        s += noise2(x, y, seed + o as f64 * 101.0) * a;
        n += a;
        x *= 2.03;
        y *= 2.03;
        a *= 0.5;
    }
    s / n
}

/// 2D value noise whose lattice repeats every `px` cells in x and `py` cells in y (0: no repeat).
pub fn tile_noise(x: f64, y: f64, px: f64, py: f64, seed: f64) -> f64 {
    let (ix, iy) = (x.floor(), y.floor());
    let (fx, fy) = (x - ix, y - iy);
    let ux = fx * fx * (3.0 - 2.0 * fx);
    let uy = fy * fy * (3.0 - 2.0 * fy);
    let w = |v: f64, p: f64| if p != 0.0 { wrap(v, p) } else { v };
    let (x0, x1) = (w(ix, px), w(ix + 1.0, px));
    let (y0, y1) = (w(iy, py), w(iy + 1.0, py));
    let a = rand(&[x0, y0, seed]);
    let b = rand(&[x1, y0, seed]);
    let c = rand(&[x0, y1, seed]);
    let d = rand(&[x1, y1, seed]);
    lerp(lerp(a, b, ux), lerp(c, d, ux), uy) * 2.0 - 1.0
}
/// Fractal `tile_noise`: every octave keeps the repeat, so scrolled fields still loop.
pub fn tile_fbm(x: f64, y: f64, px: f64, py: f64, seed: f64, octaves: u32) -> f64 {
    let (mut s, mut a, mut n, mut k) = (0.0, 0.5, 0.0, 1.0);
    for o in 0..octaves {
        s += tile_noise(x * k, y * k, px * k, py * k, seed + o as f64 * 101.0) * a;
        n += a;
        a *= 0.5;
        k *= 2.0;
    }
    s / n
}
/// Noise that loops: the same value at phase 0 and phase 1.
pub fn loop_noise(phase: f64, radius: f64, seed: f64) -> f64 {
    let a = phase * TAU;
    noise2(a.cos() * radius + 50.0, a.sin() * radius + 50.0, seed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn f(v: &Value) -> f64 {
        v.as_f64().unwrap()
    }

    /// serde_json's default float parser can be 1 ulp off, but rand/Rng values are k / 2^32,
    /// so compare the exact integer k.
    fn k(v: &Value) -> u64 {
        (f(v) * 4294967296.0).round() as u64
    }
    fn k_of(x: f64) -> u64 {
        (x * 4294967296.0).round() as u64
    }

    #[test]
    fn matches_upstream() {
        let g: Value = serde_json::from_str(include_str!("../../tests/golden/math.json")).unwrap();
        for c in g["hash"].as_array().unwrap() {
            let args: Vec<f64> = c["args"].as_array().unwrap().iter().map(f).collect();
            assert_eq!(hash(&args) as u64, c["hash"].as_u64().unwrap(), "hash {args:?}");
            assert_eq!(k_of(rand(&args)), k(&c["rand"]), "rand {args:?}");
        }
        for c in g["rng"].as_array().unwrap() {
            let mut r = Rng::new(c["seed"].as_u64().unwrap() as u32);
            for (i, v) in c["values"].as_array().unwrap().iter().enumerate() {
                assert_eq!(k_of(r.next()), k(v), "rng seed {} #{i}", c["seed"]);
            }
        }
        for c in g["noise"].as_array().unwrap() {
            let got = noise(f(&c["x"]), f(&c["seed"]));
            assert!((got - f(&c["v"])).abs() < 1e-9, "noise {c}");
        }
        for c in g["noise2"].as_array().unwrap() {
            let got = noise2(f(&c["x"]), f(&c["y"]), f(&c["seed"]));
            assert!((got - f(&c["v"])).abs() < 1e-9, "noise2 {c}");
        }
        for c in g["fbm"].as_array().unwrap() {
            let got = fbm(f(&c["x"]), f(&c["y"]), f(&c["seed"]), c["octaves"].as_u64().unwrap() as u32);
            assert!((got - f(&c["v"])).abs() < 1e-9, "fbm {c}");
        }
        for c in g["tile"].as_array().unwrap() {
            let (x, y, px, py, seed) = (f(&c["x"]), f(&c["y"]), f(&c["px"]), f(&c["py"]), f(&c["seed"]));
            assert!((tile_noise(x, y, px, py, seed) - f(&c["v"])).abs() < 1e-9, "tile_noise {c}");
            assert!((tile_fbm(x, y, px, py, seed, 4) - f(&c["f"])).abs() < 1e-9, "tile_fbm {c}");
        }
        for c in g["loop"].as_array().unwrap() {
            let got = loop_noise(f(&c["phase"]), f(&c["radius"]), f(&c["seed"]));
            assert!((got - f(&c["v"])).abs() < 1e-9, "loop_noise {c}");
        }
    }

    #[test]
    fn back_s_constant() {
        assert_eq!(ease::BACK_S, 1.70158);
        assert_eq!(ease::out_back(0.5, ease::BACK_S), 1.0 + 2.70158 * -0.125 + 1.70158 * 0.25);
    }
}
