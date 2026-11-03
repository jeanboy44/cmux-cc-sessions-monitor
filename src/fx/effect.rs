//! Effect modules and their live instances: a port of the instance half of pixelfx `core/fx.js` (`resolveOptions`,
//! `createFx`). The frame half (`createFrame`, `composite`) lives in [`super::frame`].
//!
//! An upstream effect module is `{ meta, draw(p, t, o), drawBack?, create?(o), actorAlpha?, act? }`. Here it is an
//! [`Effect`]: its [`Meta`] plus one of
//! - [`Effect::stateless`]: `draw(p, t, o)` (and optionally `draw_back`) of the effect's time only;
//! - [`Effect::stateful`]: `create(o) -> Box<dyn EffectImpl>` for effects that simulate (`export function create`);
//! - [`Effect::composite`]: a [`Chain`] of other effects (`create = chain([...])`).
//!
//! A module's `act` (when another move reuses its poses) is a plain `pub fn act() -> Act` next to the effect.

use super::chain::{Chain, ChainInst};
use super::color::fit;
use super::frame::Frame;
use super::math::clamp;
use super::painter::{Layer, Painter};
use std::collections::BTreeMap;

/// An effect's options: placement (set by whoever plays it), the common keys, and named extras.
///
/// Every common key is optional so an `Options` can be both a module's defaults (`meta.options`) and a caller's
/// overrides; [`resolve_options`] lays the overrides on the defaults. Effects read them through the getters, which
/// return what JS would compute for a missing key (`num` gives NaN like `undefined` arithmetic, ramps are empty).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Options {
    /// Placement in frame space: set by the host (an act or a chain), not by defaults.
    pub x: f64,
    pub y: f64,
    pub flip: bool,
    pub angle: f64,
    pub seed: Option<u32>,
    /// Glow strength (`light`, default 1).
    pub light: Option<f64>,
    /// The main color ramp, dark to bright.
    pub palette: Option<Vec<u32>>,
    /// Other numeric options (`radius`, `height`, ...), by upstream name.
    pub nums: BTreeMap<&'static str, f64>,
    /// Other color ramps (`sparks`, `smoke`, ...), by upstream name.
    pub ramps: BTreeMap<&'static str, Vec<u32>>,
    /// Boolean options, by upstream name.
    pub flags: BTreeMap<&'static str, bool>,
}
impl Options {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn with_seed(mut self, s: u32) -> Self {
        self.seed = Some(s);
        self
    }
    pub fn with_light(mut self, k: f64) -> Self {
        self.light = Some(k);
        self
    }
    pub fn with_palette(mut self, r: &[u32]) -> Self {
        self.palette = Some(r.to_vec());
        self
    }
    pub fn with_num(mut self, name: &'static str, v: f64) -> Self {
        self.nums.insert(name, v);
        self
    }
    pub fn with_ramp(mut self, name: &'static str, r: &[u32]) -> Self {
        self.ramps.insert(name, r.to_vec());
        self
    }
    pub fn with_flag(mut self, name: &'static str, v: bool) -> Self {
        self.flags.insert(name, v);
        self
    }
    /// The seed (1 when nobody set one).
    pub fn seed(&self) -> u32 {
        self.seed.unwrap_or(1)
    }
    pub fn light(&self) -> f64 {
        self.light.unwrap_or(1.0)
    }
    pub fn palette(&self) -> &[u32] {
        self.palette.as_deref().unwrap_or(&[])
    }
    /// A numeric option; NaN when missing (as JS `undefined` would turn arithmetic into).
    pub fn num(&self, name: &str) -> f64 {
        self.nums.get(name).copied().unwrap_or(f64::NAN)
    }
    pub fn num_or(&self, name: &str, d: f64) -> f64 {
        self.nums.get(name).copied().unwrap_or(d)
    }
    pub fn has(&self, name: &str) -> bool {
        self.nums.contains_key(name) || self.ramps.contains_key(name) || self.flags.contains_key(name)
    }
    /// A color ramp option; empty when missing.
    pub fn ramp(&self, name: &str) -> &[u32] {
        self.ramps.get(name).map_or(&[], |v| v.as_slice())
    }
    pub fn flag(&self, name: &str) -> bool {
        self.flags.get(name).copied().unwrap_or(false)
    }
    /// `{ ...self, ...over }`: placement comes from `over`, every key `over` sets wins.
    pub fn overlay(&self, over: &Options) -> Options {
        let mut o = self.clone();
        o.x = over.x;
        o.y = over.y;
        o.flip = over.flip;
        o.angle = over.angle;
        if over.seed.is_some() {
            o.seed = over.seed;
        }
        if over.light.is_some() {
            o.light = over.light;
        }
        if over.palette.is_some() {
            o.palette = over.palette.clone();
        }
        o.nums.extend(over.nums.iter().map(|(k, v)| (*k, *v)));
        o.ramps.extend(over.ramps.iter().map(|(k, v)| (*k, v.clone())));
        o.flags.extend(over.flags.iter().map(|(k, v)| (*k, *v)));
        o
    }
}

