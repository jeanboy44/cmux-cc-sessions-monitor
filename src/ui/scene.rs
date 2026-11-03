//! Braille scene renderer for pixelfx frames.
//!
//! Each terminal cell shows a 2 x 4 block of source pixels as one braille glyph (U+2800 + dot bits).
//! A cell has one foreground color: the average RGB of its lit pixels. A pixel is lit when its alpha is
//! above 0 after compositing over the scene background. The cell background is always `SCENE_BG`.

#![allow(dead_code)] // wired up by the screen integration task

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;

/// Source pixels per braille dot along each axis (1 = native 1:1). Applied only in `scene_cells`
/// and `sample`; only 1 is supported for now.
pub const SCALE: usize = 1;

/// Scene background (0xAABBGGRR, opaque): a near-black blue-violet that keeps dark ink pixels
/// distinguishable from empty space. Also the cell background color of every rendered cell.
pub const SCENE_BG: u32 = 0xff1a_1210;

const CELL_W: usize = 2;
const CELL_H: usize = 4;

/// Braille dot bit for the pixel at (dx, dy) inside a 2 x 4 cell.
const DOT_BITS: [[u32; CELL_W]; CELL_H] = [[0x01, 0x08], [0x02, 0x10], [0x04, 0x20], [0x40, 0x80]];

/// Terminal cells (columns, rows) needed for a `w_px` x `h_px` frame, rounding up.
pub fn scene_cells(w_px: usize, h_px: usize) -> (u16, u16) {
    let cw = (w_px / SCALE).div_ceil(CELL_W);
    let ch = (h_px / SCALE).div_ceil(CELL_H);
    (cw.min(u16::MAX as usize) as u16, ch.min(u16::MAX as usize) as u16)
}

/// Pixel at the (scaled) position, or None outside the frame.
fn sample(rgba: &[u32], w_px: usize, h_px: usize, x: usize, y: usize) -> Option<u32> {
    let (sx, sy) = (x * SCALE, y * SCALE);
    if sx >= w_px || sy >= h_px {
        return None;
    }
    rgba.get(sy * w_px + sx).copied()
}

/// Draws `rgba` (0xAABBGGRR, `w_px` x `h_px`, row-major) as braille into `area` of `buf`, clipped to the
/// area and to the buffer. A pixel is lit when its alpha is above 0 and its RGB differs from `bg` (the
/// color the frame was composited over, so untouched scene pixels stay dark). Cells are written with
/// `bg` as their background. If `rgba.len() != w_px * h_px` nothing is drawn.
pub fn render_braille(buf: &mut Buffer, area: Rect, rgba: &[u32], w_px: usize, h_px: usize, bg: u32) {
    let area = area.intersection(*buf.area());
    if area.width == 0 || area.height == 0 || w_px == 0 || h_px == 0 || rgba.len() != w_px * h_px {
        return;
    }
    let bg_color = rgb_color(bg);
    let (cols, rows) = scene_cells(w_px, h_px);
    for cy in 0..rows.min(area.height) {
        for cx in 0..cols.min(area.width) {
            let (mut bits, mut n, mut sum) = (0u32, 0u32, [0u32; 3]);
            for (dy, row) in DOT_BITS.iter().enumerate() {
                for (dx, bit) in row.iter().enumerate() {
                    let (x, y) = (cx as usize * CELL_W + dx, cy as usize * CELL_H + dy);
                    if let Some(p) = sample(rgba, w_px, h_px, x, y) {
                        if p >> 24 > 0 && (p ^ bg) & 0xff_ffff != 0 {
                            bits |= bit;
                            n += 1;
                            sum[0] += p & 255;
                            sum[1] += (p >> 8) & 255;
                            sum[2] += (p >> 16) & 255;
                        }
                    }
                }
            }
            let cell = &mut buf[(area.x + cx, area.y + cy)];
            cell.set_bg(bg_color);
            if n == 0 {
                cell.set_char(' ');
            } else {
                let avg = |v: u32| ((v + n / 2) / n) as u8;
                cell.set_char(char::from_u32(0x2800 + bits).unwrap_or(' '));
                cell.set_fg(Color::Rgb(avg(sum[0]), avg(sum[1]), avg(sum[2])));
            }
        }
    }
}

fn rgb_color(c: u32) -> Color {
    Color::Rgb((c & 255) as u8, ((c >> 8) & 255) as u8, ((c >> 16) & 255) as u8)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fx::actor::{draw_actor, ActorKind};
    use crate::fx::frame::{composite, Frame};

    fn blank(w: usize, h: usize) -> Buffer {
        Buffer::empty(Rect::new(0, 0, w as u16, h as u16))
    }

    #[test]
    fn cells_for_native_hero() {
        assert_eq!(scene_cells(24, 38), (12, 10));
    }

    #[test]
    fn single_lit_pixel_sets_one_braille_dot() {
        let mut px = vec![0u32; 2 * 4];
        px[0] = 0xff00_00ff;
        let mut buf = blank(1, 1);
        render_braille(&mut buf, Rect::new(0, 0, 1, 1), &px, 2, 4, SCENE_BG);
        assert_eq!(buf[(0, 0)].symbol(), "\u{2801}");
        assert_eq!(buf[(0, 0)].fg, Color::Rgb(255, 0, 0));
    }

    #[test]
    fn cell_color_is_average_of_lit_pixels() {
        let mut px = vec![0u32; 2 * 4];
        px[0] = 0xff00_0000 | 200; // r=200
        px[1] = 0xff00_0000 | 100; // r=100
        px[2] = 0xff00_0000 | (60 << 8); // g=60
        let mut buf = blank(1, 1);
        render_braille(&mut buf, Rect::new(0, 0, 1, 1), &px, 2, 4, SCENE_BG);
        // dots: (0,0)=0x01, (1,0)=0x08, (0,1)=0x02
        assert_eq!(buf[(0, 0)].symbol(), "\u{280b}");
        assert_eq!(buf[(0, 0)].fg, Color::Rgb(100, 20, 0));
    }

    #[test]
    fn zero_area_and_clipped_area_do_not_panic() {
        let px = vec![0xff00_00ffu32; 24 * 38];
        let mut buf = blank(12, 10);
        for area in [Rect::new(0, 0, 0, 0), Rect::new(0, 0, 1, 1), Rect::new(0, 0, 5, 3), Rect::new(10, 8, 12, 10), Rect::new(50, 50, 4, 4)] {
            render_braille(&mut buf, area, &px, 24, 38, SCENE_BG);
        }
        assert_eq!(buf[(0, 0)].symbol(), "\u{28ff}");
        // mismatched length and empty frame: no panic, nothing drawn
        let mut buf = blank(4, 4);
        let area = buf.area;
        render_braille(&mut buf, area, &px[..10], 24, 38, SCENE_BG);
        render_braille(&mut buf, area, &[], 0, 0, SCENE_BG);
        assert_eq!(buf[(0, 0)].symbol(), " ");
    }

    #[test]
    fn dummy_renders_a_plausible_number_of_cells() {
        let mut f = Frame::new(24, 38, 0, 0);
        draw_actor(&mut f, 12, 38, ActorKind::Dummy);
        let lit = composite(&f, SCENE_BG);
        let mut buf = blank(12, 10);
        let area = buf.area;
        render_braille(&mut buf, area, &lit, 24, 38, SCENE_BG);
        let filled = buf.content().iter().filter(|c| c.symbol() != " ").count();
        assert!((40..=100).contains(&filled), "filled cells = {filled}");
    }
}
