//! The character who acts: a port of pixelfx `core/rig.js`. A real skeleton in 3D (pelvis, spine, chest, shoulders, arms
//! and legs with fixed bone lengths), solved from where a pose wants the hands and feet, drawn through one depth buffer,
//! shaded into a few flat tones of the grey dummy palette and outlined with one ink line.
//!
//! Coordinates: x right, y up, z towards the viewer, in pixels, feet at the origin. Poses are in screen terms (y negative
//! is up). The output of [`posed`] is in screen terms again (y down), feet at (0, 0).
//!
//! JS details kept on purpose: the depth and tone buffers are `f32` (upstream `Float32Array`), `Math.hypot` and
//! `Math.round` follow V8, and rendered poses are cached by a rounded pose key exactly like upstream (so two poses that
//! round the same share one render; the cache is per thread).

use super::actor::DUMMY_PAL;
use super::color::hex;
use std::cell::RefCell;
use std::collections::HashMap;
use std::f64::consts::PI;
use std::rc::Rc;

// ---- JS number helpers

/// `Math.hypot` as V8 computes it: scaled by the largest value, Kahan-summed squares.
pub fn js_hypot(vals: &[f64]) -> f64 {
    let mut max = 0.0f64;
    let mut nan = false;
    for &v in vals {
        if v.is_nan() {
            nan = true;
        } else if v.abs() > max {
            max = v.abs();
        }
    }
    if max == f64::INFINITY {
        return f64::INFINITY;
    }
    if nan {
        return f64::NAN;
    }
    if max == 0.0 {
        return 0.0;
    }
    let (mut sum, mut comp) = (0.0f64, 0.0f64);
    for &v in vals {
        let n = v.abs() / max;
        let summand = n * n - comp;
        let pre = sum + summand;
        comp = (pre - sum) - summand;
        sum = pre;
    }
    sum.sqrt() * max
}
/// `Math.round`: halves go up, exact for every double.
pub fn js_round(x: f64) -> f64 {
    let c = x.ceil();
    if c - 0.5 > x {
        c - 1.0
    } else {
        c
    }
}
/// JS ToInt32 (what `x | 0` and `x >> n` do to a number).
pub fn to_i32(x: f64) -> i32 {
    if !x.is_finite() {
        return 0;
    }
    (x.trunc().rem_euclid(4294967296.0) as u64 as u32) as i32
}

// ---- vectors and turns
type V3 = [f64; 3];
type M3 = [f64; 9];
fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn mul(a: V3, s: f64) -> V3 {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: V3, b: V3) -> V3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn len(a: V3) -> f64 {
    js_hypot(&a)
}
fn unit(a: V3) -> V3 {
    let l = len(a);
    mul(a, 1.0 / if l == 0.0 || l.is_nan() { 1e-6 } else { l })
}
fn rot_y(t: f64) -> M3 {
    let (c, s) = (t.cos(), t.sin());
    [c, 0.0, s, 0.0, 1.0, 0.0, -s, 0.0, c]
}
fn rot_z(t: f64) -> M3 {
    let (c, s) = (t.cos(), t.sin());
    [c, -s, 0.0, s, c, 0.0, 0.0, 0.0, 1.0]
}
fn mm(a: &M3, b: &M3) -> M3 {
    let mut o = [0.0; 9];
    for r in 0..3 {
        for c in 0..3 {
            o[r * 3 + c] = a[r * 3] * b[c] + a[r * 3 + 1] * b[3 + c] + a[r * 3 + 2] * b[6 + c];
        }
    }
    o
}
fn mv(m: &M3, v: V3) -> V3 {
    [m[0] * v[0] + m[1] * v[1] + m[2] * v[2], m[3] * v[0] + m[4] * v[1] + m[5] * v[2], m[6] * v[0] + m[7] * v[1] + m[8] * v[2]]
}
fn tv(m: &M3, v: V3) -> V3 {
    [m[0] * v[0] + m[3] * v[1] + m[6] * v[2], m[1] * v[0] + m[4] * v[1] + m[7] * v[2], m[2] * v[0] + m[5] * v[1] + m[8] * v[2]]
}
fn clamp01(v: f64) -> f64 {
    v.min(1.0).max(0.0)
}

const DEG: f64 = PI / 180.0;
const W0: i32 = 24;
const H0: i32 = 38;

// ---- the body's measurements, in pixels
const M_PELVIS: f64 = 19.7;
const M_SPINE1: f64 = 2.8;
const M_SPINE2: f64 = 4.6;
const M_SHOULDER_UP: f64 = 2.5;
const M_SHOULDER_OUT: f64 = 5.9;
const M_NECK_UP: f64 = 4.0;
const M_HEAD_UP: f64 = 3.1;
const M_HIP_DOWN: f64 = 1.1;
const M_HIP_OUT: f64 = 2.8;
const M_THIGH: f64 = 8.6;
const M_SHIN: f64 = 8.3;
const M_UPPER: f64 = 7.6;
const M_FORE: f64 = 7.4;
fn m_turn() -> f64 {
    36.0 * DEG
}

// ---- poses

/// What the near hand (or the far hand) holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Weapon {
    Sword,
    Greatsword,
    Dagger,
    Spear,
    Hammer,
    Scythe,
    Claw,
    Bow,
    Pistol,
    Launcher,
    Handle,
}
impl Weapon {
    pub const ALL: [Weapon; 11] = [
        Weapon::Sword,
        Weapon::Greatsword,
        Weapon::Dagger,
        Weapon::Spear,
        Weapon::Hammer,
        Weapon::Scythe,
        Weapon::Claw,
        Weapon::Bow,
        Weapon::Pistol,
        Weapon::Launcher,
        Weapon::Handle,
    ];
    /// The upstream name ("sword", ...).
    pub fn name(self) -> &'static str {
        match self {
            Weapon::Sword => "sword",
            Weapon::Greatsword => "greatsword",
            Weapon::Dagger => "dagger",
            Weapon::Spear => "spear",
            Weapon::Hammer => "hammer",
            Weapon::Scythe => "scythe",
            Weapon::Claw => "claw",
            Weapon::Bow => "bow",
            Weapon::Pistol => "pistol",
            Weapon::Launcher => "launcher",
            Weapon::Handle => "handle",
        }
    }
    pub fn from_name(s: &str) -> Option<Weapon> {
        Weapon::ALL.iter().copied().find(|w| w.name() == s)
    }
    /// Where the other hand goes on a two-handed grip (`two` in upstream).
    fn two(self) -> Option<f64> {
        match self {
            Weapon::Sword => Some(-4.4),
            Weapon::Greatsword => Some(-4.8),
            Weapon::Spear => Some(-12.0),
            Weapon::Hammer => Some(-7.0),
            Weapon::Scythe => Some(-11.0),
            Weapon::Launcher => Some(-9.0),
            _ => None,
        }
    }
    /// The handle lies across the weapon's length.
    fn across(self) -> bool {
        matches!(self, Weapon::Bow | Weapon::Pistol)
    }
}

