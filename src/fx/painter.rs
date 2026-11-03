//! The painter draws into a frame's two buffers: `color` (opaque pixels, faded with ordered dither, never
//! blended, except by `blend_px`/`disc`) and `light` (additive glow, composited on top). Effects draw in local
//! coordinates around their origin; the painter places, mirrors and rotates them, so an effect never needs to
//! know where it is. Port of pixelfx `core/painter.js`.
//!
//! Skipped: `text` and `measure` (they need upstream's bitmap fonts, which no planned theme uses).
//!
//! # "Value or function" arguments
//!
//! Upstream lets most color and alpha arguments be either a constant or a function of the shape's own
//! coordinates. Here those parameters are `impl Into<Val<T>>`: pass a plain `u32` / `f64` for a constant, or
//! build a function with [`Val::f1`] (one coordinate, e.g. `line`'s `u`) or [`Val::f2`] (two coordinates:
//! `(d, angle)` for circles and discs, `(u, v)` for beams and arcs, `(x, y)` for polygons, `(mx, my)` for
//! masks). `Val::f1` closures ignore the second coordinate. Examples:
//!
//! ```ignore
//! p.circle(0.0, 0.0, 8.0, Val::f2(|d, _| mix(WHITE, BLU, d)), 1.0);
//! p.line(0.0, 0.0, 9.0, 0.0, Val::f1(|u| mix(RED, YEL, u)), Val::f1(|u| 1.0 - u));
//! ```
//!
//! Alpha / strength arguments that upstream defaults (`a = 1`, `k = 0.5`) are explicit here.

use super::frame::Frame;
use super::math::{clamp01, TAU};

/// 4x4 ordered (Bayer) thresholds: a pixel at alpha a is drawn when a is above its cell's threshold.
const BAYER_RAW: [u8; 16] = [0, 8, 2, 10, 12, 4, 14, 6, 3, 11, 1, 9, 15, 7, 13, 5];
fn bayer(i: usize) -> f64 {
    (BAYER_RAW[i] as f64 + 0.5) / 16.0
}

/// A constant, or a function of up to two shape coordinates. See the module docs.
pub enum Val<'a, T> {
    K(T),
    F(Box<dyn Fn(f64, f64) -> T + 'a>),
}
impl<'a, T: Copy> Val<'a, T> {
    /// A function of one coordinate.
    pub fn f1(f: impl Fn(f64) -> T + 'a) -> Self {
        Val::F(Box::new(move |a, _| f(a)))
    }
    /// A function of two coordinates.
    pub fn f2(f: impl Fn(f64, f64) -> T + 'a) -> Self {
        Val::F(Box::new(f))
    }
    /// Evaluate at (a, b).
    pub fn at(&self, a: f64, b: f64) -> T {
        match self {
            Val::K(v) => *v,
            Val::F(f) => f(a, b),
        }
    }
}
impl From<u32> for Val<'_, u32> {
    fn from(v: u32) -> Self {
        Val::K(v)
    }
}
impl From<f64> for Val<'_, f64> {
    fn from(v: f64) -> Self {
        Val::K(v)
    }
}

/// Output of a `shade` callback: `c` color, `a` alpha, `l` light color (defaults to the color), `k` light amount.
#[derive(Clone, Copy, Debug)]
pub struct Out {
    pub c: u32,
    pub a: f64,
    pub l: u32,
    pub k: f64,
}

/// A bitmap mask: `data[y * w + x] != 0` is set.
#[derive(Clone, Debug)]
pub struct Mask {
    pub w: usize,
    pub h: usize,
    pub data: Vec<u8>,
}

/// Options for `sprite`: `scale`, alpha `a`, `flip` (mirror left-right), `flip_y`, `tint` (one color for all).
#[derive(Clone, Copy, Debug)]
pub struct SpriteOpts {
    pub scale: f64,
    pub a: f64,
    pub flip: bool,
    pub flip_y: bool,
    pub tint: Option<u32>,
}
impl Default for SpriteOpts {
    fn default() -> Self {
        SpriteOpts { scale: 1.0, a: 1.0, flip: false, flip_y: false, tint: None }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Layer {
    Back,
    Front,
}

fn fl(x: f64) -> i32 {
    x.floor() as i32
}
/// JS `Math.round`: halves round up.
fn js_round(x: f64) -> i32 {
    (x + 0.5).floor() as i32
}

pub struct Painter<'a> {
    pub frame: &'a mut Frame,
    w: i32,
    h: i32,
    ox: i32,
    oy: i32,
    tx: f64,
    ty: f64,
    fx: f64,
    cs: f64,
    sn: f64,
    /// Fade multiplier (0..1) applied to every write; dithers color, scales light.
    pub fade: f64,
    /// Light strength multiplier.
    pub light_k: f64,
    pub layer: Layer,
}

impl<'a> Painter<'a> {
    pub fn new(frame: &'a mut Frame) -> Self {
        let (w, h, ox, oy) = (frame.w as i32, frame.h as i32, frame.ox, frame.oy);
        Painter { frame, w, h, ox, oy, tx: 0.0, ty: 0.0, fx: 1.0, cs: 1.0, sn: 0.0, fade: 1.0, light_k: 1.0, layer: Layer::Front }
    }

