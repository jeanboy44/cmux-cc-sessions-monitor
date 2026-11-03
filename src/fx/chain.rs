//! Moves: several effects played as one, a port of pixelfx `core/chain.js`. A cue says which effect, when, where and for
//! how long; a projectile is just an effect whose x is a function of time. From outside a chain is an ordinary effect
//! ([`Effect::composite`]).
//!
//! Port note: upstream moves read a module-level `TX` (target distance) inside cue closures. Here a move builds its chain
//! from the scene's `tx` (`Move::create: fn(tx) -> Chain`), so closures capture it: `Pos::f(move |t| 28.0 + (t - THROW)
//! / FLY * (tx - 28.0))`.

use super::act::{Dust, Hit, Key};
use super::effect::{create_fx, Effect, EffectImpl, FxInstance, Options};
use super::painter::{Layer, Painter};
use super::rig::Pose;
use std::rc::Rc;

/// A cue coordinate: a number, or a function of the chain's time (seconds since the chain started).
#[derive(Clone)]
pub enum Pos {
    K(f64),
    F(Rc<dyn Fn(f64) -> f64>),
}
impl Pos {
    pub fn f(f: impl Fn(f64) -> f64 + 'static) -> Pos {
        Pos::F(Rc::new(f))
    }
    pub fn at(&self, t: f64) -> f64 {
        match self {
            Pos::K(v) => *v,
            Pos::F(f) => f(t),
        }
    }
}
impl From<f64> for Pos {
    fn from(v: f64) -> Pos {
        Pos::K(v)
    }
}

/// One effect in a chain: `{ fx, t, until, x, y, flip, ...options }` upstream. Build with [`Cue::new`] and the setters.
#[derive(Clone)]
pub struct Cue {
    pub fx: Effect,
    /// When it starts, in chain time.
    pub t: f64,
    /// Cut it off here.
    pub until: f64,
    /// Where its origin goes, local to the chain (mirrored with it).
    pub x: Pos,
    pub y: Pos,
    /// Mirror this cue on top of the chain's own flip.
    pub flip: bool,
    /// Overrides of the effect's options (the seed is always the chain's seed plus the cue's index).
    pub opts: Options,
}
impl Cue {
    pub fn new(fx: Effect) -> Cue {
        Cue { fx, t: 0.0, until: f64::INFINITY, x: Pos::K(0.0), y: Pos::K(0.0), flip: false, opts: Options::new() }
    }
    pub fn t(mut self, t: f64) -> Cue {
        self.t = t;
        self
    }
    pub fn until(mut self, t: f64) -> Cue {
        self.until = t;
        self
    }
    pub fn x(mut self, x: impl Into<Pos>) -> Cue {
        self.x = x.into();
        self
    }
    pub fn y(mut self, y: impl Into<Pos>) -> Cue {
        self.y = y.into();
        self
    }
    pub fn flip(mut self, f: bool) -> Cue {
        self.flip = f;
        self
    }
    /// Option overrides for this effect (palette, named numbers and ramps, light).
    pub fn opts(mut self, o: Options) -> Cue {
        self.opts = o;
        self
    }
    /// Shorthand for the most common override.
    pub fn palette(mut self, r: &[u32]) -> Cue {
        self.opts.palette = Some(r.to_vec());
        self
    }
}

/// A list of cues (upstream `chain([...])` before it is given options).
#[derive(Clone, Default)]
pub struct Chain {
    pub cues: Vec<Cue>,
}
pub fn chain(cues: Vec<Cue>) -> Chain {
    Chain { cues }
}

struct Live {
    cue: Cue,
    opts: Options,
    inst: Option<FxInstance>,
    over: bool,
}
/// A chain being played (what upstream `chain(cues)(o)` returns).
pub struct ChainInst {
    list: Vec<Live>,
    t: f64,
}
impl ChainInst {
    pub fn new(c: &Chain, o: &Options) -> ChainInst {
        let base = o.seed();
        let list = c
            .cues
            .iter()
            .enumerate()
            .map(|(i, cue)| {
                let mut opts = cue.opts.clone();
                opts.seed = Some(base.wrapping_add(i as u32));
                Live { cue: cue.clone(), opts, inst: None, over: false }
            })
            .collect();
        ChainInst { list, t: 0.0 }
    }
    fn each(&mut self, p: &mut Painter, layer: Layer) {
        let (tx, ty) = p.origin();
        let sign = p.sign();
        let t = self.t;
        for c in self.list.iter_mut() {
            if c.over {
                continue;
            }
            let Some(inst) = c.inst.as_mut() else { continue };
            inst.options.x = tx + sign * c.cue.x.at(t);
            inst.options.y = ty + c.cue.y.at(t);
            inst.options.flip = (sign < 0.0) != c.cue.flip;
            inst.draw(&mut *p.frame, Some(layer));
        }
    }
}
impl EffectImpl for ChainInst {
    fn update(&mut self, dt: f64, _o: &Options) {
        self.t += dt;
        let t = self.t;
        for c in self.list.iter_mut() {
            if c.over || t < c.cue.t {
                continue;
            }
            match c.inst.as_mut() {
                None => c.inst = Some(create_fx(&c.cue.fx, &c.opts)),
                Some(i) => i.update(dt),
            }
            let i = c.inst.as_ref().unwrap();
            if t >= c.cue.until || (i.done() && !c.cue.fx.meta.looping) {
                c.over = true;
            }
        }
    }
    fn draw_back(&mut self, p: &mut Painter, _t: f64, _o: &Options) {
        self.each(p, Layer::Back);
    }
    fn draw(&mut self, p: &mut Painter, _t: f64, _o: &Options) {
        self.each(p, Layer::Front);
    }
}