macro_rules! pose_struct {
    (nums: [$($n:ident : $js:literal),* $(,)?], flags: [$($b:ident : $jsb:literal),* $(,)?]) => {
        /// A pose: only what differs from standing easy ([`STAND`]). Every field is optional, like a JS object's keys;
        /// whether a key is present matters (merging, interpolation and the render cache all look at it).
        ///
        /// Build with the chained setters: `READY.x(-3.0).crouch(4.0).toe_b(true).weapon(Weapon::Sword)`.
        /// Field meanings (upstream poses.js): x, y feet position; crouch; lean (+ forward); hx, hy head roll/nod;
        /// fx, fy, fz near hand; bx, by, bz far hand; ffx, ffy near foot; fbx, fby far foot; fzz, bzz foot depths;
        /// turn chest turn (radians); yaw whole-body turn; twist; sa weapon angle; wl, wz weapon foreshortening;
        /// sb off-weapon angle; ghost after-images; face (-1 faces left); flags two (both hands), arms_front,
        /// toe_f, toe_b (on the toes), pull, nocked (bow).
        #[derive(Clone, Copy, Debug, Default, PartialEq)]
        pub struct Pose {
            $(pub $n: Option<f64>,)*
            $(pub $b: Option<bool>,)*
            pub weapon: Option<Weapon>,
            pub off_weapon: Option<Weapon>,
        }
        impl Pose {
            pub const EMPTY: Pose = Pose { $($n: None,)* $($b: None,)* weapon: None, off_weapon: None };
            /// Upstream (JS) names of the numeric keys, in [`Pose::nums`] order.
            pub const NUM_KEYS: &'static [&'static str] = &[$($js),*];
            /// Upstream (JS) names of the flag keys, in [`Pose::flags`] order.
            pub const FLAG_KEYS: &'static [&'static str] = &[$($jsb),*];
            $(pub const fn $n(mut self, v: f64) -> Self { self.$n = Some(v); self })*
            $(pub const fn $b(mut self, v: bool) -> Self { self.$b = Some(v); self })*
            pub const fn weapon(mut self, w: Weapon) -> Self { self.weapon = Some(w); self }
            pub const fn off_weapon(mut self, w: Weapon) -> Self { self.off_weapon = Some(w); self }
            /// `{ ...self, ...over }`.
            pub fn merge(&self, over: &Pose) -> Pose {
                let mut r = *self;
                $(if over.$n.is_some() { r.$n = over.$n; })*
                $(if over.$b.is_some() { r.$b = over.$b; })*
                if over.weapon.is_some() { r.weapon = over.weapon; }
                if over.off_weapon.is_some() { r.off_weapon = over.off_weapon; }
                r
            }
            pub fn nums(&self) -> Vec<Option<f64>> { vec![$(self.$n),*] }
            pub fn set_nums(&mut self, v: &[Option<f64>]) {
                let mut it = v.iter();
                $(self.$n = *it.next().unwrap();)*
            }
            pub fn set_num(&mut self, i: usize, v: Option<f64>) {
                let mut n = self.nums();
                n[i] = v;
                self.set_nums(&n);
            }
            pub fn flags(&self) -> Vec<Option<bool>> { vec![$(self.$b),*] }
            pub fn set_flag(&mut self, i: usize, v: Option<bool>) {
                let mut k = 0;
                $(if k == i { self.$b = v; } k += 1;)*
                let _ = k;
            }
        }
    };
}
pose_struct!(
    nums: [x: "x", y: "y", lean: "lean", crouch: "crouch", yaw: "yaw", fx: "fx", fy: "fy", fz: "fz", bx: "bx", by: "by",
           bz: "bz", ffx: "ffx", ffy: "ffy", fbx: "fbx", fby: "fby", fzz: "fzz", bzz: "bzz", turn: "turn", twist: "twist",
           hx: "hx", hy: "hy", sa: "sa", sb: "sb", wl: "wl", wz: "wz", ghost: "ghost", face: "face"],
    flags: [two: "two", arms_front: "armsFront", toe_f: "toeF", toe_b: "toeB", pull: "pull", nocked: "nocked"]
);

/// Standing easy, in screen terms: hands and feet are where they should end up, y negative is up.
pub const STAND: Pose = Pose::EMPTY
    .lean(0.0)
    .crouch(0.0)
    .yaw(0.0)
    .fx(-3.0)
    .fy(-15.0)
    .bx(5.0)
    .by(-15.0)
    .ffx(-2.0)
    .ffy(-4.0)
    .fbx(3.0)
    .fby(-4.0);

/// A merged pose's number (every STAND key is set, the rest may not be).
fn n(v: Option<f64>) -> f64 {
    v.unwrap_or(f64::NAN)
}

// ---- solving a pose: from where the hands and feet should be, to every joint
#[derive(Clone, Copy)]
struct Ik {
    joint: V3,
    end: V3,
    v: V3,
}
fn ik(a: V3, target: V3, l1: f64, l2: f64, pole: V3) -> Ik {
    let mut to = sub(target, a);
    let mut d = len(to);
    let reach = l1 + l2 - 0.02;
    if d > reach {
        to = mul(to, reach / d);
        d = reach;
    }
    let u = unit(to);
    let along = (l1 * l1 - l2 * l2 + d * d) / (2.0 * d);
    let h = (0.0f64).max(l1 * l1 - along * along).sqrt();
    let mut v = sub(pole, mul(u, dot(pole, u)));
    v = if len(v) < 1e-4 { unit(cross(u, [0.0, 0.0, 1.0])) } else { unit(v) };
    Ik { joint: add(a, add(mul(u, along), mul(v, h))), end: add(a, to), v }
}
#[derive(Clone, Copy)]
struct BFrame {
    t: V3,
    r: M3,
}
fn bone_frame(a: V3, b: V3, pole: V3) -> BFrame {
    let x = unit(sub(b, a));
    let mut y = sub(pole, mul(x, dot(pole, x)));
    y = if len(y) < 1e-4 { unit(cross(x, [0.0, 0.0, 1.0])) } else { unit(y) };
    let z = cross(x, y);
    BFrame { t: a, r: [x[0], y[0], z[0], x[1], y[1], z[1], x[2], y[2], z[2]] }
}

// frame ids, in upstream FRAME_IDS order
const F_PELVIS: usize = 0;
const F_WAIST: usize = 1;
const F_CHEST: usize = 2;
const F_HEAD: usize = 3;
const F_NECK: usize = 4;
const F_UPPER_N: usize = 5;
const F_FORE_N: usize = 6;
const F_UPPER_F: usize = 7;
const F_FORE_F: usize = 8;
const F_THIGH_N: usize = 9;
const F_SHIN_N: usize = 10;
const F_THIGH_F: usize = 11;
const F_SHIN_F: usize = 12;
const F_FOOT_N: usize = 13;
const F_FOOT_F: usize = 14;

struct Solved {
    b: [BFrame; 15],
    w: Option<Weapon>,
    ow: Option<Weapon>,
    q: Pose,
    axis: V3,
    side: V3,
    arm_n: Ik,
    arm_f: Ik,
    leg_n: Ik,
    leg_f: Ik,
    s_n: V3,
    s_f: V3,
    hip_n: V3,
    hip_f: V3,
}