    /// Clear placement, fade and light strength (re-reads the frame's origin).
    pub fn reset(&mut self) -> &mut Self {
        self.w = self.frame.w as i32;
        self.h = self.frame.h as i32;
        self.ox = self.frame.ox;
        self.oy = self.frame.oy;
        self.tx = 0.0;
        self.ty = 0.0;
        self.fx = 1.0;
        self.cs = 1.0;
        self.sn = 0.0;
        self.fade = 1.0;
        self.light_k = 1.0;
        self.layer = Layer::Front;
        self
    }

    /// Put the effect's origin at (x, y) in frame space, mirrored if `flip`, rotated by `angle` (radians).
    pub fn place(&mut self, x: f64, y: f64, flip: bool, angle: f64) -> &mut Self {
        self.tx = x;
        self.ty = y;
        self.fx = if flip { -1.0 } else { 1.0 };
        self.cs = angle.cos();
        self.sn = angle.sin();
        self
    }

    // ---- transforms: local (effect) space <-> screen (frame) space
    pub fn to_x(&self, x: f64, y: f64) -> f64 {
        self.tx + x * self.fx * self.cs - y * self.sn
    }
    pub fn to_y(&self, x: f64, y: f64) -> f64 {
        self.ty + x * self.fx * self.sn + y * self.cs
    }
    pub fn from_x(&self, sx: f64, sy: f64) -> f64 {
        let (dx, dy) = (sx - self.tx, sy - self.ty);
        (dx * self.cs + dy * self.sn) * self.fx
    }
    pub fn from_y(&self, sx: f64, sy: f64) -> f64 {
        let (dx, dy) = (sx - self.tx, sy - self.ty);
        -dx * self.sn + dy * self.cs
    }
    /// The screen pixel of the local pixel cell holding (x, y): mirrors and rotations land on whole cells.
    pub fn cell_x(&self, x: f64, y: f64) -> i32 {
        fl(self.to_x(x.floor() + 0.5, y.floor() + 0.5))
    }
    pub fn cell_y(&self, x: f64, y: f64) -> i32 {
        fl(self.to_y(x.floor() + 0.5, y.floor() + 0.5))
    }