/// An effect's description (upstream `meta`). `size`/`origin` are its preview box in px and where its (0, 0) sits in it.
#[derive(Clone, Debug, PartialEq)]
pub struct Meta {
    pub name: &'static str,
    pub size: (i32, i32),
    pub origin: (i32, i32),
    pub duration: f64,
    /// Upstream `loop`.
    pub looping: bool,
    pub fps: f64,
    /// Upstream `actor`: 2 when the effect is shown on the 24 x 38 stand-in.
    pub actor: Option<u8>,
    /// The effect's default options.
    pub options: Options,
}
impl Meta {
    /// A one-shot at 30 fps with no defaults.
    pub fn new(name: &'static str, size: (i32, i32), origin: (i32, i32), duration: f64) -> Self {
        Meta { name, size, origin, duration, looping: false, fps: 30.0, actor: None, options: Options::new() }
    }
    pub fn looping(mut self, v: bool) -> Self {
        self.looping = v;
        self
    }
    pub fn fps(mut self, v: f64) -> Self {
        self.fps = v;
        self
    }
    pub fn actor(mut self, k: u8) -> Self {
        self.actor = Some(k);
        self
    }
    pub fn options(mut self, o: Options) -> Self {
        self.options = o;
        self
    }
}

/// A live effect's behaviour (what upstream `create(o)` returns: `{ update?, draw, drawBack? }`).
/// `t` is the effect's own time (wrapped for loops, held at the end for one-shots).
pub trait EffectImpl {
    fn update(&mut self, _dt: f64, _o: &Options) {}
    /// What goes behind a character.
    fn draw_back(&mut self, _p: &mut Painter, _t: f64, _o: &Options) {}
    /// What goes in front.
    fn draw(&mut self, p: &mut Painter, t: f64, o: &Options);
}

pub type DrawFn = fn(&mut Painter, f64, &Options);
pub type CreateFn = fn(&Options) -> Box<dyn EffectImpl>;
/// How visible an effect wants the stand-in character at time t (upstream `actorAlpha`).
pub type ActorAlphaFn = fn(f64, &Options) -> f64;

#[derive(Clone)]
pub enum EffectKind {
    Stateless { draw: DrawFn, draw_back: Option<DrawFn> },
    Stateful(CreateFn),
    Composite(Chain),
}

/// An effect module.
#[derive(Clone)]
pub struct Effect {
    pub meta: Meta,
    pub kind: EffectKind,
    pub actor_alpha: Option<ActorAlphaFn>,
}
impl Effect {
    pub fn stateless(meta: Meta, draw: DrawFn) -> Self {
        Effect { meta, kind: EffectKind::Stateless { draw, draw_back: None }, actor_alpha: None }
    }
    pub fn stateful(meta: Meta, create: CreateFn) -> Self {
        Effect { meta, kind: EffectKind::Stateful(create), actor_alpha: None }
    }
    pub fn composite(meta: Meta, chain: Chain) -> Self {
        Effect { meta, kind: EffectKind::Composite(chain), actor_alpha: None }
    }
    /// Add a `drawBack` to a stateless effect (ignored for the other kinds, which implement it themselves).
    pub fn with_back(mut self, f: DrawFn) -> Self {
        if let EffectKind::Stateless { draw_back, .. } = &mut self.kind {
            *draw_back = Some(f);
        }
        self
    }
    pub fn with_actor_alpha(mut self, f: ActorAlphaFn) -> Self {
        self.actor_alpha = Some(f);
        self
    }
    /// The stand-in's alpha at time t.
    pub fn actor_alpha_at(&self, t: f64, o: &Options) -> f64 {
        self.actor_alpha.map_or(1.0, |f| f(t, o))
    }
}

struct Stateless {
    draw: DrawFn,
    draw_back: Option<DrawFn>,
}
impl EffectImpl for Stateless {
    fn draw_back(&mut self, p: &mut Painter, t: f64, o: &Options) {
        if let Some(f) = self.draw_back {
            f(p, t, o)
        }
    }
    fn draw(&mut self, p: &mut Painter, t: f64, o: &Options) {
        (self.draw)(p, t, o)
    }
}

/// An effect's options: its defaults with the caller's on top. A color ramp (the palette or a named ramp) of a different
/// length than its default is resampled to the default's length, so effects can index steps by number.
pub fn resolve_options(meta: &Meta, o: &Options) -> Options {
    let def = &meta.options;
    let mut r = def.overlay(o);
    if let (Some(d), Some(p)) = (&def.palette, &o.palette) {
        if !d.is_empty() && !p.is_empty() && p.len() != d.len() {
            r.palette = Some(fit(p, d.len()));
        }
    }
    for (k, d) in &def.ramps {
        if let Some(p) = o.ramps.get(k) {
            if !d.is_empty() && !p.is_empty() && p.len() != d.len() {
                r.ramps.insert(k, fit(p, d.len()));
            }
        }
    }
    r
}

const FADE: f64 = 0.3;