fn solve(pose: &Pose) -> Solved {
    let q = STAND.merge(pose);
    let lean = n(q.lean);
    let w = q.weapon;
    let ow = q.off_weapon;
    let sa = q.sa.unwrap_or(0.0);
    let wl = q.wl.unwrap_or(1.0);
    let axis_z = (0.0f64).max(1.0 - wl * wl).sqrt() * q.wz.unwrap_or(1.0);
    let axis = [sa.cos() * wl, -sa.sin() * wl, axis_z];
    let side = [-sa.sin(), -sa.cos(), 0.0];
    let two = q.two == Some(true) && w.is_some();
    let near_z = q.fz.unwrap_or(if two { 3.0 } else { 4.5 });
    // how the chest is turned follows the hands
    let (qfx, qfy, qbx, qby) = (n(q.fx), n(q.fy), n(q.bx), n(q.by));
    let fwd = (qfx - n(STAND.fx)) / 24.0;
    let far_fwd = if two { 0.0 } else { (qbx - n(STAND.bx)) / 22.0 };
    let mut turn = m_turn() + if fwd >= 0.0 { -44.0 * DEG * (1.0f64).min(fwd) } else { 28.0 * DEG * (1.0f64).min(-fwd * 1.8) };
    turn += (if far_fwd > 0.0 { 34.0 * DEG * (1.0f64).min(far_fwd) } else { 0.0 }) + q.twist.unwrap_or(0.0) * 4.0 * DEG;
    if let Some(t) = q.turn {
        turn = t;
    }
    let hip_turn = m_turn() + (turn - m_turn()) * 0.45;
    let tilt = lean.atan2(13.0);
    let rp = mm(&rot_z(-tilt * 0.25), &rot_y(-hip_turn));
    let rc = mm(&rot_z(-tilt), &rot_y(-turn));
    // feet first: the pelvis sinks if that's what it takes for both legs to reach them
    let ankle_n = [n(q.ffx), -n(q.ffy) - 1.8, q.fzz.unwrap_or(2.4)];
    let ankle_f = [n(q.fbx), -n(q.fby) - 1.8, q.bzz.unwrap_or(-2.4)];
    let mut pelvis = [lean * 0.2, M_PELVIS - n(q.crouch), 0.0];
    let hip_at = |pelvis: V3, s: f64| add(pelvis, mv(&rp, [0.0, -M_HIP_DOWN, s * M_HIP_OUT]));
    let max_leg = (M_THIGH + M_SHIN) * 0.995;
    for (s, a) in [(1.0, ankle_n), (-1.0, ankle_f)] {
        let h = hip_at(pelvis, s);
        let (dx, dz) = (h[0] - a[0], h[2] - a[2]);
        let flat = dx * dx + dz * dz;
        if flat < max_leg * max_leg {
            let top = a[1] + (max_leg * max_leg - flat).sqrt();
            if h[1] > top {
                pelvis = [pelvis[0], pelvis[1] - (h[1] - top), pelvis[2]];
            }
        }
    }
    let waist = add(pelvis, [(tilt * 0.45).sin() * M_SPINE1, (tilt * 0.45).cos() * M_SPINE1, 0.0]);
    let chest = add(waist, [tilt.sin() * M_SPINE2, tilt.cos() * M_SPINE2, 0.0]);
    let neck = add(chest, mv(&rc, [0.0, M_NECK_UP, 0.0]));
    // the head keeps looking where he's going
    let head_turn = m_turn() * 0.9 + (turn - m_turn()) * 0.35;
    let nod = tilt * 0.35 + q.hy.unwrap_or(0.0) * 7.0 * DEG;
    let roll = q.hx.unwrap_or(0.0) * 7.0 * DEG;
    let rh = mm(&rot_z(-nod - roll), &rot_y(-head_turn));
    let head = add(neck, mv(&rh, [0.15, M_HEAD_UP, 0.0]));
    // the arms: shoulders reach a little towards a far hand
    let pick = |s: f64, target: V3| {
        let base = add(chest, mv(&rc, [0.0, M_SHOULDER_UP, s * M_SHOULDER_OUT]));
        let to = sub(target, base);
        let d = len(to);
        add(base, mul(unit(to), (1.8f64).min((0.0f64).max(d - 11.0) * 0.35)))
    };
    let near_t = [qfx, -qfy, near_z];
    let far_t = match (two, w.and_then(|w| w.two())) {
        (true, Some(wt)) => [qfx + axis[0] * wt, -qfy + axis[1] * wt, near_z + axis[2] * wt],
        // a two-handed grip on a weapon without a `two` point: NaN, as upstream (undefined * n)
        (true, None) => [f64::NAN; 3],
        _ => [qbx, -qby, q.bz.unwrap_or(if q.arms_front == Some(true) { 5.5 } else { -4.5 })],
    };
    let s_n = pick(1.0, near_t);
    let s_f = pick(-1.0, far_t);
    let pole = |s: f64, t: V3, shoulder: V3, hold: Option<V3>| -> V3 {
        // elbows hang down and back; overhead they go out and forward
        let up = clamp01((t[1] - shoulder[1] + 2.0) / 10.0);
        let base = [-0.55 + up * 1.1, -1.0 + up * 0.9, s * (0.45 + up * 0.5)];
        let Some(hold) = hold else { return base };
        // holding something, the forearm wants to meet the handle side on: try round the circle, keep the best
        let u = unit(sub(t, shoulder));
        let (mut best, mut score) = (base, f64::INFINITY);
        for i in 0..16 {
            let a = (i as f64 / 16.0) * PI * 2.0;
            let x = unit(cross(u, [0.3, 1.0, 0.2]));
            let y = cross(u, x);
            let v = add(mul(x, a.cos()), mul(y, a.sin()));
            let aa = ik(shoulder, t, M_UPPER, M_FORE, v);
            let f = unit(sub(aa.end, aa.joint));
            let cost = dot(f, hold).abs() * 0.7 + (1.0 - dot(unit(base), v)) * 0.55;
            if cost < score {
                score = cost;
                best = v;
            }
        }
        best
    };
    let grip = |w: Weapon| if w.across() { side } else { axis };
    let hold_n = w.map(grip);
    let hold_f = if two {
        w.map(grip)
    } else if ow.is_some() {
        let sb = q.sb.unwrap_or(PI);
        Some([sb.cos(), -sb.sin(), 0.0])
    } else {
        None
    };
    let arm_n = ik(s_n, near_t, M_UPPER, M_FORE, pole(1.0, near_t, s_n, hold_n));
    let arm_f = ik(s_f, far_t, M_UPPER, M_FORE, pole(-1.0, far_t, s_f, hold_f));
    // the legs: knees forward, a little out
    let knee_pole = |s: f64| mv(&rp, [1.0, 0.0, s * 0.35]);
    let hip_n = hip_at(pelvis, 1.0);
    let hip_f = hip_at(pelvis, -1.0);
    let leg_n = ik(hip_n, ankle_n, M_THIGH, M_SHIN, knee_pole(1.0));
    let leg_f = ik(hip_f, ankle_f, M_THIGH, M_SHIN, knee_pole(-1.0));
    let foot_dir = |s: f64| {
        let t = hip_turn * 0.6 + s * 8.0 * DEG;
        [t.cos(), 0.0, t.sin()]
    };
    let fr = |t: V3, r: M3| BFrame { t, r };
    let foot = |end: V3, s: f64, toe: Option<bool>| {
        let d = foot_dir(s);
        fr(end, mm(&rot_y(-d[2].atan2(d[0])), &rot_z(if toe == Some(true) { -55.0 * DEG } else { 0.0 })))
    };
    let b = [
        fr(pelvis, rp),
        fr(waist, mm(&rot_z(-tilt * 0.6), &rot_y(-(turn + hip_turn) / 2.0))),
        fr(chest, rc),
        fr(head, rh),
        fr(neck, rh),
        bone_frame(s_n, arm_n.joint, arm_n.v),
        bone_frame(arm_n.joint, arm_n.end, arm_n.v),
        bone_frame(s_f, arm_f.joint, arm_f.v),
        bone_frame(arm_f.joint, arm_f.end, arm_f.v),
        bone_frame(hip_n, leg_n.joint, leg_n.v),
        bone_frame(leg_n.joint, leg_n.end, leg_n.v),
        bone_frame(hip_f, leg_f.joint, leg_f.v),
        bone_frame(leg_f.joint, leg_f.end, leg_f.v),
        foot(leg_n.end, 1.0, q.toe_f),
        foot(leg_f.end, -1.0, q.toe_b),
    ];
    Solved { b, w, ow, q, axis, side, arm_n, arm_f, leg_n, leg_f, s_n, s_f, hip_n, hip_f }
}

/// The whole figure turned about the vertical through the feet.
fn turn_all(s: &mut Solved, a: f64) {
    let r = rot_y(-a);
    let p = |v: V3| mv(&r, v);
    for f in s.b.iter_mut() {
        f.t = p(f.t);
        f.r = mm(&r, &f.r);
    }
    for k in [&mut s.arm_n, &mut s.arm_f, &mut s.leg_n, &mut s.leg_f] {
        k.joint = p(k.joint);
        k.end = p(k.end);
    }
    s.s_n = p(s.s_n);
    s.s_f = p(s.s_f);
    s.hip_n = p(s.hip_n);
    s.hip_f = p(s.hip_f);
    s.axis = p(s.axis);
    s.side = p(s.side);
}
fn whole_turn(q: &Pose) -> f64 {
    q.yaw.unwrap_or(0.0) + if q.face == Some(-1.0) { PI } else { 0.0 }
}
/// JS truthiness of a number.
fn truthy(v: f64) -> bool {
    v != 0.0 && !v.is_nan()
}

