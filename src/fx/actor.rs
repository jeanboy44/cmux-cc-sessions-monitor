//! The stock character effects are tried on: the 12 x 19 `HERO` and the 24 x 38 `HERO2` (drawn with the
//! neutral `DUMMY_PAL`). Pixel grids and palettes are copied verbatim from upstream `core/actor.js`.
//! Effects only read the character's silhouette (`actor_mask`); `draw_actor` paints it with its feet at a point.

use super::color::hex;
use super::frame::Frame;
use super::painter::{Mask, Painter, SpriteOpts};

pub const HERO: [&str; 19] = [
    "....kkkkk...",
    "..kkhhhhhk..",
    ".khhhhhhhhk.",
    ".khhhhhhhhhk",
    ".khhssssshhk",
    ".khsessesshk",
    ".khsessesshk",
    "..ksssssssk.",
    "...kkSSSkk..",
    "..kttttttk..",
    ".kttTttTttk.",
    ".ksttTTttsk.",
    ".kstttttttk.",
    "..kbbbbbbk..",
    "..kppkkppk..",
    "..kppk.kppk.",
    "..kppk.kppk.",
    "..koook.kook",
    "..kkkkk.kkkk",
];
pub const HERO2: [&str; 38] = [
    "...........kkk..........",
    "..........kAABk.........",
    ".........kAAABJk........",
    ".........kABBJJk........",
    ".........kBBBJCDk.......",
    ".........kBCJDDDk.......",
    ".........kkJJJDk........",
    ".......kkkkABBJCkk......",
    "......kAABCJBCCCkDk.....",
    "......kABBCJCCCmDkDk....",
    "......kBBCDDJCDDDkDk....",
    "......kABCDJDDDDDkk.....",
    ".....kAABDkDDDDJkk......",
    ".....kABCDkDDJJGkk......",
    ".....kABCkJJJGGGkk......",
    "....kJJCDkFFFGGGk.......",
    "....kAJJDkJJJGGGk.......",
    "....kJCJJkABBJGGk.......",
    ".....kJCCkBBCDJJJk......",
    ".....kCCCJBBCDJJDk......",
    ".....kCCCDJCCDkJDDk.....",
    "......kCDDJBCDkDDDk.....",
    ".......kDJJJCDkDDDkk....",
    ".......kJBCJCDkDDDkk....",
    ".......kBDJBCDkCDDkk....",
    "........kJJBCDkCDDk.....",
    "........kBBBCDkCCDDk....",
    "........kBBBCDkCCDDk....",
    ".........kBBCDkJDDDk....",
    "........kBJJJJkCJJJk....",
    "........kBBCCDkCCDDk....",
    "........kBBCDDkCDDk.....",
    "........kBBCDkCCDDk.....",
    "........kBCCDkCCDDk.....",
    "........kBCDDkCDDDk.....",
    ".......kJBCJkkkDJkkkk...",
    ".......kAJJAAAAkCCCCCk..",
    "........kkkkkkkkkkkkk...",
];
pub const HERO_PAL: &[(char, &str)] = &[
    ('k', "#17121f"),
    ('h', "#7a4128"),
    ('s', "#f3c49d"),
    ('S', "#c98b6c"),
    ('e', "#17121f"),
    ('t', "#5f6f94"),
    ('T', "#46536f"),
    ('b', "#8a5a32"),
    ('p', "#38334a"),
    ('o', "#6a4030"),
    ('H', "#a35d34"),
    ('w', "#ffffff"),
    ('m', "#a3564a"),
    ('g', "#e8c060"),
    ('L', "#7f90b6"),
    ('O', "#8f5a3f"),
    ('P', "#4b4560"),
    ('c', "#f53cd1"),
    ('C', "#ff8fe6"),
    ('D', "#a31f8a"),
];
pub const DUMMY_PAL: &[(char, &str)] = &[
    ('k', "#0e0c14"),
    ('A', "#e4e6ec"),
    ('B', "#b6b9c3"),
    ('C', "#868995"),
    ('D', "#5c5e6a"),
    ('E', "#6e707c"),
    ('F', "#4b4d58"),
    ('G', "#34353f"),
    ('J', "#2a2b33"),
    ('m', "#f53cd1"),
];