    // ---- raw writes, screen space, whole pixels. Color 0 means "nothing": it never erases what's there.
    pub fn put(&mut self, sx: i32, sy: i32, c: u32, a: f64) {
        if c == 0 {
            return;
        }
        let a = a * self.fade;
        if a < 1.0 && a <= bayer((((sy & 3) << 2) | (sx & 3)) as usize) {
            return;
        }
        let (x, y) = (sx.wrapping_sub(self.ox), sy.wrapping_sub(self.oy));
        if x < 0 || y < 0 || x >= self.w || y >= self.h {
            return;
        }
        self.frame.color[(y * self.w + x) as usize] = c;
    }
    pub fn add(&mut self, sx: i32, sy: i32, c: u32, k: f64) {
        let k = k * self.fade * self.light_k;
        if k <= 0.004 {
            return;
        }
        let (x, y) = (sx.wrapping_sub(self.ox), sy.wrapping_sub(self.oy));
        if x < 0 || y < 0 || x >= self.w || y >= self.h {
            return;
        }
        let i = (y * self.w + x) as usize;
        let d = self.frame.light[i];
        let ch = |s: u32| (((d >> s) & 255) as f64 + ((c >> s) & 255) as f64 * k).min(255.0) as u32;
        self.frame.light[i] = (255 << 24) | (ch(16) << 16) | (ch(8) << 8) | ch(0);
    }
    /// Blend a color over what's there at alpha a (0..1), keeping the result's own alpha: soft smoke, vapour
    /// and mist. Use dithered `dot` for hard pixel fades and `blend` for translucent volumes.
    pub fn blend_px(&mut self, sx: i32, sy: i32, c: u32, a: f64) {
        let a = a * self.fade;
        if c == 0 || a <= 0.004 {
            return;
        }
        let (x, y) = (sx.wrapping_sub(self.ox), sy.wrapping_sub(self.oy));
        if x < 0 || y < 0 || x >= self.w || y >= self.h {
            return;
        }
        let i = (y * self.w + x) as usize;
        let d = self.frame.color[i];
        let da = (d >> 24) as f64 / 255.0;
        let oa = a + da * (1.0 - a);
        let k = a / oa;
        let ch = |s: u32| ((((c >> s) & 255) as f64 * k + ((d >> s) & 255) as f64 * (1.0 - k)) as i32 & 255) as u32;
        self.frame.color[i] = (((oa * 255.0) as i32 & 255) as u32) << 24 | ch(16) << 16 | ch(8) << 8 | ch(0);
    }
    /// Translucent pixel at a local point.
    pub fn blend(&mut self, x: f64, y: f64, c: u32, a: f64) {
        let (sx, sy) = (self.cell_x(x, y), self.cell_y(x, y));
        self.blend_px(sx, sy, c, a);
    }
    /// A translucent disc: puffs of smoke, vapour, dust. `c` and `a` may be functions of (d: 0 center..1 rim, angle).
    pub fn disc<'v>(&mut self, cx: f64, cy: f64, r: f64, c: impl Into<Val<'v, u32>>, a: impl Into<Val<'v, f64>>) {
        let (c, a) = (c.into(), a.into());
        if r < 0.5 {
            return self.blend(cx, cy, c.at(0.0, 0.0), a.at(0.0, 0.0));
        }
        self.each(cx - r, cy - r, cx + r, cy + r, |p, x, y, sx, sy| {
            let (dx, dy) = (x - cx, y - cy);
            let d2 = dx * dx + dy * dy;
            if d2 > r * r {
                return;
            }
            let d = d2.sqrt() / r;
            let ang = dy.atan2(dx);
            p.blend_px(sx, sy, c.at(d, ang), a.at(d, ang));
        });
    }
    /// Read back the color under a local point (0 if empty).
    pub fn get(&self, x: f64, y: f64) -> u32 {
        let sx = self.cell_x(x, y) - self.ox;
        let sy = self.cell_y(x, y) - self.oy;
        if sx < 0 || sy < 0 || sx >= self.w || sy >= self.h {
            0
        } else {
            self.frame.color[(sy * self.w + sx) as usize]
        }
    }

    // ---- pixels
    /// One opaque pixel.
    pub fn px(&mut self, x: f64, y: f64, c: u32) {
        let (sx, sy) = (self.cell_x(x, y), self.cell_y(x, y));
        self.put(sx, sy, c, 1.0);
    }
    /// One pixel at alpha a (0..1), faded by ordered dither.
    pub fn dot(&mut self, x: f64, y: f64, c: u32, a: f64) {
        if a > 0.0 {
            let (sx, sy) = (self.cell_x(x, y), self.cell_y(x, y));
            self.put(sx, sy, c, a);
        }
    }
    /// Add light to one pixel.
    pub fn light(&mut self, x: f64, y: f64, c: u32, k: f64) {
        let (sx, sy) = (self.cell_x(x, y), self.cell_y(x, y));
        self.add(sx, sy, c, k);
    }

    /// Visit every screen pixel whose center falls in the local box [x0, x1] x [y0, y1], with that center in
    /// local space: `f(painter, lx, ly, sx, sy)`. The basis of every filled shape, so shapes mirror and rotate
    /// exactly. The painter is passed back so the callback can write.
    pub fn each(&mut self, x0: f64, y0: f64, x1: f64, y1: f64, mut f: impl FnMut(&mut Self, f64, f64, i32, i32)) {
        let (mut ax, mut bx, mut ay, mut by) = (f64::INFINITY, f64::NEG_INFINITY, f64::INFINITY, f64::NEG_INFINITY);
        for k in 0..4 {
            let x = if k & 1 != 0 { x1 } else { x0 };
            let y = if k & 2 != 0 { y1 } else { y0 };
            let (sx, sy) = (self.to_x(x, y), self.to_y(x, y));
            ax = ax.min(sx);
            bx = bx.max(sx);
            ay = ay.min(sy);
            by = by.max(sy);
        }
        let sx0 = fl(ax).max(self.ox);
        let sx1 = (bx.ceil() as i32).min(self.ox + self.w - 1);
        let sy0 = fl(ay).max(self.oy);
        let sy1 = (by.ceil() as i32).min(self.oy + self.h - 1);
        for sy in sy0..=sy1 {
            for sx in sx0..=sx1 {
                let (cx, cy) = (sx as f64 + 0.5, sy as f64 + 0.5);
                let (lx, ly) = (self.from_x(cx, cy), self.from_y(cx, cy));
                f(self, lx, ly, sx, sy);
            }
        }
    }