// ---- drawing: every part goes through one depth buffer
const BW: i64 = 300;
const BH: i64 = 240;
const OX: i64 = 150;
const OY: i64 = 180;
const NB: usize = (BW * BH) as usize;

// parts: each has its own number, so edges between them can be seen
const PARTS: [(&str, i8); 24] = [
    ("pelvis", 1),
    ("waist", 2),
    ("chest", 3),
    ("neck", 4),
    ("head", 5),
    ("face", 6),
    ("upperN", 7),
    ("elbowN", 8),
    ("foreN", 9),
    ("handN", 10),
    ("upperF", 11),
    ("elbowF", 12),
    ("foreF", 13),
    ("handF", 14),
    ("thighN", 15),
    ("kneeN", 16),
    ("shinN", 17),
    ("footN", 18),
    ("thighF", 19),
    ("kneeF", 20),
    ("shinF", 21),
    ("footF", 22),
    ("weapon", 23),
    ("offWeapon", 24),
];
const P_CHEST: i8 = 3;
const P_UPPER_F: i8 = 11;
const P_HAND_F: i8 = 14;
const P_THIGH_F: i8 = 19;
const P_FOOT_F: i8 = 22;
const P_WEAPON: i8 = 23;
const P_OFF_WEAPON: i8 = 24;

struct Grid {
    zb: Vec<f32>,
    tone: Vec<f32>,
    part: Vec<i8>,
    mat: Vec<i8>,
    fr: Vec<i8>,
    col: Vec<u32>,
    tones: Vec<i8>,
    marks: Vec<u32>,
    ring: Vec<i64>,
    touched: Vec<usize>,
    clip_body: bool,
    clip_weapon: bool,
    cur_frame: i8,
    only_part: i8,
    light: V3,
}
impl Grid {
    fn new() -> Grid {
        Grid {
            zb: vec![0.0; NB],
            tone: vec![0.0; NB],
            part: vec![0; NB],
            mat: vec![0; NB],
            fr: vec![0; NB],
            col: vec![0; NB],
            tones: vec![-1; NB],
            marks: vec![0; NB],
            ring: vec![-1; NB],
            touched: Vec::new(),
            clip_body: false,
            clip_weapon: false,
            cur_frame: 0,
            only_part: 0,
            light: unit([-0.55, 0.72, 0.5]),
        }
    }
    /// The part at a grid index; None outside the grid (upstream reads `undefined` there).
    fn part_at(&self, j: i64) -> Option<i8> {
        if j < 0 || j >= NB as i64 {
            None
        } else {
            Some(self.part[j as usize])
        }
    }
    fn plot(&mut self, i: i64, z: f64, tone: f64, part: i8, mat: i8, color: u32) {
        if self.only_part != 0 && part != self.only_part {
            return;
        }
        if i < 0 || i >= NB as i64 {
            return;
        }
        let i = i as usize;
        if self.part[i] == 0 {
            self.touched.push(i);
        } else if z <= self.zb[i] as f64 {
            return;
        }
        self.zb[i] = z as f32;
        self.tone[i] = tone as f32;
        self.part[i] = part;
        self.mat[i] = mat;
        self.fr[i] = self.cur_frame;
        self.col[i] = color;
    }
    fn sphere(&mut self, c: V3, r: f64, part: i8, mat: i8) {
        let (x0, x1, y0, y1) = ((c[0] - r).floor() as i64, (c[0] + r).ceil() as i64, (c[1] - r).floor() as i64, (c[1] + r).ceil() as i64);
        for wy in y0..=y1 {
            for wx in x0..=x1 {
                let (dx, dy) = (wx as f64 - c[0], wy as f64 - c[1]);
                let d2 = dx * dx + dy * dy;
                if d2 > r * r || (self.clip_body && (wy as f64) < 1.5) {
                    continue;
                }
                let dz = (r * r - d2).sqrt();
                let nn = [dx / r, dy / r, dz / r];
                let i = (OY - wy) * BW + (OX + wx);
                let l = self.light;
                self.plot(i, c[2] + dz, dot(nn, l), part, mat, 0);
            }
        }
    }
    fn sweep(&mut self, a: V3, b: V3, ra: f64, rb: f64, part: i8, mat: i8, bulge: f64) {
        let n = (2.0f64).max((len(sub(b, a)) / 0.4).ceil()) as i64;
        for k in 0..=n {
            let u = k as f64 / n as f64;
            self.sphere(add(a, mul(sub(b, a), u)), ra + (rb - ra) * u + bulge * (u * PI).sin(), part, mat);
        }
    }
    /// An ellipsoid with radii r along the axes of frame R, seen along z.
    fn ellipsoid(&mut self, c: V3, rr: &M3, r: V3, part: i8, mat: i8) {
        let big = r[0].max(r[1]).max(r[2]);
        let (x0, x1, y0, y1) = ((c[0] - big).floor() as i64, (c[0] + big).ceil() as i64, (c[1] - big).floor() as i64, (c[1] + big).ceil() as i64);
        let d = [1.0 / (r[0] * r[0]), 1.0 / (r[1] * r[1]), 1.0 / (r[2] * r[2])];
        let mut a = [0.0; 9];
        for i in 0..3 {
            for j in 0..3 {
                a[i * 3 + j] = rr[i * 3] * d[0] * rr[j * 3] + rr[i * 3 + 1] * d[1] * rr[j * 3 + 1] + rr[i * 3 + 2] * d[2] * rr[j * 3 + 2];
            }
        }
        for wy in y0..=y1 {
            for wx in x0..=x1 {
                if self.clip_body && (wy as f64) < 1.5 {
                    continue;
                }
                let (dx, dy) = (wx as f64 - c[0], wy as f64 - c[1]);
                let qa = a[8];
                let qb = 2.0 * (a[2] * dx + a[5] * dy);
                let qc = a[0] * dx * dx + 2.0 * a[1] * dx * dy + a[4] * dy * dy - 1.0;
                let disc = qb * qb - 4.0 * qa * qc;
                if disc < 0.0 {
                    continue;
                }
                let dz = (-qb + disc.sqrt()) / (2.0 * qa);
                let nn = unit([a[0] * dx + a[1] * dy + a[2] * dz, a[3] * dx + a[4] * dy + a[5] * dz, a[6] * dx + a[7] * dy + a[8] * dz]);
                let l = self.light;
                self.plot((OY - wy) * BW + (OX + wx), c[2] + dz, dot(nn, l), part, mat, 0);
            }
        }
    }
}