/// Which stock character: `Hero` is the 12 x 19 one, `Dummy` the 24 x 38 neutral shell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActorKind {
    Hero,
    Dummy,
}

impl ActorKind {
    pub fn rows(self) -> &'static [&'static str] {
        match self {
            ActorKind::Hero => &HERO,
            ActorKind::Dummy => &HERO2,
        }
    }
    pub fn palette(self) -> &'static [(char, &'static str)] {
        match self {
            ActorKind::Hero => HERO_PAL,
            ActorKind::Dummy => DUMMY_PAL,
        }
    }
    /// Sprite size in pixels, (w, h).
    pub fn size(self) -> (usize, usize) {
        let r = self.rows();
        (r[0].chars().count(), r.len())
    }
}

fn palette_u32(kind: ActorKind) -> Vec<(char, u32)> {
    kind.palette().iter().map(|&(c, h)| (c, hex(h))).collect()
}

/// Top-left of the sprite relative to its feet: (-floor(w / 2), -h).
pub fn actor_origin(kind: ActorKind) -> (i32, i32) {
    let (w, h) = kind.size();
    (-((w / 2) as i32), -(h as i32))
}

/// Silhouette: every non-"." cell is set.
pub fn actor_mask(kind: ActorKind) -> Mask {
    let rows = kind.rows();
    let (w, h) = kind.size();
    let mut data = vec![0u8; w * h];
    for (y, r) in rows.iter().enumerate() {
        for (x, ch) in r.chars().enumerate() {
            if ch != '.' {
                data[y * w + x] = 1;
            }
        }
    }
    Mask { w, h, data }
}

/// The character's own colors row by row (0 where empty), for effects that recolor it.
pub fn actor_pixels(kind: ActorKind) -> Vec<u32> {
    let pal = palette_u32(kind);
    let (w, h) = kind.size();
    let mut data = vec![0u32; w * h];
    for (y, r) in kind.rows().iter().enumerate() {
        for (x, ch) in r.chars().enumerate() {
            if ch != '.' {
                data[y * w + x] = pal.iter().find(|(k, _)| *k == ch).map_or(0, |(_, c)| *c);
            }
        }
    }
    data
}

/// Draw the character with its feet at (x, y) in frame pixel coordinates (frame origin offset applies).
pub fn draw_actor(frame: &mut Frame, x: i32, y: i32, kind: ActorKind) {
    let pal = palette_u32(kind);
    let (ax, ay) = actor_origin(kind);
    let mut p = Painter::new(frame);
    p.place(x as f64, y as f64, false, 0.0);
    p.sprite(kind.rows(), &pal, ax as f64, ay as f64, &SpriteOpts::default());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hero_and_dummy_are_24x38() {
        assert_eq!(HERO2.len(), 38);
        assert!(HERO2.iter().all(|r| r.chars().count() == 24));
        assert_eq!(HERO.len(), 19);
        assert!(HERO.iter().all(|r| r.chars().count() == 12));
        // every glyph in the grid has a color in its palette
        for kind in [ActorKind::Hero, ActorKind::Dummy] {
            for r in kind.rows() {
                for ch in r.chars().filter(|&c| c != '.') {
                    assert!(kind.palette().iter().any(|(k, _)| *k == ch), "{kind:?} missing {ch}");
                }
            }
        }
    }

    #[test]
    fn mask_matches_drawn_pixels() {
        let mut f = Frame::new(24, 38, 0, 0);
        draw_actor(&mut f, 12, 38, ActorKind::Dummy);
        let m = actor_mask(ActorKind::Dummy);
        assert_eq!((m.w, m.h), (24, 38));
        for i in 0..24 * 38 {
            assert_eq!(f.color[i] != 0, m.data[i] != 0, "pixel {i}");
        }
        assert_eq!(f.color[10], actor_pixels(ActorKind::Dummy)[10]);
    }
}