/// A live instance of an effect (upstream `createFx`): `update(dt)`, `draw(frame, layer)`, `stop()`, `done()`.
pub struct FxInstance {
    pub meta: Meta,
    /// Resolved options; hosts move `x`, `y`, `flip` between draws.
    pub options: Options,
    t: f64,
    stop_at: f64,
    has_update: bool,
    imp: Box<dyn EffectImpl>,
}
/// Start an effect. `o`: placement plus any overrides of its options.
pub fn create_fx(fx: &Effect, o: &Options) -> FxInstance {
    let options = resolve_options(&fx.meta, o);
    let (imp, has_update): (Box<dyn EffectImpl>, bool) = match &fx.kind {
        EffectKind::Stateless { draw, draw_back } => (Box::new(Stateless { draw: *draw, draw_back: *draw_back }), false),
        EffectKind::Stateful(create) => (create(&options), true),
        EffectKind::Composite(chain) => (Box::new(ChainInst::new(chain, &options)), true),
    };
    FxInstance { meta: fx.meta.clone(), options, t: 0.0, stop_at: f64::INFINITY, has_update, imp }
}
impl FxInstance {
    pub fn t(&self) -> f64 {
        self.t
    }
    pub fn update(&mut self, dt: f64) {
        self.t += dt;
        if !self.has_update {
            return;
        }
        // simulations step at 120 Hz or finer whatever the caller's frame rate, so they look the same everywhere
        let n = (dt * 120.0 - 1e-9).ceil().max(1.0);
        let n = if n.is_finite() { n.min(1e6) as usize } else { 1 };
        for _ in 0..n {
            self.imp.update(dt / n as f64, &self.options);
        }
    }
    /// Draw into a frame: `Some(Layer::Back)` what goes behind a character, `Some(Layer::Front)` what goes in front,
    /// `None` both.
    pub fn draw(&mut self, frame: &mut Frame, layer: Option<Layer>) {
        let mut p = Painter::new(frame);
        let o = &self.options;
        p.place(o.x, o.y, o.flip, o.angle);
        p.fade = if self.stop_at == f64::INFINITY { 1.0 } else { clamp(1.0 - (self.t - self.stop_at) / FADE, 0.0, 1.0) };
        p.light_k = o.light();
        let d = self.meta.duration;
        let tt = if self.meta.looping { self.t % d } else { self.t.min(d) };
        if layer != Some(Layer::Front) {
            p.layer = Layer::Back;
            self.imp.draw_back(&mut p, tt, o);
        }
        if layer != Some(Layer::Back) {
            p.layer = Layer::Front;
            self.imp.draw(&mut p, tt, o);
        }
    }
    /// Let it end: loops fade out, one-shots cut short.
    pub fn stop(&mut self) {
        if self.stop_at == f64::INFINITY {
            self.stop_at = self.t;
        }
    }
    pub fn done(&self) -> bool {
        self.t - self.stop_at >= FADE || (!self.meta.looping && self.t >= self.meta.duration)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_overlay_and_ramp_resampling() {
        let meta = Meta::new("e", (8, 8), (4, 4), 1.0).options(
            Options::new().with_seed(2).with_palette(&[1, 2, 3, 4]).with_num("radius", 42.0).with_ramp("sparks", &[5, 6, 7]),
        );
        let o = resolve_options(&meta, &Options::new().with_seed(9).with_palette(&[0xFF00_0000, 0xFFFF_FFFF]).with_ramp("sparks", &[8, 9, 10]));
        assert_eq!(o.seed(), 9);
        assert_eq!(o.palette().len(), 4, "palette resampled to the default's length");
        assert_eq!((o.palette()[0], o.palette()[3]), (0xFF00_0000, 0xFFFF_FFFF));
        assert_eq!(o.ramp("sparks"), &[8, 9, 10]);
        assert_eq!(o.num("radius"), 42.0);
        assert!(o.num("missing").is_nan());
        assert!(o.ramp("missing").is_empty());
        let d = resolve_options(&meta, &Options::new());
        assert_eq!((d.seed(), d.light()), (2, 1.0));
    }

    fn red(p: &mut Painter, t: f64, _o: &Options) {
        p.px(t * 10.0, 0.0, 0xFF00_00FF);
    }
    #[test]
    fn instance_holds_one_shots_and_stops() {
        let fx = Effect::stateless(Meta::new("e", (8, 8), (0, 0), 0.5), red);
        let mut i = create_fx(&fx, &Options::new());
        for _ in 0..30 {
            i.update(1.0 / 30.0);
        }
        assert!(i.done());
        let mut f = Frame::new(10, 2, 0, 0);
        i.draw(&mut f, None);
        assert_eq!(f.color[5], 0xFF00_00FF, "time held at the duration");
        let lp = Effect::stateless(Meta::new("e", (8, 8), (0, 0), 0.5).looping(true), red);
        let mut i = create_fx(&lp, &Options::new());
        i.update(1.0);
        assert!(!i.done());
        i.stop();
        i.update(0.31);
        assert!(i.done());
    }
}