fn draw_body(g: &mut Grid, s: &Solved) {
    let b = &s.b;
    g.cur_frame = F_PELVIS as i8;
    g.ellipsoid(add(b[F_PELVIS].t, mv(&b[F_PELVIS].r, [0.0, -0.2, 0.0])), &b[F_PELVIS].r, [2.9, 2.6, 4.2], 1, 0);
    g.cur_frame = F_WAIST as i8;
    g.sweep(b[F_PELVIS].t, b[F_CHEST].t, 3.0, 3.3, 2, 1, 0.0);
    g.cur_frame = F_CHEST as i8;
    g.ellipsoid(add(b[F_CHEST].t, mv(&b[F_CHEST].r, [0.2, 0.9, 0.0])), &b[F_CHEST].r, [3.4, 4.2, 5.5], 3, 0);
    g.cur_frame = F_NECK as i8;
    g.sweep(b[F_NECK].t, add(b[F_NECK].t, mv(&b[F_HEAD].r, [0.0, 1.6, 0.0])), 1.25, 1.15, 4, 1, 0.0);
    g.cur_frame = F_HEAD as i8;
    g.ellipsoid(b[F_HEAD].t, &b[F_HEAD].r, [3.0, 3.3, 2.8], 5, 0);
    g.ellipsoid(add(b[F_HEAD].t, mv(&b[F_HEAD].r, [0.85, -0.45, 0.0])), &b[F_HEAD].r, [2.4, 2.7, 2.35], 6, 0);
    let arm = |g: &mut Grid, a: &Ik, sh: V3, parts: [i8; 4], fu: usize, ff: usize| {
        g.cur_frame = fu as i8;
        g.sphere(sh, 2.5, parts[0], 0);
        g.sweep(sh, a.joint, 2.1, 1.6, parts[0], 0, 0.0);
        g.sphere(a.joint, 1.45, parts[1], 1);
        g.cur_frame = ff as i8;
        g.sweep(a.joint, a.end, 1.6, 1.2, parts[2], 0, 0.15);
        g.sphere(a.end, 1.9, parts[3], 0);
    };
    arm(g, &s.arm_n, s.s_n, [7, 8, 9, 10], F_UPPER_N, F_FORE_N);
    arm(g, &s.arm_f, s.s_f, [11, 12, 13, 14], F_UPPER_F, F_FORE_F);
    let leg = |g: &mut Grid, l: &Ik, hip: V3, parts: [i8; 4], ft: usize, fs: usize, ff: usize| {
        g.cur_frame = ft as i8;
        g.sweep(hip, l.joint, 3.2, 2.2, parts[0], 0, 0.0);
        g.sphere(l.joint, 1.9, parts[1], 1);
        g.cur_frame = fs as i8;
        g.sweep(l.joint, l.end, 2.15, 1.4, parts[2], 0, 0.4);
        g.cur_frame = ff as i8;
        let (r, a) = (b[ff].r, b[ff].t);
        g.sweep(add(a, mv(&r, [-1.5, -0.6, 0.0])), add(a, mv(&r, [4.6, -1.1, 0.0])), 1.7, 1.25, parts[3], 0, 0.0);
    };
    leg(g, &s.leg_n, s.hip_n, [15, 16, 17, 18], F_THIGH_N, F_SHIN_N, F_FOOT_N);
    leg(g, &s.leg_f, s.hip_f, [19, 20, 21, 22], F_THIGH_F, F_SHIN_F, F_FOOT_F);
}

// ---- what they hold: drawn along the handle from the fist, through the same depth buffer
struct Along {
    h: V3,
    axis: V3,
    side: V3,
    n: V3,
    part: i8,
}
impl Along {
    fn new(h: V3, axis: V3, side: V3, part: i8) -> Along {
        Along { h, axis, side, n: cross(axis, side), part }
    }
    fn t(&self, g: &mut Grid, i: f64, k: f64, c: u32) {
        self.tl(g, i, k, c, 0.0)
    }
    fn tl(&self, g: &mut Grid, i: f64, k: f64, c: u32, lift: f64) {
        let (h, a, s, n) = (self.h, self.axis, self.side, self.n);
        let x = h[0] + a[0] * i + s[0] * k + n[0] * lift;
        let y = h[1] + a[1] * i + s[1] * k + n[1] * lift;
        let z = h[2] + a[2] * i + s[2] * k + n[2] * lift;
        let (wx, wy) = (js_round(x), js_round(y));
        if g.clip_weapon && wy < 0.0 {
            return;
        }
        if !wx.is_finite() || !wy.is_finite() {
            return;
        }
        let idx = (OY - wy as i64) * BW + (OX + wx as i64);
        if idx < 0 || idx >= NB as i64 {
            return;
        }
        g.cur_frame = -1;
        g.plot(idx, z + 0.05, 2.0, self.part, 2, c);
    }
    fn at(&self, i: f64, k: f64) -> V3 {
        let (h, a, s) = (self.h, self.axis, self.side);
        [h[0] + a[0] * i + s[0] * k, h[1] + a[1] * i + s[1] * k, h[2] + a[2] * i + s[2] * k]
    }
}

