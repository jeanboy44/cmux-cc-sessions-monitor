//! Particles without a simulation: each one's position is a closed-form function of its age, so an effect
//! can be drawn at any time t, scrubbed, looped and rendered frame by frame. A port of pixelfx
//! `core/particles.js`. `Particles` here is upstream's simple list for effects that keep state; the richer
//! system with kinds and drawing is `sim::Particles`.

use crate::fx::math::{clamp01, rng, Rng};

/// Where a particle is after `t` seconds, launched from (x, y) at (vx, vy) px/s, under gravity `g` (px/s^2,
/// positive is down) and linear drag (1/s). Returns (x, y).
pub fn fly(x: f64, y: f64, vx: f64, vy: f64, t: f64, g: f64, drag: f64) -> (f64, f64) {
    if drag <= 0.0 {
        return (x + vx * t, y + vy * t + 0.5 * g * t * t);
    }
    let f = (1.0 - (-drag * t).exp()) / drag;
    let vt = g / drag;
    (x + vx * f, y + vt * t + (vy - vt) * f)
}

/// Its velocity at `t`, for streaks that point along the motion. Returns (vx, vy).
pub fn fly_v(vx: f64, vy: f64, t: f64, g: f64, drag: f64) -> (f64, f64) {
    if drag <= 0.0 {
        return (vx, vy + g * t);
    }
    let e = (-drag * t).exp();
    let vt = g / drag;
    (vx * e, vt + (vy - vt) * e)
}

/// `n` one-shot particles: `f(r, i)` with a repeatable rng per particle (the same numbers every frame).
pub fn burst(n: usize, seed: f64, mut f: impl FnMut(Rng, usize)) {
    for i in 0..n {
        f(rng(&[seed, i as f64]), i);
    }
}

/// A particle lifetime: a number of seconds, or a function of the particle's rng.
pub enum Life<'a> {
    Fixed(f64),
    With(Box<dyn Fn(&mut Rng) -> f64 + 'a>),
}
impl From<f64> for Life<'_> {
    fn from(v: f64) -> Self {
        Life::Fixed(v)
    }
}
impl<'a> Life<'a> {
    /// A lifetime drawn from the rng.
    pub fn with(f: impl Fn(&mut Rng) -> f64 + 'a) -> Self {
        Life::With(Box::new(f))
    }
    fn get(&self, g: &mut Rng) -> f64 {
        match self {
            Life::Fixed(v) => *v,
            Life::With(f) => f(g),
        }
    }
}

/// Particles that repeat every `period` seconds, for loops with no seam. Particle i is born at its own phase
/// in the period and lives `life` seconds. For every live copy, `f(age, life, r, i)` is called; `r` is a fresh
/// rng for particle i, the same each period.
pub fn cycle<'a>(t: f64, period: f64, count: usize, seed: f64, life: impl Into<Life<'a>>, mut f: impl FnMut(f64, f64, Rng, usize)) {
    let life = life.into();
    for i in 0..count {
        let mut g = rng(&[seed, i as f64, 911.0]);
        let born = g.next() * period;
        let l = life.get(&mut g);
        let mut age = (((t - born) % period) + period) % period;
        while age < l {
            f(age, l, rng(&[seed, i as f64]), i);
            age += period;
        }
    }
}

/// Particles born at a steady `rate` (per second) between `t0` and `t1` (an emitter that stops):
/// `f(age, life, r, i)` for every particle alive at `t`.
pub fn stream<'a>(t: f64, t0: f64, t1: f64, rate: f64, seed: f64, life: impl Into<Life<'a>>, mut f: impl FnMut(f64, f64, Rng, usize)) {
    let life = life.into();
    let n = ((t.min(t1) - t0) * rate).floor();
    let mut i = 0usize;
    while (i as f64) <= n {
        let born = t0 + i as f64 / rate;
        let mut g = rng(&[seed, i as f64, 911.0]);
        let l = life.get(&mut g);
        let age = t - born;
        if age >= 0.0 && age < l {
            f(age, l, rng(&[seed, i as f64]), i);
        }
        i += 1;
    }
}