    /// A pixel shader over a local box. `f(x, y, out)` returns a color, or sets `out.c` (color), `out.a`
    /// (alpha), `out.l` (light color, defaults to the color) and `out.k` (light amount) and returns 0.
    /// Leave `out.c` at 0 and return 0 to leave the pixel alone.
    pub fn shade(&mut self, x0: f64, y0: f64, x1: f64, y1: f64, mut f: impl FnMut(f64, f64, &mut Out) -> u32) {
        self.each(x0, y0, x1, y1, |p, lx, ly, sx, sy| {
            let mut o = Out { c: 0, a: 1.0, l: 0, k: 0.0 };
            let r = f(lx, ly, &mut o);
            let c = if r != 0 { r } else { o.c };
            if c != 0 {
                p.put(sx, sy, c, o.a);
            }
            if o.k > 0.0 {
                p.add(sx, sy, if o.l != 0 { o.l } else { c }, o.k);
            }
        });
    }

    // ---- shapes. Colors (c) and alphas (a) can be constants or functions of the shape's own coordinates.
    /// 1 px line. `c` and `a` may be functions of u (0 at the start, 1 at the end; use `Val::f1`).
    pub fn line<'v>(&mut self, x0: f64, y0: f64, x1: f64, y1: f64, c: impl Into<Val<'v, u32>>, a: impl Into<Val<'v, f64>>) {
        let (c, a) = (c.into(), a.into());
        let (mut sx, mut sy) = (self.cell_x(x0, y0), self.cell_y(x0, y0));
        let (ex, ey) = (self.cell_x(x1, y1), self.cell_y(x1, y1));
        let dx = (ex - sx).abs();
        let dy = -(ey - sy).abs();
        let stepx = if sx < ex { 1 } else { -1 };
        let stepy = if sy < ey { 1 } else { -1 };
        let n = if dx.max(-dy) == 0 { 1.0 } else { dx.max(-dy) as f64 };
        let mut err = dx + dy;
        let mut i = 0;
        loop {
            let u = i as f64 / n;
            i += 1;
            self.put(sx, sy, c.at(u, 0.0), a.at(u, 0.0));
            if sx == ex && sy == ey {
                break;
            }
            let e2 = 2 * err;
            if e2 >= dy {
                err += dy;
                sx += stepx;
            }
            if e2 <= dx {
                err += dx;
                sy += stepy;
            }
        }
    }
    /// Lines through a list of points.
    pub fn polyline<'v>(&mut self, pts: &[(f64, f64)], c: impl Into<Val<'v, u32>>, a: impl Into<Val<'v, f64>>) {
        let (c, a) = (c.into(), a.into());
        for i in 1..pts.len() {
            let (p, q) = (pts[i - 1], pts[i]);
            self.line(p.0, p.1, q.0, q.1, Val::f1(|u| c.at(u, 0.0)), Val::f1(|u| a.at(u, 0.0)));
        }
    }
    /// Thick segment with round ends. `c` and `a` may be functions of (u along, v across: 0 center, 1 edge).
    pub fn beam<'v>(&mut self, x0: f64, y0: f64, x1: f64, y1: f64, width: f64, c: impl Into<Val<'v, u32>>, a: impl Into<Val<'v, f64>>) {
        let (c, a) = (c.into(), a.into());
        let r = width / 2.0;
        let (dx, dy) = (x1 - x0, y1 - y0);
        let l2 = {
            let v = dx * dx + dy * dy;
            if v == 0.0 { 1e-9 } else { v }
        };
        self.shade(x0.min(x1) - r, y0.min(y1) - r, x0.max(x1) + r, y0.max(y1) + r, |x, y, o| {
            let u = clamp01(((x - x0) * dx + (y - y0) * dy) / l2);
            let (px, py) = (x0 + dx * u - x, y0 + dy * u - y);
            let d2 = px * px + py * py;
            if d2 > r * r {
                return 0;
            }
            let v = d2.sqrt() / r;
            o.c = c.at(u, v);
            o.a = a.at(u, v);
            0
        });
    }
    /// Filled disc. `c` and `a` may be functions of (d: 0 center to 1 rim, angle).
    pub fn circle<'v>(&mut self, cx: f64, cy: f64, r: f64, c: impl Into<Val<'v, u32>>, a: impl Into<Val<'v, f64>>) {
        let (c, a) = (c.into(), a.into());
        if r < 0.6 {
            return self.dot(cx, cy, c.at(0.0, 0.0), a.at(0.0, 0.0));
        }
        self.shade(cx - r, cy - r, cx + r, cy + r, |x, y, o| {
            let (dx, dy) = (x - cx, y - cy);
            let d2 = dx * dx + dy * dy;
            if d2 > r * r {
                return 0;
            }
            let d = d2.sqrt() / r;
            let ang = dy.atan2(dx);
            o.c = c.at(d, ang);
            o.a = a.at(d, ang);
            0
        });
    }
    /// Ellipse. `c` and `a` may be functions of (d: 0 center to 1 rim, angle).
    pub fn ellipse<'v>(&mut self, cx: f64, cy: f64, rx: f64, ry: f64, c: impl Into<Val<'v, u32>>, a: impl Into<Val<'v, f64>>) {
        if rx <= 0.0 || ry <= 0.0 {
            return;
        }
        let (c, a) = (c.into(), a.into());
        self.shade(cx - rx, cy - ry, cx + rx, cy + ry, |x, y, o| {
            let (dx, dy) = ((x - cx) / rx, (y - cy) / ry);
            let d2 = dx * dx + dy * dy;
            if d2 > 1.0 {
                return 0;
            }
            let d = d2.sqrt();
            let ang = dy.atan2(dx);
            o.c = c.at(d, ang);
            o.a = a.at(d, ang);
            0
        });
    }
    /// Ring from radius r - w to r. `c` and `a` may be functions of (v: 0 inner to 1 outer, angle).
    pub fn ring<'v>(&mut self, cx: f64, cy: f64, r: f64, w: f64, c: impl Into<Val<'v, u32>>, a: impl Into<Val<'v, f64>>) {
        let (c, a) = (c.into(), a.into());
        let r0 = (r - w).max(0.0);
        self.shade(cx - r, cy - r, cx + r, cy + r, |x, y, o| {
            let (dx, dy) = (x - cx, y - cy);
            let d2 = dx * dx + dy * dy;
            if d2 > r * r || d2 < r0 * r0 {
                return 0;
            }
            let v = if w > 0.0 {
                let den = r - r0;
                (d2.sqrt() - r0) / if den == 0.0 { 1.0 } else { den }
            } else {
                1.0
            };
            let ang = dy.atan2(dx);
            o.c = c.at(v, ang);
            o.a = a.at(v, ang);
            0
        });
    }
    /// Band of a ring from angle a0 to a1 (radians, a1 > a0), radius r - w to r: the slash and smear shape.
    /// `c` and `a` may be functions of (u: 0 at a0 to 1 at a1, v: 0 inner to 1 outer). `w` may be a function
    /// of u (via `Val::f1`) for a crescent that tapers at both ends.
    #[allow(clippy::too_many_arguments)]
    pub fn arc<'v>(
        &mut self,
        cx: f64,
        cy: f64,
        r: f64,
        w: impl Into<Val<'v, f64>>,
        a0: f64,
        a1: f64,
        c: impl Into<Val<'v, u32>>,
        a: impl Into<Val<'v, f64>>,
    ) {
        let (w, c, a) = (w.into(), c.into(), a.into());
        let span = a1 - a0;
        self.shade(cx - r, cy - r, cx + r, cy + r, |x, y, o| {
            let (dx, dy) = (x - cx, y - cy);
            let d = (dx * dx + dy * dy).sqrt();
            if d > r {
                return 0;
            }
            let mut ang = dy.atan2(dx) - a0;
            ang -= (ang / TAU).floor() * TAU;
            if ang > span {
                return 0;
            }
            let u = ang / span;
            let ww = w.at(u, 0.0);
            if ww <= 0.0 || d < r - ww {
                return 0;
            }
            let v = (d - (r - ww)) / ww;
            o.c = c.at(u, v);
            o.a = a.at(u, v);
            0
        });
    }
    /// Filled rectangle.
    pub fn rect(&mut self, x: f64, y: f64, w: f64, h: f64, c: u32, a: f64) {
        if self.sn == 0.0 {
            let (x0, y0, x1, y1) = (js_round(x), js_round(y), js_round(x + w), js_round(y + h));
            for yy in y0..y1 {
                for xx in x0..x1 {
                    let (sx, sy) = (self.cell_x(xx as f64, yy as f64), self.cell_y(xx as f64, yy as f64));
                    self.put(sx, sy, c, a);
                }
            }
            return;
        }
        self.shade(x, y, x + w, y + h, |lx, ly, o| {
            if lx < x || ly < y || lx >= x + w || ly >= y + h {
                return 0;
            }
            o.c = c;
            o.a = a;
            0
        });
    }
    /// Filled polygon from points (even-odd rule). `c` and `a` may be functions of (x, y).
    pub fn poly<'v>(&mut self, pts: &[(f64, f64)], c: impl Into<Val<'v, u32>>, a: impl Into<Val<'v, f64>>) {
        let (c, a) = (c.into(), a.into());
        let (mut x0, mut y0, mut x1, mut y1) = (f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
        for &(x, y) in pts {
            x0 = x0.min(x);
            x1 = x1.max(x);
            y0 = y0.min(y);
            y1 = y1.max(y);
        }
        let n = pts.len();
        self.shade(x0, y0, x1, y1, |x, y, o| {
            let mut inside = false;
            let mut j = n.wrapping_sub(1);
            for i in 0..n {
                let ((xi, yi), (xj, yj)) = (pts[i], pts[j]);
                if (yi > y) != (yj > y) && x < ((xj - xi) * (y - yi)) / (yj - yi) + xi {
                    inside = !inside;
                }
                j = i;
            }
            if !inside {
                return 0;
            }
            o.c = c.at(x, y);
            o.a = a.at(x, y);
            0
        });
    }
    #[allow(clippy::too_many_arguments)]
    pub fn tri<'v>(&mut self, x0: f64, y0: f64, x1: f64, y1: f64, x2: f64, y2: f64, c: impl Into<Val<'v, u32>>, a: impl Into<Val<'v, f64>>) {
        self.poly(&[(x0, y0), (x1, y1), (x2, y2)], c, a);
    }

    // ---- light
    /// Radial glow into the light buffer: k at the center, falling to 0 at radius r.
    pub fn glow(&mut self, x: f64, y: f64, r: f64, c: u32, k: f64) {
        if r < 1.0 || k <= 0.004 {
            return;
        }
        let (cx, cy) = (self.to_x(x, y), self.to_y(x, y));
        let (x0, x1, y0, y1) = (fl(cx - r), (cx + r).ceil() as i32, fl(cy - r), (cy + r).ceil() as i32);
        for sy in y0..=y1 {
            let dy = (sy as f64 + 0.5 - cy) / r;
            for sx in x0..=x1 {
                let dx = (sx as f64 + 0.5 - cx) / r;
                let d = dx * dx + dy * dy;
                if d >= 1.0 {
                    continue;
                }
                let f = 1.0 - d.sqrt();
                self.add(sx, sy, c, f * f * k);
            }
        }
    }
    /// Glow around a segment (a soft beam of light).
    #[allow(clippy::too_many_arguments)]
    pub fn glow_line(&mut self, x0: f64, y0: f64, x1: f64, y1: f64, r: f64, c: u32, k: f64) {
        if r < 1.0 || k <= 0.004 {
            return;
        }
        let (ax, ay, bx, by) = (self.to_x(x0, y0), self.to_y(x0, y0), self.to_x(x1, y1), self.to_y(x1, y1));
        let (dx, dy) = (bx - ax, by - ay);
        let l2 = {
            let v = dx * dx + dy * dy;
            if v == 0.0 { 1e-9 } else { v }
        };
        let (xa, xb) = (fl(ax.min(bx) - r), (ax.max(bx) + r).ceil() as i32);
        let (ya, yb) = (fl(ay.min(by) - r), (ay.max(by) + r).ceil() as i32);
        for sy in ya..=yb {
            for sx in xa..=xb {
                let (px, py) = (sx as f64 + 0.5, sy as f64 + 0.5);
                let u = clamp01(((px - ax) * dx + (py - ay) * dy) / l2);
                let (ex, ey) = (ax + dx * u - px, ay + dy * u - py);
                let d = ex.hypot(ey) / r;
                if d >= 1.0 {
                    continue;
                }
                let f = 1.0 - d;
                self.add(sx, sy, c, f * f * k);
            }
        }
    }
    /// Glow hugging a circle of radius `big_r` (a shockwave's light), falling off over width r.
    pub fn glow_ring(&mut self, x: f64, y: f64, big_r: f64, r: f64, c: u32, k: f64) {
        if r < 1.0 || k <= 0.004 {
            return;
        }
        let (cx, cy) = (self.to_x(x, y), self.to_y(x, y));
        let e = big_r + r;
        for sy in fl(cy - e)..=((cy + e).ceil() as i32) {
            for sx in fl(cx - e)..=((cx + e).ceil() as i32) {
                let d = ((sx as f64 + 0.5 - cx).hypot(sy as f64 + 0.5 - cy) - big_r).abs() / r;
                if d >= 1.0 {
                    continue;
                }
                let f = 1.0 - d;
                self.add(sx, sy, c, f * f * k);
            }
        }
    }

    // ---- sprites, masks
    /// A little sprite from strings: rows like `["..##..", ".#oo#."]`; `pal` maps characters to colors;
    /// "." and " " are empty, unmapped characters draw nothing. Top-left at (x, y).
    pub fn sprite(&mut self, rows: &[&str], pal: &[(char, u32)], x: f64, y: f64, o: &SpriteOpts) {
        let grid: Vec<Vec<char>> = rows.iter().map(|r| r.chars().collect()).collect();
        let k = o.scale;
        let wd = grid.first().map_or(0, |r| r.len());
        for (r, line) in grid.iter().enumerate() {
            for (q, &ch) in line.iter().enumerate() {
                if ch == '.' || ch == ' ' {
                    continue;
                }
                let c = o.tint.unwrap_or_else(|| pal.iter().find(|(k, _)| *k == ch).map_or(0, |(_, c)| *c));
                if c == 0 {
                    continue;
                }
                let col = if o.flip { wd as f64 - 1.0 - q as f64 } else { q as f64 };
                let row = if o.flip_y { grid.len() as f64 - 1.0 - r as f64 } else { r as f64 };
                if k == 1.0 {
                    self.dot(x + col, y + row, c, o.a);
                } else {
                    self.rect(x + col * k, y + row * k, k, k, c, o.a);
                }
            }
        }
    }
    /// Fill a mask with its top-left at (x, y). `c` and `a` may be functions of (mx, my).
    pub fn mask<'v>(&mut self, m: &Mask, x: f64, y: f64, c: impl Into<Val<'v, u32>>, a: impl Into<Val<'v, f64>>) {
        let (c, a) = (c.into(), a.into());
        for my in 0..m.h {
            for mx in 0..m.w {
                if m.data[my * m.w + mx] != 0 {
                    let (fx, fy) = (mx as f64, my as f64);
                    self.dot(x + fx, y + fy, c.at(fx, fy), a.at(fx, fy));
                }
            }
        }
    }
    /// The 1 px outline just outside a mask.
    pub fn outline<'v>(&mut self, m: &Mask, x: f64, y: f64, c: impl Into<Val<'v, u32>>, a: impl Into<Val<'v, f64>>) {
        let (c, a) = (c.into(), a.into());
        let at = |mx: i32, my: i32| mx >= 0 && my >= 0 && (mx as usize) < m.w && (my as usize) < m.h && m.data[my as usize * m.w + mx as usize] != 0;
        for my in -1..=(m.h as i32) {
            for mx in -1..=(m.w as i32) {
                if !at(mx, my) && (at(mx - 1, my) || at(mx + 1, my) || at(mx, my - 1) || at(mx, my + 1)) {
                    let (fx, fy) = (mx as f64, my as f64);
                    self.dot(x + fx, y + fy, c.at(fx, fy), a.at(fx, fy));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fx::color::{hex, mix};
    use crate::fx::frame::{composite_bytes, Bg, Frame};
    use crate::fx::shapes::star;

    fn scene(p: &mut Painter, with_fade: bool) {
        let (red, yel, blu, grn, wht) = (hex("#e43b44"), hex("#feae34"), hex("#0099db"), hex("#63c74d"), hex("#ffffff"));
        p.dot(-20.0, -15.0, wht, 1.0);
        p.px(-19.0, -15.0, yel);
        p.line(-24.0, -18.0, -4.0, -10.0, Val::f1(move |u| mix(red, yel, u)), 1.0);
        p.circle(-12.0, 4.0, 8.0, Val::f2(move |d, _| mix(wht, blu, d)), Val::f2(|d, _| 1.0 - d * 0.5));
        p.shade(2.0, -20.0, 22.0, -6.0, |x, y, o| {
            o.c = mix(grn, blu, (x - 2.0) / 20.0);
            o.a = (y + 20.0) / 14.0;
            o.k = 0.3;
            0
        });
        p.rect(8.0, 2.0, 10.0, 6.0, red, 0.6);
        p.ellipse(14.0, 14.0, 9.0, 4.0, yel, 1.0);
        p.ring(-14.0, 14.0, 7.0, 2.0, blu, 1.0);
        p.arc(0.0, 0.0, 18.0, 5.0, 0.2, 2.4, Val::f2(move |u, _| mix(yel, red, u)), 1.0);
        p.beam(-22.0, 20.0, 20.0, -14.0, 3.0, Val::f2(move |u, _| mix(blu, wht, u)), 0.8);
        p.poly(&star(18.0, 18.0, 3.0, 7.0, 5, -std::f64::consts::FRAC_PI_2), yel, 1.0);
        p.tri(-6.0, -22.0, 0.0, -14.0, -12.0, -14.0, grn, 1.0);
        p.disc(-4.0, 10.0, 6.0, Val::f2(move |d, _| mix(wht, red, d)), Val::f2(|d, _| 0.7 * (1.0 - d)));
        p.glow(0.0, 0.0, 14.0, yel, 0.6);
        p.glow_line(-20.0, 8.0, 20.0, 8.0, 5.0, blu, 0.5);
        p.glow_ring(0.0, 0.0, 20.0, 4.0, grn, 0.5);
        p.light(5.0, 5.0, wht, 1.0);
        if with_fade {
            p.fade = 0.5;
            p.rect(-24.0, -24.0, 14.0, 10.0, wht, 1.0);
            p.light_k = 0.5;
            p.glow(-18.0, 18.0, 8.0, red, 1.0);
        }
    }

    fn check(f: &Frame, golden: &[u8]) {
        let w = u32::from_le_bytes(golden[0..4].try_into().unwrap()) as usize;
        let h = u32::from_le_bytes(golden[4..8].try_into().unwrap()) as usize;
        assert_eq!((w, h), (f.w, f.h));
        let want = &golden[8..];
        let got = composite_bytes(f, Bg::Flat(hex("#201020")), 1.0);
        assert_eq!(got.len(), want.len());
        let mut bad = 0;
        for (g, x) in got.chunks(4).zip(want.chunks(4)) {
            if g.iter().zip(x).any(|(a, b)| (*a as i32 - *b as i32).abs() > 2) {
                bad += 1;
            }
        }
        let ratio = bad as f64 / (w * h) as f64;
        assert!(ratio <= 0.005, "{bad} of {} pixels differ ({ratio})", w * h);
        // the scene must actually draw something
        assert!(f.color.iter().any(|&c| c != 0) && f.light.iter().any(|&c| c != 0));
    }

    #[test]
    fn shapes_match_upstream() {
        let mut a = Frame::new(64, 48, 0, 0);
        {
            let mut p = Painter::new(&mut a);
            p.place(32.0, 24.0, false, 0.0);
            scene(&mut p, true);
        }
        check(&a, include_bytes!("../../tests/golden/painter_a.bin"));
        let mut b = Frame::new(64, 48, -10, -5);
        {
            let mut p = Painter::new(&mut b);
            p.place(22.0, 14.0, true, 0.6);
            scene(&mut p, false);
        }
        check(&b, include_bytes!("../../tests/golden/painter_b.bin"));
    }

    #[test]
    fn dither_is_ordered_and_color_zero_is_nothing() {
        let mut f = Frame::new(4, 4, 0, 0);
        let mut p = Painter::new(&mut f);
        for y in 0..4 {
            for x in 0..4 {
                p.put(x, y, 0xff00_00ff, 0.5);
            }
        }
        p.put(0, 0, 0, 1.0);
        let n = f.color.iter().filter(|&&c| c != 0).count();
        assert_eq!(n, 8); // 8 of 16 Bayer thresholds sit below 0.5
    }

    #[test]
    fn transforms_round_trip() {
        let mut f = Frame::new(8, 8, 0, 0);
        let mut p = Painter::new(&mut f);
        p.place(3.0, 4.0, true, 0.7);
        let (sx, sy) = (p.to_x(1.5, -2.0), p.to_y(1.5, -2.0));
        assert!((p.from_x(sx, sy) - 1.5).abs() < 1e-9 && (p.from_y(sx, sy) + 2.0).abs() < 1e-9);
    }

    #[test]
    fn sprite_flip_and_mask_outline() {
        let mut f = Frame::new(8, 4, 0, 0);
        let mut p = Painter::new(&mut f);
        let rows = ["#.", ".#"];
        p.sprite(&rows, &[('#', 0xff11_2233)], 0.0, 0.0, &SpriteOpts { flip: true, ..Default::default() });
        assert_eq!(p.get(1.0, 0.0), 0xff11_2233);
        assert_eq!(p.get(0.0, 0.0), 0);
        let m = Mask { w: 1, h: 1, data: vec![1] };
        p.outline(&m, 4.0, 1.0, 0xff00_00ff, 1.0);
        assert_eq!(p.get(3.0, 1.0), 0xff00_00ff);
        assert_eq!(p.get(4.0, 1.0), 0);
    }
}
