//! A frame: `w` x `h` pixels whose top-left sits at (`ox`, `oy`) in world space. `color` holds opaque
//! pixels (0xAABBGGRR, 0 = empty) and `light` additive glow; `composite` puts them together.

#[derive(Clone, Debug)]
pub struct Frame {
    pub w: usize,
    pub h: usize,
    pub ox: i32,
    pub oy: i32,
    pub color: Vec<u32>,
    pub light: Vec<u32>,
}

impl Frame {
    pub fn new(w: usize, h: usize, ox: i32, oy: i32) -> Self {
        Frame { w, h, ox, oy, color: vec![0; w * h], light: vec![0; w * h] }
    }
    pub fn clear(&mut self) {
        self.color.fill(0);
        self.light.fill(0);
    }
}

/// What `composite_bytes` lays the frame over.
#[derive(Clone, Copy)]
pub enum Bg<'a> {
    /// Transparent: translucent color keeps its alpha, light becomes color with alpha.
    None,
    Flat(u32),
    /// One color per pixel, the frame's size.
    Per(&'a [u32]),
}

/// Color over a background, plus light, into RGBA bytes (w * h * 4). Mirrors upstream `composite`.
pub fn composite_bytes(f: &Frame, bg: Bg, light_k: f64) -> Vec<u8> {
    let n = f.w * f.h;
    let mut out = vec![0u8; n * 4];
    let ch = |v: u32, s: u32| ((v >> s) & 255) as f64;
    for i in 0..n {
        let c = f.color[i];
        let l = f.light[i] & 0xff_ffff;
        let ca = c >> 24;
        let (mut r, mut g, mut b, mut a);
        if ca == 255 {
            (r, g, b, a) = (ch(c, 0), ch(c, 8), ch(c, 16), 255.0);
        } else if let Bg::None = bg {
            (r, g, b, a) = (ch(c, 0), ch(c, 8), ch(c, 16), ca as f64);
        } else {
            let k = match bg {
                Bg::Flat(k) => k,
                Bg::Per(v) => v[i],
                Bg::None => 0,
            };
            let q = ca as f64 / 255.0;
            r = ch(c, 0) * q + ch(k, 0) * (1.0 - q);
            g = ch(c, 8) * q + ch(k, 8) * (1.0 - q);
            b = ch(c, 16) * q + ch(k, 16) * (1.0 - q);
            a = 255.0;
        }
        if l != 0 {
            let (lr, lg, lb) = (ch(l, 0) * light_k, ch(l, 8) * light_k, ch(l, 16) * light_k);
            if a < 255.0 {
                // over transparency: premultiply what's there, add the light, divide by the new alpha
                let pa = a / 255.0;
                let (pr, pg, pb) = (r * pa + lr, g * pa + lg, b * pa + lb);
                let na = pa.max(lr.max(lg).max(lb) / 255.0).min(1.0);
                if na > 0.0 {
                    r = (pr / na).min(255.0);
                    g = (pg / na).min(255.0);
                    b = (pb / na).min(255.0);
                    a = na * 255.0;
                }
            } else {
                r = (r + lr).min(255.0);
                g = (g + lg).min(255.0);
                b = (b + lb).min(255.0);
            }
        }
        // Uint8ClampedArray semantics: round half to even, clamp
        let q = |v: f64| v.round_ties_even().clamp(0.0, 255.0) as u8;
        out[i * 4..i * 4 + 4].copy_from_slice(&[q(r), q(g), q(b), q(a)]);
    }
    out
}

/// Composite over a flat opaque background color; returns packed 0xAABBGGRR pixels (alpha 255).
pub fn composite(f: &Frame, bg: u32) -> Vec<u32> {
    composite_bytes(f, Bg::Flat(bg), 1.0)
        .chunks_exact(4)
        .map(|p| (p[3] as u32) << 24 | (p[2] as u32) << 16 | (p[1] as u32) << 8 | p[0] as u32)
        .collect()
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn composite_adds_light_over_color() {
        let mut f = Frame::new(2, 1, 0, 0);
        f.color[0] = 0xff00_0010; // opaque r=0x10
        f.light[0] = 0xff00_0020; // r +0x20
        f.light[1] = 0xffff_ffff; // saturates over bg
        let px = composite(&f, 0xff00_0000);
        assert_eq!(px[0] & 255, 0x30);
        assert_eq!(px[0] >> 24, 255);
        assert_eq!(px[1] & 0xffffff, 0xffffff);
    }

    #[test]
    fn composite_translucent_color_over_bg() {
        let mut f = Frame::new(1, 1, 0, 0);
        f.color[0] = (128 << 24) | 200; // half-alpha red 200 over black
        let px = composite(&f, 0xff00_0000);
        assert!(((px[0] & 255) as i32 - 100).abs() <= 1);
    }

    #[test]
    fn frame_clear_zeroes_buffers() {
        let mut f = Frame::new(2, 2, 3, 4);
        f.color[1] = 5;
        f.light[2] = 6;
        f.clear();
        assert!(f.color.iter().chain(f.light.iter()).all(|&c| c == 0));
        assert_eq!((f.ox, f.oy), (3, 4));
    }
}