/// Reuse an act's keys in a longer move: later by `dt`, moved along by `dx`, with `over` laid over each pose
/// (`x` always comes from the key: its own x plus dx).
pub fn shift_keys(keys: &[Key], dt: f64, dx: f64, over: Pose) -> Vec<Key> {
    keys.iter()
        .map(|k| {
            let mut pose = k.pose.merge(&over);
            pose.x = Some(k.pose.x.unwrap_or(0.0) + dx);
            Key { t: k.t + dt, pose, how: k.how }
        })
        .collect()
}

/// Anything on an act's timeline (hits and dust).
pub trait Timed: Clone {
    fn time(&self) -> f64;
    fn set_time(&mut self, t: f64);
}
impl Timed for Hit {
    fn time(&self) -> f64 {
        self.t
    }
    fn set_time(&mut self, t: f64) {
        self.t = t;
    }
}
impl Timed for Dust {
    fn time(&self) -> f64 {
        self.t
    }
    fn set_time(&mut self, t: f64) {
        self.t = t;
    }
}
/// The same as [`shift_keys`] for hits (and dust, which upstream shifts with the same function).
pub fn shift_hits<T: Timed>(items: &[T], dt: f64) -> Vec<T> {
    items
        .iter()
        .map(|h| {
            let mut h = h.clone();
            h.set_time(h.time() + dt);
            h
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fx::act::{key, Dust, Hit};
    use crate::fx::effect::{create_fx, Effect, Meta, Options};
    use crate::fx::frame::Frame;
    use crate::fx::painter::Painter;
    use crate::fx::poses::READY;
    use crate::fx::rig::{Pose, Weapon};

    #[test]
    fn shift_keys_moves_times_and_feet() {
        let keys = vec![key(0.0, READY.x(2.0)), key(0.5, READY)];
        let s = shift_keys(&keys, 1.0, 10.0, Pose::EMPTY.weapon(Weapon::Sword).x(99.0));
        assert_eq!(s.len(), 2);
        assert_eq!((s[0].t, s[1].t), (1.0, 1.5));
        assert_eq!((s[0].pose.x, s[1].pose.x), (Some(12.0), Some(10.0)), "x comes from the key, not the overlay");
        assert!(s.iter().all(|k| k.pose.weapon == Some(Weapon::Sword) && k.pose.fx == READY.fx));
    }

    #[test]
    fn shift_hits_moves_hits_and_dust() {
        let h = shift_hits(&[Hit::new(0.2).push(2.0), Hit::new(0.5)], 1.0);
        assert_eq!((h[0].t, h[1].t, h[0].push), (1.2, 1.5, 2.0));
        let d = shift_hits(&[Dust::new(0.1, 4.0)], 0.5);
        assert_eq!((d[0].t, d[0].x), (0.6, 4.0));
    }

    fn seed_px(p: &mut Painter, _t: f64, o: &Options) {
        p.px(o.seed() as f64, 0.0, o.palette()[0]);
    }
    const C: u32 = 0xFF12_3456;
    fn fx() -> Effect {
        Effect::stateless(Meta::new("px", (8, 8), (4, 4), 1.0).options(Options::new().with_palette(&[C])), seed_px)
    }

    #[test]
    fn cue_plays_from_t_until_cut_and_seeds_by_index() {
        let ch = chain(vec![Cue::new(fx()).t(0.2).until(0.5).y(1.0), Cue::new(fx()).x(Pos::f(|t| t * 10.0)).y(2.0)]);
        let mut inst = create_fx(&Effect::composite(Meta::new("m", (40, 10), (0, 0), 2.0), ch), &Options::new().with_seed(10));
        let lit = |inst: &mut crate::fx::effect::FxInstance| {
            let mut f = Frame::new(40, 10, 0, 0);
            inst.draw(&mut f, None);
            let row = |y: usize| (0..40).filter(|&x| f.color[y * 40 + x] == C).collect::<Vec<_>>();
            (row(1), row(2))
        };
        let dt = 1.0 / 30.0;
        inst.update(dt);
        let (a, b) = lit(&mut inst);
        assert!(a.is_empty(), "first cue not started yet");
        assert_eq!(b, vec![11], "second cue: seed 10 + 1 at x = 1 + chain x(t)");
        for _ in 0..8 {
            inst.update(dt);
        }
        let (a, _) = lit(&mut inst);
        assert_eq!(a, vec![10], "first cue running, seed 10 + 0");
        for _ in 0..12 {
            inst.update(dt);
        }
        let (a, b) = lit(&mut inst);
        assert!(a.is_empty(), "cut at until");
        assert_eq!(b.len(), 1, "the second cue (1 s long) still runs at 0.7 s");
        for _ in 0..12 {
            inst.update(dt);
        }
        let (_, b) = lit(&mut inst);
        assert!(b.is_empty(), "a one-shot ends after its own duration");
    }

    #[test]
    fn flipped_chain_mirrors_cue_positions() {
        let ch = chain(vec![Cue::new(fx()).x(5.0)]);
        let mut o = Options::new().with_seed(0);
        o.x = 20.0;
        o.flip = true;
        let mut inst = create_fx(&Effect::composite(Meta::new("m", (40, 10), (0, 0), 2.0), ch), &o);
        inst.update(1.0 / 30.0);
        let mut f = Frame::new(40, 10, 0, 0);
        inst.draw(&mut f, None);
        // cue at 20 - 5 = 15, itself flipped: local px 0 lands on cell -1 from its origin
        assert!(f.color.iter().position(|&c| c == C).is_some_and(|i| i % 40 < 16));
    }
}
