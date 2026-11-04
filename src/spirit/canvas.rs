//! Sub-pixel canvas: the spirit is painted in braille dots (2 x 4 per cell),
//! then every block becomes one braille cell whose colour is the average of
//! its highest-priority dots. Cells carry a foreground colour only.

use super::{Cell, SpiritGrid, CX, CY};

pub(super) type Rgb = [f32; 3];

/// Colour the spirit is assumed to sit on; fades blend toward it.
pub(super) const CARD_BG: Rgb = [22., 23., 32.];
pub(super) const WHITE: Rgb = [255., 255., 255.];
/// Priority of the eye catchlight; the only dots allowed to stay pure white.
pub(super) const EYE_PRIO: u8 = 3;
/// Brightest channel value for anything that is not the eye catchlight.
const MAX_NON_EYE: f32 = 250.0;

const BRAILLE_BITS: [[u32; 4]; 2] = [[0x01, 0x02, 0x04, 0x40], [0x08, 0x10, 0x20, 0x80]];

pub(super) fn lerp(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let t = t.clamp(0.0, 1.0);
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

pub(super) fn mul(c: Rgb, m: f32) -> Rgb {
    [(c[0] * m).min(255.), (c[1] * m).min(255.), (c[2] * m).min(255.)]
}

/// Canvas geometry for one spirit size.
#[derive(Clone, Copy)]
pub(super) struct Geom {
    /// real sub-pixels per design unit
    pub ppu: f32,
    pub w: usize,
    pub h: usize,
    /// body centre in real sub-pixels (always an integer)
    pub rcx: f32,
    pub rcy: f32,
    /// low-resolution sizes exaggerate flames and spikes so they still read
    pub small: bool,
}

pub(super) struct Canvas {
    pub g: Geom,
    px: Vec<Option<(Rgb, u8)>>,
    body: Vec<bool>,
}

impl Canvas {
    pub(super) fn new(g: Geom) -> Self {
        Canvas { g, px: vec![None; g.w * g.h], body: vec![false; g.w * g.h] }
    }

    fn idx(&self, x: i32, y: i32) -> Option<usize> {
        (x >= 0 && y >= 0 && (x as usize) < self.g.w && (y as usize) < self.g.h).then(|| y as usize * self.g.w + x as usize)
    }

    /// Design coordinates to real sub-pixels.
    pub(super) fn to_px(&self, x: f32, y: f32) -> (i32, i32) {
        (
            (self.g.rcx + (x - CX) * self.g.ppu).round() as i32,
            (self.g.rcy + (y - CY) * self.g.ppu).round() as i32,
        )
    }

    /// Real sub-pixels covered by one design unit (at least 1).
    pub(super) fn n(&self) -> i32 {
        self.g.ppu.round().max(1.0) as i32
    }

    pub(super) fn set(&mut self, x: usize, y: usize, c: Rgb, prio: u8, body: bool) {
        let i = y * self.g.w + x;
        self.px[i] = Some((c, prio));
        self.body[i] = body;
    }

    pub(super) fn set_px(&mut self, x: i32, y: i32, c: Rgb, prio: u8, skip_body: bool) {
        if let Some(i) = self.idx(x, y) {
            if !(skip_body && self.body[i]) {
                self.px[i] = Some((c, prio));
            }
        }
    }

    pub(super) fn clear_px(&mut self, x: i32, y: i32) {
        if let Some(i) = self.idx(x, y) {
            self.px[i] = None;
            self.body[i] = false;
        }
    }

    /// Fill the block of real pixels covered by the design unit whose top-left is (x, y).
    pub(super) fn blk(&mut self, x: f32, y: f32, c: Rgb, prio: u8, skip_body: bool) {
        let (x0, y0) = self.to_px(x, y);
        let n = self.n();
        for yy in y0..y0 + n {
            for xx in x0..x0 + n {
                self.set_px(xx, yy, c, prio, skip_body);
            }
        }
    }

    /// 2x4 blocks -> braille cells; the highest-priority dots win the cell.
    pub(super) fn cells(&self) -> SpiritGrid {
        let (cols, rows) = (self.g.w / 2, self.g.h / 4);
        let mut cells = Vec::with_capacity(cols * rows);
        for cy in 0..rows {
            for cx in 0..cols {
                let at = |dx: usize, dy: usize| self.px[(cy * 4 + dy) * self.g.w + cx * 2 + dx];
                let maxp = (0..4).flat_map(|dy| (0..2).map(move |dx| (dx, dy))).filter_map(|(dx, dy)| at(dx, dy)).map(|(_, p)| p).max();
                let Some(maxp) = maxp else {
                    cells.push(Cell { ch: ' ', fg: None });
                    continue;
                };
                let (mut bits, mut sum, mut n) = (0u32, [0f32; 3], 0f32);
                for dy in 0..4 {
                    for dx in 0..2 {
                        if let Some((c, p)) = at(dx, dy) {
                            if p == maxp {
                                bits |= BRAILLE_BITS[dx][dy];
                                sum = [sum[0] + c[0], sum[1] + c[1], sum[2] + c[2]];
                                n += 1.0;
                            }
                        }
                    }
                }
                let top = if maxp == EYE_PRIO { 255.0 } else { MAX_NON_EYE };
                let ch = char::from_u32(0x2800 + bits).unwrap_or(' ');
                cells.push(Cell { ch, fg: Some(to_rgb([sum[0] / n, sum[1] / n, sum[2] / n], top)) });
            }
        }
        SpiritGrid { cols: cols as u16, rows: rows as u16, cells }
    }
}

pub(super) fn to_rgb(c: Rgb, top: f32) -> (u8, u8, u8) {
    let q = |v: f32| v.clamp(0.0, top) as u8;
    (q(c[0]), q(c[1]), q(c[2]))
}