struct Steel {
    hi: u32,
    a: u32,
    b: u32,
    d: u32,
    gold: u32,
    gold_d: u32,
    grip: u32,
    grip_d: u32,
    wood: u32,
    wood_d: u32,
    dark: u32,
    dark_d: u32,
    olive: u32,
    olive_a: u32,
    olive_d: u32,
}
fn steel() -> Steel {
    Steel {
        hi: hex("#ffffff"),
        a: hex("#e8edf6"),
        b: hex("#a4afc6"),
        d: hex("#68748f"),
        gold: hex("#ffd84a"),
        gold_d: hex("#b8862a"),
        grip: hex("#7a3a2a"),
        grip_d: hex("#4a2018"),
        wood: hex("#8a5a32"),
        wood_d: hex("#5a3820"),
        dark: hex("#3a3448"),
        dark_d: hex("#262033"),
        olive: hex("#58663f"),
        olive_a: hex("#7d8c58"),
        olive_d: hex("#3a4529"),
    }
}
/// `for (let v = from; v <= to; v += step)`, with the same float accumulation.
fn steps(from: f64, to: f64, step: f64) -> impl Iterator<Item = f64> {
    let mut v = from;
    std::iter::from_fn(move || {
        if v <= to {
            let r = v;
            v += step;
            Some(r)
        } else {
            None
        }
    })
}
fn blade(g: &mut Grid, t: &Along, st: &Steel, from: f64, length: f64, w: f64) {
    for i in steps(from, from + length + 0.01, 0.4) {
        let r = i - from;
        let wd = if r > length - 5.0 { (0.0f64).max(((length - r) / 5.0) * w) } else { w };
        for k in steps(-wd, wd + 0.01, 0.4) {
            t.t(g, i, k, if k < -w * 0.4 { st.hi } else if k > w * 0.4 { st.d } else { st.a });
        }
    }
}
fn haft(g: &mut Grid, t: &Along, from: f64, to: f64, a: u32, d: u32, r: f64) {
    for i in steps(from, to, 0.4) {
        for k in steps(-r, r + 0.01, 0.4) {
            t.t(g, i, k, if k > 0.1 { d } else { a });
        }
    }
}
fn draw_weapon(w: Weapon, g: &mut Grid, t: &Along, q: &Pose, off: V3) {
    let st = steel();
    match w {
        Weapon::Sword => {
            let end = if q.two == Some(true) { -6.5 } else { -2.6 };
            haft(g, t, end, 2.2, st.grip, st.grip_d, 0.8);
            for k in steps(-1.4, 1.4, 0.4) {
                t.t(g, end - 0.4, k, st.gold);
            }
            for k in steps(-4.6, 4.6, 0.4) {
                t.t(g, 2.6, k, if k.abs() > 3.8 { st.gold_d } else { st.gold });
                t.t(g, 3.0, k, st.gold_d);
            }
            blade(g, t, &st, 3.2, 25.0, 2.2);
        }
        Weapon::Greatsword => {
            haft(g, t, -7.2, 2.2, st.grip, st.grip_d, 0.8);
            for k in steps(-1.8, 1.8, 0.4) {
                t.t(g, -7.6, k, st.gold);
            }
            for k in steps(-6.6, 6.6, 0.4) {
                t.t(g, 2.6, k, if k.abs() > 5.6 { st.gold_d } else { st.gold });
                t.t(g, 3.0, k, st.gold_d);
            }
            blade(g, t, &st, 3.2, 31.0, 3.0);
            for i in steps(4.0, 28.0, 0.4) {
                t.t(g, i, 0.0, st.b);
            }
        }
        Weapon::Dagger => {
            haft(g, t, -2.4, 1.8, st.grip, st.grip_d, 0.8);
            for k in steps(-2.6, 2.6, 0.4) {
                t.t(g, 2.2, k, st.gold_d);
            }
            blade(g, t, &st, 2.6, 10.0, 1.3);
        }
        Weapon::Spear => {
            haft(g, t, -18.0, 36.0, st.wood, st.wood_d, 0.5);
            blade(g, t, &st, 36.0, 11.0, 2.0);
            for k in steps(-2.4, 2.4, 0.4) {
                t.t(g, 36.0, k, st.gold_d);
            }
        }
        Weapon::Hammer => {
            for lift in [-0.8, 0.0, 0.8] {
                for i in steps(-10.0, 18.0, 0.4) {
                    for k in steps(-0.8, 0.81, 0.4) {
                        t.tl(g, i, k, if k > 0.1 { st.wood_d } else { st.wood }, lift);
                    }
                }
            }
            // the head is a block: its two faces, and the steel between them
            for i in steps(18.0, 28.0, 0.4) {
                for k in steps(-9.0, 9.0, 0.4) {
                    let face = if k.abs() > 7.4 {
                        if k < 0.0 {
                            st.a
                        } else {
                            st.d
                        }
                    } else if i < 20.0 {
                        st.hi
                    } else if i > 26.4 {
                        st.d
                    } else if k.abs() < 2.0 {
                        st.gold
                    } else {
                        st.b
                    };
                    t.tl(g, i, k, face, 4.5);
                    t.tl(g, i, k, face, -4.5);
                    if i < 18.4 || i > 27.6 || k.abs() > 8.6 {
                        for l in steps(-4.0, 4.0, 0.4) {
                            t.tl(g, i, k, if k < 0.0 { st.a } else if i > 27.6 { st.d } else { st.b }, l);
                        }
                    }
                }
            }
        }
        Weapon::Scythe => {
            haft(g, t, -16.0, 31.0, st.dark, st.dark_d, 0.5);
            for j in steps(0.0, 28.0, 0.4) {
                let u = j / 28.0;
                let w = 3.2 * (1.0 - u) + 0.3;
                let b = 31.5 - u * u * 12.0;
                for k in steps(-w, w, 0.4) {
                    t.t(g, b + k, j + 0.8, if k > w * 0.3 { st.hi } else if k < -w * 0.4 { st.d } else { st.b });
                }
            }
        }
        Weapon::Claw => {
            for q0 in [-2.4, 0.0, 2.4] {
                for i in steps(2.0, 9.5, 0.4) {
                    let u = (i - 2.0) / 7.5;
                    let k = q0 + u * u * 1.8;
                    t.t(g, i, k, if u > 0.55 { st.hi } else { st.a });
                    if u < 0.6 {
                        t.t(g, i, k + 0.8, st.d);
                    }
                }
            }
        }
        Weapon::Bow => {
            for u in steps(-1.0, 1.001, 0.025) {
                let k = u * 17.0;
                let i = 6.0 * (1.0 - u * u) - 1.0;
                t.t(g, i, k, if u.abs() > 0.85 { st.gold_d } else { st.wood });
                t.t(g, i + 0.8, k, st.wood_d);
            }
            let tip_a = t.at(-1.0, -17.0);
            let tip_b = t.at(-1.0, 17.0);
            let mid = if q.pull == Some(true) { off } else { t.at(-1.0, 0.0) };
            let line = |g: &mut Grid, a: V3, b: V3, c: u32| {
                let n = (2.0f64).max((len(sub(b, a)) / 0.4).ceil()) as i64;
                for k in 0..=n {
                    let p = add(a, mul(sub(b, a), k as f64 / n as f64));
                    let (wx, wy) = (js_round(p[0]), js_round(p[1]));
                    if !wx.is_finite() || !wy.is_finite() {
                        continue;
                    }
                    g.cur_frame = -1;
                    g.plot((OY - wy as i64) * BW + (OX + wx as i64), p[2] + 0.05, 2.0, P_WEAPON, 2, c);
                }
            };
            line(g, tip_a, mid, st.a);
            line(g, tip_b, mid, st.a);
            if q.nocked == Some(true) {
                let tip = t.at(13.0, 0.0);
                line(g, mid, tip, st.wood);
                for i in steps(9.0, 13.0, 0.4) {
                    let h = (13.0 - i) * 0.45;
                    for k in steps(-h, h, 0.4) {
                        t.t(g, i, k, st.hi);
                    }
                }
            }
        }
        Weapon::Pistol => {
            for i in steps(-1.0, 9.5, 0.4) {
                for k in steps(-4.2, -2.4, 0.4) {
                    t.t(g, i, k, if k < -3.6 { st.a } else if k > -2.9 { st.d } else { st.b });
                }
            }
            for i in steps(-1.2, 1.2, 0.4) {
                for k in steps(-2.4, 2.4, 0.4) {
                    t.t(g, i, k, st.grip_d);
                }
            }
            t.t(g, 9.0, -4.6, st.hi);
        }
        Weapon::Launcher => {
            for i in steps(-14.0, 22.0, 0.4) {
                for k in steps(-6.0, -1.0, 0.4) {
                    let c = if k < -4.8 {
                        st.olive_a
                    } else if k > -2.2 {
                        st.olive_d
                    } else if (i - 3.0).abs() < 1.4 {
                        st.gold
                    } else {
                        st.olive
                    };
                    t.t(g, i, k, c);
                }
            }
            for k in steps(-6.8, -0.2, 0.4) {
                t.t(g, 22.4, k, st.b);
                t.t(g, -14.4, k, st.d);
            }
        }
        Weapon::Handle => {
            haft(g, t, -2.6, 6.0, st.grip, st.grip_d, 0.8);
            for k in steps(-1.6, 1.6, 0.4) {
                t.t(g, -3.0, k, st.gold);
                t.t(g, 6.4, k, st.gold);
            }
        }
    }
}

/// The pixels of a pose: parallel arrays, in local space with the feet at (0, 0), y down.
/// `src[i]` is the pixel (row * 24 + col) of the standing 24 x 38 character this pixel came from, or -1.
#[derive(Clone, Debug, Default)]
pub struct Posed {
    pub n: usize,
    pub xs: Vec<i16>,
    pub ys: Vec<i16>,
    pub color: Vec<u32>,
    pub src: Vec<i16>,
    pub ink: Vec<u8>,
    pub x0: i32,
    pub y0: i32,
    pub x1: i32,
    pub y1: i32,
    /// Mean depth of the body pixels.
    pub z: f64,
}

struct Pal {
    ink: u32,
    seam: u32,
    mark: u32,
    shell: [u32; 4],
    core: [u32; 4],
}
fn pal() -> Pal {
    let p = |ch: char| DUMMY_PAL.iter().find(|(c, _)| *c == ch).map_or(0, |(_, h)| hex(h));
    Pal { ink: p('k'), seam: p('J'), mark: p('m'), shell: [p('A'), p('B'), p('C'), p('D')], core: [p('E'), p('F'), p('G'), p('G')] }
}

