//! Point lists for shapes effects keep reaching for: lightning paths, stars, polygons, trails.

use super::math::{Rng, TAU};

pub type Pt = (f64, f64);

#[derive(Clone, Copy)]
pub struct BoltOpts {
    pub jag: f64,
    pub depth: u32,
}
impl Default for BoltOpts {
    fn default() -> Self {
        BoltOpts { jag: 0.22, depth: 5 }
    }
}

/// A jagged lightning path from (x0, y0) to (x1, y1) by midpoint displacement. `jag` is how far a midpoint
/// may move sideways, as a share of the segment's length.
pub fn bolt(r: &mut Rng, x0: f64, y0: f64, x1: f64, y1: f64, o: BoltOpts) -> Vec<Pt> {
    let mut pts = vec![(x0, y0), (x1, y1)];
    for _ in 0..o.depth {
        let mut next = vec![pts[0]];
        for i in 1..pts.len() {
            let ((ax, ay), (bx, by)) = (pts[i - 1], pts[i]);
            let (dx, dy) = (bx - ax, by - ay);
            let l = dx.hypot(dy);
            let off = (r.next() * 2.0 - 1.0) * o.jag * l;
            let l1 = if l == 0.0 { 1.0 } else { l };
            next.push(((ax + bx) / 2.0 + (-dy / l1) * off, (ay + by) / 2.0 + (dx / l1) * off));
            next.push(pts[i]);
        }
        pts = next;
    }
    pts
}

#[derive(Clone, Copy)]
pub struct ForkOpts {
    pub count: usize,
    pub len: f64,
    pub jag: f64,
    pub spread: f64,
}
impl Default for ForkOpts {
    fn default() -> Self {
        ForkOpts { count: 3, len: 20.0, jag: 0.3, spread: 0.9 }
    }
}

/// Forks off a bolt path: `count` branches, each up to `len` px long. Returns a list of paths.
pub fn forks(r: &mut Rng, path: &[Pt], o: ForkOpts) -> Vec<Vec<Pt>> {
    let mut out = Vec::new();
    for _ in 0..o.count {
        let i = 1 + (r.next() * (path.len() as f64 - 2.0)).floor() as usize;
        let ((x, y), (px, py)) = (path[i], path[i - 1]);
        let side = if r.next() < 0.5 { -1.0 } else { 1.0 };
        let a = (y - py).atan2(x - px) + side * o.spread * (0.4 + 0.6 * r.next());
        let l = o.len * (0.4 + 0.6 * r.next());
        out.push(bolt(r, x, y, x + a.cos() * l, y + a.sin() * l, BoltOpts { jag: o.jag, depth: 3 }));
    }
    out
}

/// A star: n points between radii r1 (tips) and r0 (inner corners). Upstream default: n = 5, rot = -PI/2.
pub fn star(cx: f64, cy: f64, r0: f64, r1: f64, n: usize, rot: f64) -> Vec<Pt> {
    (0..n * 2)
        .map(|i| {
            let a = rot + i as f64 * std::f64::consts::PI / n as f64;
            let r = if i % 2 == 1 { r0 } else { r1 };
            (cx + a.cos() * r, cy + a.sin() * r)
        })
        .collect()
}

/// A regular polygon with n sides. Upstream default: n = 6, rot = 0.
pub fn ngon(cx: f64, cy: f64, r: f64, n: usize, rot: f64) -> Vec<Pt> {
    (0..n)
        .map(|i| {
            let a = rot + i as f64 * TAU / n as f64;
            (cx + a.cos() * r, cy + a.sin() * r)
        })
        .collect()
}

/// Positions of pos(t - i * dt) for i = 0..n-1: a moving thing's trail, newest first.
pub fn trail(pos: impl Fn(f64) -> Pt, t: f64, n: usize, dt: f64) -> Vec<Pt> {
    (0..n).map(|i| pos((t - i as f64 * dt).max(0.0))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bolt_has_expected_length_and_endpoints() {
        let mut r = Rng::new(3);
        let p = bolt(&mut r, 0.0, 0.0, 10.0, 0.0, BoltOpts::default());
        assert_eq!(p.len(), 33);
        assert_eq!((p[0], p[32]), ((0.0, 0.0), (10.0, 0.0)));
        let f = forks(&mut r, &p, ForkOpts::default());
        assert_eq!(f.len(), 3);
        assert_eq!(f[0].len(), 9);
    }

    #[test]
    fn star_ngon_trail() {
        let s = star(0.0, 0.0, 1.0, 2.0, 5, -std::f64::consts::FRAC_PI_2);
        assert_eq!(s.len(), 10);
        assert!(s[0].0.abs() < 1e-9 && (s[0].1 + 2.0).abs() < 1e-9);
        assert_eq!(ngon(0.0, 0.0, 1.0, 6, 0.0).len(), 6);
        let t = trail(|t| (t, 0.0), 0.5, 3, 0.4);
        assert_eq!(t.len(), 3);
        assert!((t[1].0 - 0.1).abs() < 1e-12 && t[2] == (0.0, 0.0));
    }
}