/// One simple simulated particle. `ground` is the floor y, if any; `bounce` its restitution (default 0.3).
#[derive(Clone, Copy, Debug, Default)]
pub struct Particle {
    pub x: f64,
    pub y: f64,
    pub vx: f64,
    pub vy: f64,
    pub life: f64,
    pub age: f64,
    pub g: f64,
    pub drag: f64,
    pub ground: Option<f64>,
    pub bounce: Option<f64>,
}

/// A simple simulated particle list, for effects that keep state.
#[derive(Default)]
pub struct Particles {
    pub list: Vec<Particle>,
}

impl Particles {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn add(&mut self, mut p: Particle) {
        p.age = 0.0;
        self.list.push(p);
    }
    pub fn update(&mut self, dt: f64) {
        let mut i = self.list.len();
        while i > 0 {
            i -= 1;
            let p = &mut self.list[i];
            p.age += dt;
            if p.age >= p.life {
                self.list.remove(i);
                continue;
            }
            p.vx -= p.vx * p.drag * dt;
            p.vy += p.g * dt - p.vy * p.drag * dt;
            p.x += p.vx * dt;
            p.y += p.vy * dt;
            if let Some(ground) = p.ground {
                if p.y > ground {
                    p.y = ground;
                    p.vy = -p.vy.abs() * p.bounce.unwrap_or(0.3);
                    p.vx *= 0.7;
                }
            }
        }
    }
    /// Life left, 1 at birth to 0 at death.
    pub fn k(p: &Particle) -> f64 {
        clamp01(1.0 - p.age / p.life)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fly_matches_closed_form_and_flyv_is_its_derivative() {
        let (x, y) = fly(1.0, 2.0, 10.0, -5.0, 2.0, 100.0, 0.0);
        assert_eq!((x, y), (21.0, 2.0 - 10.0 + 200.0));
        let (x, y) = fly(0.0, 0.0, 10.0, -5.0, 1.5, 100.0, 2.0);
        let h = 1e-6;
        let (x2, y2) = fly(0.0, 0.0, 10.0, -5.0, 1.5 + h, 100.0, 2.0);
        let (vx, vy) = fly_v(10.0, -5.0, 1.5, 100.0, 2.0);
        assert!(((x2 - x) / h - vx).abs() < 1e-3 && ((y2 - y) / h - vy).abs() < 1e-3);
        assert_eq!(fly_v(3.0, 4.0, 2.0, 10.0, 0.0), (3.0, 24.0));
    }

    #[test]
    fn burst_cycle_stream_visit_the_right_particles() {
        let mut n = 0;
        burst(5, 9.0, |_, _| n += 1);
        assert_eq!(n, 5);
        // a cycle repeats with its period: the same particles are live at t and t + period
        let mut a = Vec::new();
        cycle(0.3, 1.0, 6, 4.0, 0.5, |age, _, _, i| a.push((i, (age * 1e6).round() as i64)));
        let mut b = Vec::new();
        cycle(1.3, 1.0, 6, 4.0, 0.5, |age, _, _, i| b.push((i, (age * 1e6).round() as i64)));
        assert!(!a.is_empty());
        assert_eq!(a, b);
        // a stream: 10/s from 0 to 1, at t = 0.55 only the particles born within `life` are alive
        let mut ids = Vec::new();
        stream(0.55, 0.0, 1.0, 10.0, 1.0, 0.25, |age, life, _, i| {
            assert!((0.0..life).contains(&age));
            ids.push(i);
        });
        assert_eq!(ids, vec![4, 5]);
    }

    #[test]
    fn particles_drop_the_dead_and_bounce_off_the_ground() {
        let mut ps = Particles::new();
        ps.add(Particle { life: 0.1, ..Default::default() });
        ps.add(Particle { y: 9.0, vy: 100.0, g: 0.0, life: 5.0, ground: Some(10.0), ..Default::default() });
        ps.update(0.05);
        assert_eq!(ps.list.len(), 2);
        let p = ps.list[1];
        assert_eq!(p.y, 10.0, "clamped to the ground");
        assert!(p.vy < 0.0 && (p.vy + 30.0).abs() < 1e-9, "bounced at 0.3");
        ps.update(0.06);
        assert_eq!(ps.list.len(), 1, "the 0.1 s particle died");
        assert!(Particles::k(&ps.list[0]) < 1.0 && Particles::k(&ps.list[0]) > 0.9);
    }
}