// ---- flat tones, seams, and one clean ink line
fn finish(g: &mut Grid, s: &Solved, rest: &[BFrame; 15]) -> Posed {
    let pl = pal();
    let dim = |part: i8| -> i8 { i8::from((P_UPPER_F..=P_HAND_F).contains(&part) || (P_THIGH_F..=P_FOOT_F).contains(&part)) };
    let tone_of = |v: f64, part: i8| -> i8 {
        let t: i8 = if v > 0.8 {
            0
        } else if v > 0.42 {
            1
        } else if v > 0.08 {
            2
        } else {
            3
        };
        (t + dim(part)).min(3)
    };
    let touched = std::mem::take(&mut g.touched);
    for &i in &touched {
        g.tones[i] = tone_of(g.tone[i] as f64, g.part[i]);
    }
    let dirs = [-1i64, 1, -BW, BW];
    // a lone pixel of a tone, with no neighbour like it on the same part, takes its neighbours' tone
    for &i in &touched {
        if g.mat[i] == 2 {
            continue;
        }
        let t = g.tones[i];
        let mut votes = [0i32; 4];
        let mut same = false;
        for d in dirs {
            let j = i as i64 + d;
            if g.part_at(j) != Some(g.part[i]) || g.tones[j as usize] < 0 {
                continue;
            }
            let u = g.tones[j as usize];
            votes[u as usize] += 1;
            if u == t {
                same = true;
            }
        }
        if !same && votes.iter().any(|&v| v > 0) {
            let mx = *votes.iter().max().unwrap();
            g.tones[i] = votes.iter().position(|&v| v == mx).unwrap() as i8;
        }
    }
    for &i in &touched {
        if g.mat[i] == 2 {
            continue;
        }
        let set = if g.mat[i] == 1 { pl.core } else { pl.shell };
        g.col[i] = set[g.tones[i] as usize];
    }
    // where two parts meet: a soft seam if they touch, ink if one stands in front of the other
    let mut marked: Vec<usize> = Vec::new();
    for &i in &touched {
        for d in [1i64, BW] {
            let j = i as i64 + d;
            let Some(pj) = g.part_at(j) else { continue };
            if pj == 0 || pj == g.part[i] {
                continue;
            }
            let j = j as usize;
            let far = if (g.zb[i] as f64) < (g.zb[j] as f64) { i } else { j };
            let dz = (g.zb[i] as f64 - g.zb[j] as f64).abs();
            let steel = g.mat[i] == 2 || g.mat[j] == 2;
            if g.mat[far] == 2 && dz < 1.0 {
                continue;
            }
            let ink = steel || dz > 2.2;
            if g.marks[far] == 0 {
                marked.push(far);
            }
            if ink {
                g.marks[far] = pl.ink;
            } else if g.marks[far] == 0 {
                g.marks[far] = pl.seam;
            }
        }
    }
    for &i in &marked {
        g.col[i] = g.marks[i];
        g.marks[i] = 0;
    }
    // the mark on his chest, when it's facing us
    let ch = &s.b[F_CHEST];
    let chest_at = add(ch.t, mv(&ch.r, [3.0, 1.6, -1.8]));
    if mv(&ch.r, [1.0, 0.0, 0.0])[2] > 0.15 {
        let i = (OY - js_round(chest_at[1]) as i64) * BW + (OX + js_round(chest_at[0]) as i64);
        if g.part_at(i) == Some(P_CHEST) {
            g.col[i as usize] = pl.mark;
        }
    }
    // the outline, and everything into a small list of pixels
    let mut ring_list: Vec<usize> = Vec::new();
    for &i in &touched {
        for d in dirs {
            let j = i as i64 + d;
            if g.part_at(j) == Some(0) && g.ring[j as usize] < 0 {
                g.ring[j as usize] = i as i64;
                ring_list.push(j as usize);
            }
        }
    }
    let n = touched.len() + ring_list.len();
    let mut out = Posed { n, ..Posed::default() };
    let src_for = |g: &Grid, i: usize| -> i16 {
        let f = g.fr[i];
        if f < 0 {
            return -1;
        }
        let (a, r0) = (&s.b[f as usize], &rest[f as usize]);
        let wx = (i as i64 % BW - OX) as f64;
        let wy = (OY - i as i64 / BW) as f64;
        let p = [wx, wy, g.zb[i] as f64];
        let lp = tv(&a.r, sub(p, a.t));
        let rp = add(r0.t, mv(&r0.r, lp));
        let c = js_round(rp[0]) + 12.0;
        let r = 38.0 - js_round(rp[1]);
        if c < 0.0 || r < 0.0 || c >= W0 as f64 || r >= H0 as f64 || c.is_nan() || r.is_nan() {
            -1
        } else {
            (r as i32 * W0 + c as i32) as i16
        }
    };
    let (mut x0, mut y0, mut x1, mut y1) = (1_000_000_000i32, 1_000_000_000i32, -1_000_000_000i32, -1_000_000_000i32);
    let mut push = |out: &mut Posed, i: usize, c: u32, s: i16, ink: bool| {
        let x = (i as i64 % BW - OX) as i32;
        let y = (i as i64 / BW - OY) as i32;
        out.xs.push(x as i16);
        out.ys.push(y as i16);
        out.color.push(c);
        out.src.push(s);
        out.ink.push(u8::from(ink));
        x0 = x0.min(x);
        x1 = x1.max(x);
        y0 = y0.min(y);
        y1 = y1.max(y);
    };
    for &i in &touched {
        let c = g.col[i];
        let sv = src_for(g, i);
        push(&mut out, i, c, sv, c == pl.ink);
    }
    for &j in &ring_list {
        let i = g.ring[j] as usize;
        let sv = src_for(g, i);
        push(&mut out, j, pl.ink, sv, true);
    }
    let zs: f64 = touched.iter().map(|&i| g.zb[i] as f64).sum();
    out.x0 = x0;
    out.y0 = y0;
    out.x1 = x1;
    out.y1 = y1;
    out.z = if touched.is_empty() { 0.0 } else { zs / touched.len() as f64 };
    for &i in &touched {
        g.part[i] = 0;
        g.tones[i] = -1;
    }
    for &j in &ring_list {
        g.ring[j] = -1;
    }
    out
}

fn render(g: &mut Grid, pose: &Pose, rest: &[BFrame; 15]) -> Posed {
    let mut s = solve(pose);
    let q = s.q;
    let whole = whole_turn(&q);
    if truthy(whole) {
        turn_all(&mut s, whole);
    }
    let air = q.y.unwrap_or(0.0) < -0.5;
    g.clip_body = !air;
    g.clip_weapon = !air;
    draw_body(g, &s);
    g.clip_body = false;
    if let Some(w) = s.w {
        draw_weapon(w, g, &Along::new(s.arm_n.end, s.axis, s.side, P_WEAPON), &q, s.arm_f.end);
    }
    if let Some(ow) = s.ow {
        let sb = q.sb.unwrap_or(PI);
        let (mut ax, mut sd) = ([sb.cos(), -sb.sin(), 0.0], [-sb.sin(), -sb.cos(), 0.0]);
        if truthy(whole) {
            let r = rot_y(-whole);
            ax = mv(&r, ax);
            sd = mv(&r, sd);
        }
        draw_weapon(ow, g, &Along::new(s.arm_f.end, ax, sd, P_OFF_WEAPON), &q, s.arm_f.end);
    }
    g.clip_weapon = false;
    finish(g, &s, rest)
}

struct RigState {
    grid: Grid,
    rest: [BFrame; 15],
    cache: HashMap<String, Rc<Posed>>,
}
thread_local! {
    static RIG: RefCell<Option<RigState>> = const { RefCell::new(None) };
}
fn with_rig<R>(f: impl FnOnce(&mut RigState) -> R) -> R {
    RIG.with(|r| {
        let mut r = r.borrow_mut();
        let st = r.get_or_insert_with(|| RigState { grid: Grid::new(), rest: solve(&Pose::EMPTY).b, cache: HashMap::new() });
        f(st)
    })
}

/// The cache key of a pose: present keys with rounded values (angles to 1/20 rad), x/y/ghost left out, "air" when off
/// the floor. Two poses with the same key share one render, as upstream.
fn key_of(p: &Pose) -> String {
    use std::fmt::Write;
    let mut s = String::new();
    if p.y.unwrap_or(0.0) < -0.5 {
        s.push_str("air,");
    }
    for (name, v) in Pose::NUM_KEYS.iter().zip(p.nums()) {
        let Some(v) = v else { continue };
        if matches!(*name, "x" | "y" | "ghost") {
            continue;
        }
        let r = if matches!(*name, "sa" | "sb" | "wl" | "yaw" | "turn") { js_round(v * 20.0) } else { js_round(v) };
        let _ = write!(s, "{name}{r},");
    }
    for (name, v) in Pose::FLAG_KEYS.iter().zip(p.flags()) {
        if let Some(v) = v {
            let _ = write!(s, "{name}{v},");
        }
    }
    if let Some(w) = p.weapon {
        let _ = write!(s, "weapon{},", w.name());
    }
    if let Some(w) = p.off_weapon {
        let _ = write!(s, "offWeapon{},", w.name());
    }
    s
}

/// The pixels of a pose (cached per thread, like upstream's module cache, cleared past 6000 entries).
pub fn posed(pose: &Pose) -> Rc<Posed> {
    let key = key_of(pose);
    with_rig(|st| {
        if let Some(r) = st.cache.get(&key) {
            return r.clone();
        }
        if st.cache.len() > 6000 {
            st.cache.clear();
        }
        let r = Rc::new(render(&mut st.grid, pose, &st.rest));
        st.cache.insert(key, r.clone());
        r
    })
}
/// Forget every cached render on this thread (renders then match a fresh upstream process).
pub fn clear_cache() {
    with_rig(|st| st.cache.clear());
}

/// How far short of where the pose wants them the hands fall, in px: arms don't stretch.
pub fn shortfall(pose: &Pose) -> f64 {
    let s = solve(pose);
    let q = s.q;
    let near = len(sub(s.arm_n.end, [n(q.fx), -n(q.fy), s.arm_n.end[2]]));
    let far = if q.two == Some(true) { 0.0 } else { len(sub(s.arm_f.end, [n(q.bx), -n(q.by), s.arm_f.end[2]])) };
    near.max(far)
}

/// The skeleton of a pose, in the same space as [`posed`]: x right, y down, feet at (0, 0), z toward the viewer.
#[derive(Clone, Debug)]
pub struct Skeleton {
    pub joints: Vec<(&'static str, [f64; 3])>,
    pub bones: Vec<(&'static str, &'static str)>,
    /// What a pose moves (hands and feet); everything else is solved from them.
    pub handles: [&'static str; 4],
}
impl Skeleton {
    pub fn joint(&self, name: &str) -> Option<[f64; 3]> {
        self.joints.iter().find(|(n, _)| *n == name).map(|(_, v)| *v)
    }
}
pub fn skeleton(pose: &Pose) -> Skeleton {
    let mut s = solve(pose);
    let whole = whole_turn(&s.q);
    if truthy(whole) {
        turn_all(&mut s, whole);
    }
    let b = &s.b;
    let at = |f: &BFrame, v: V3| add(f.t, mv(&f.r, v));
    let j: Vec<(&'static str, V3)> = vec![
        ("pelvis", b[F_PELVIS].t),
        ("waist", b[F_WAIST].t),
        ("chest", b[F_CHEST].t),
        ("neck", b[F_NECK].t),
        ("head", b[F_HEAD].t),
        ("crown", at(&b[F_HEAD], [0.0, 3.3, 0.0])),
        ("shoulderN", s.s_n),
        ("elbowN", s.arm_n.joint),
        ("handN", s.arm_n.end),
        ("shoulderF", s.s_f),
        ("elbowF", s.arm_f.joint),
        ("handF", s.arm_f.end),
        ("hipN", s.hip_n),
        ("kneeN", s.leg_n.joint),
        ("ankleN", s.leg_n.end),
        ("toeN", at(&b[F_FOOT_N], [4.6, -1.1, 0.0])),
        ("hipF", s.hip_f),
        ("kneeF", s.leg_f.joint),
        ("ankleF", s.leg_f.end),
        ("toeF", at(&b[F_FOOT_F], [4.6, -1.1, 0.0])),
    ];
    let joints = j.into_iter().map(|(k, v)| (k, [v[0], -v[1], v[2]])).collect();
    let bones = vec![
        ("pelvis", "waist"),
        ("waist", "chest"),
        ("chest", "neck"),
        ("neck", "head"),
        ("head", "crown"),
        ("chest", "shoulderN"),
        ("shoulderN", "elbowN"),
        ("elbowN", "handN"),
        ("chest", "shoulderF"),
        ("shoulderF", "elbowF"),
        ("elbowF", "handF"),
        ("pelvis", "hipN"),
        ("hipN", "kneeN"),
        ("kneeN", "ankleN"),
        ("ankleN", "toeN"),
        ("pelvis", "hipF"),
        ("hipF", "kneeF"),
        ("kneeF", "ankleF"),
        ("ankleF", "toeF"),
    ];
    Skeleton { joints, bones, handles: ["handN", "handF", "ankleN", "ankleF"] }
}

/// One part of the figure drawn alone, with its own outline, and where its middle is. For exploded views.
#[derive(Clone, Debug)]
pub struct PosedPart {
    pub name: &'static str,
    pub part: i8,
    pub pixels: Posed,
    pub cx: f64,
    pub cy: f64,
}
pub fn posed_parts(pose: &Pose) -> Vec<PosedPart> {
    with_rig(|st| {
        let mut out = Vec::new();
        for (name, part) in PARTS {
            st.grid.only_part = part;
            let r = render(&mut st.grid, pose, &st.rest);
            st.grid.only_part = 0;
            if r.n == 0 {
                continue;
            }
            let cx = r.xs.iter().map(|&x| x as f64).sum::<f64>() / r.n as f64;
            let cy = r.ys.iter().map(|&y| y as f64).sum::<f64>() / r.n as f64;
            out.push(PosedPart { name, part, pixels: r, cx, cy });
        }
        out
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fx::poses::READY;

    #[test]
    fn js_helpers() {
        assert_eq!(js_round(0.49999999999999994), 0.0);
        assert_eq!(js_round(-0.5), 0.0);
        assert_eq!(js_round(2.5), 3.0);
        assert_eq!(js_hypot(&[3.0, 4.0]), 5.0);
        assert_eq!(to_i32(-7.9), -7);
        assert_eq!(to_i32(f64::NAN), 0);
        assert_eq!(to_i32(4294967297.0), 1);
    }

    #[test]
    fn stand_is_24_wide_and_about_38_tall_with_feet_on_the_floor() {
        let r = posed(&STAND);
        assert!(r.n > 200);
        assert!(r.y1 <= 0 && r.y1 >= -2, "feet at y = 0, got {}", r.y1);
        assert!((34..=44).contains(&(r.y1 - r.y0)), "height {}", r.y1 - r.y0);
        assert!(r.src.iter().filter(|&&s| s >= 0).count() > r.n / 2, "most pixels map back to the standing sprite");
    }

    #[test]
    fn same_rounded_pose_shares_a_render() {
        clear_cache();
        let a = posed(&READY.fx(11.2));
        let b = posed(&READY.fx(10.8).x(5.0));
        assert!(Rc::ptr_eq(&a, &b));
        let c = posed(&READY.fx(14.0));
        assert!(!Rc::ptr_eq(&a, &c));
    }

    #[test]
    fn weapons_draw_and_yaw_mirrors() {
        for w in Weapon::ALL {
            let p = posed(&READY.weapon(w).sa(-0.5));
            assert!(p.n > posed(&READY).n, "{w:?} adds pixels");
        }
        let two = posed(&READY.weapon(Weapon::Hammer).two(true).sa(-1.0));
        assert!(two.n > 0);
        let back = posed(&READY.face(-1.0));
        assert!(back.x0 < posed(&READY).x0 || back.x1 != posed(&READY).x1);
    }

    #[test]
    fn skeleton_and_parts() {
        let s = skeleton(&READY);
        let hand = s.joint("handN").unwrap();
        assert!((hand[0] - 11.0).abs() < 2.0 && (hand[1] + 21.0).abs() < 2.0, "near hand near (11, -21): {hand:?}");
        assert_eq!(s.bones.len(), 19);
        let parts = posed_parts(&READY);
        assert!(parts.iter().any(|p| p.name == "head") && parts.iter().all(|p| p.pixels.n > 0));
        assert!(shortfall(&READY) < 0.5);
        assert!(shortfall(&READY.fx(60.0)) > 20.0);
    }
}
