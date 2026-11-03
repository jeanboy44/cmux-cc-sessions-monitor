//! Colors are `0xAABBGGRR` u32 values (R in the low byte), as in pixelfx `core/color.js`.

use super::math::clamp;

fn pack(r: f64, g: f64, b: f64) -> u32 {
    // `as i64` truncates toward zero like JS ToInt32 for in-range values; `& 255` wraps.
    let ch = |v: f64| (v as i64 & 255) as u32;
    (255 << 24) | (ch(b) << 16) | (ch(g) << 8) | ch(r)
}
pub fn rgb(r: u32, g: u32, b: u32) -> u32 {
    (255 << 24) | ((b & 255) << 16) | ((g & 255) << 8) | (r & 255)
}
pub fn red(c: u32) -> u32 {
    c & 255
}
pub fn green(c: u32) -> u32 {
    (c >> 8) & 255
}
pub fn blue(c: u32) -> u32 {
    (c >> 16) & 255
}

/// "#rgb" or "#rrggbb" to a color number. Unparsable input yields 0 components, like `parseInt` NaN | 0.
pub fn hex(h: &str) -> u32 {
    let mut s: String = h.replacen('#', "", 1);
    if s.len() == 3 {
        s = s.chars().flat_map(|c| [c, c]).collect();
    }
    let s: String = s.chars().take(6).collect();
    let v = u32::from_str_radix(&s, 16).unwrap_or(0);
    rgb((v >> 16) & 255, (v >> 8) & 255, v & 255)
}
pub fn to_hex(c: u32) -> String {
    format!("#{:02x}{:02x}{:02x}", red(c), green(c), blue(c))
}

/// Linear mix of two colors.
pub fn mix(a: u32, b: u32, t: f64) -> u32 {
    let t = clamp(t, 0.0, 1.0);
    let ch = |x: u32, y: u32| x as f64 + (y as f64 - x as f64) * t;
    pack(ch(red(a), red(b)), ch(green(a), green(b)), ch(blue(a), blue(b)))
}
/// Multiply a color's brightness by k.
pub fn scale(c: u32, k: f64) -> u32 {
    pack((red(c) as f64 * k).min(255.0), (green(c) as f64 * k).min(255.0), (blue(c) as f64 * k).min(255.0))
}
/// Perceived brightness 0..1.
pub fn luma(c: u32) -> f64 {
    (0.299 * red(c) as f64 + 0.587 * green(c) as f64 + 0.114 * blue(c) as f64) / 255.0
}

/// A list of hex strings (dark to bright) to color numbers.
pub fn ramp(list: &[&str]) -> Vec<u32> {
    list.iter().map(|h| hex(h)).collect()
}
/// Resample a ramp to exactly n steps, keeping its ends.
pub fn fit(r: &[u32], n: usize) -> Vec<u32> {
    if r.len() == n {
        return r.to_vec();
    }
    (0..n)
        .map(|i| r[((i * (r.len() - 1)) as f64 / n.saturating_sub(1).max(1) as f64).round() as usize])
        .collect()
}
/// The ramp step for t in 0..1, no blending.
pub fn at(r: &[u32], t: f64) -> u32 {
    r[clamp((t * r.len() as f64).floor(), 0.0, (r.len() - 1) as f64) as usize]
}
/// Blended color along a ramp (for light, where smooth is right).
pub fn blend(r: &[u32], t: f64) -> u32 {
    let x = clamp(t, 0.0, 1.0) * (r.len() - 1) as f64;
    let i = ((r.len() - 2) as f64).min(x.floor()) as usize;
    mix(r[i], r[i + 1], x - i as f64)
}

/// Hue-shifted ramps, dark to bright, keyed by name.
pub const PAL: &[(&str, &[&str])] = &[
    ("fire", &["#1f0b10", "#4a1118", "#8c1d1c", "#cc3a1a", "#f2701f", "#ffae35", "#ffe27a", "#fff8d6"]),
    ("ember", &["#1a0a0e", "#3d1012", "#701a14", "#a8301a", "#e0561e", "#ff8a2a", "#ffc46a"]),
    ("lava", &["#14060a", "#380a10", "#6e1210", "#ac2410", "#e8501a", "#ff8c2a", "#ffd25a"]),
    ("smoke", &["#0e0c12", "#1d1a22", "#2e2a35", "#433e4b", "#5c5664", "#7b7484", "#a39cab"]),
    ("dust", &["#1c1610", "#33281c", "#4f3f2c", "#6e5a40", "#927a58", "#b89f78", "#dcc8a2"]),
    ("ice", &["#0c1430", "#162c5c", "#1f4f94", "#2f7fcc", "#58b4ec", "#9bdcff", "#d8f4ff", "#ffffff"]),
    ("water", &["#081830", "#0e2e5a", "#14508c", "#1c76bc", "#36a2e0", "#78ccf4", "#c4ecff", "#ffffff"]),
    ("electric", &["#0a0f2e", "#172a78", "#2550d0", "#3f8cff", "#7cc8ff", "#c8f0ff", "#ffffff"]),
    ("volt", &["#24160a", "#5e3a0c", "#a8680f", "#e8a218", "#ffd23c", "#fff08a", "#ffffff"]),
    ("poison", &["#0d1a0c", "#173a14", "#265e18", "#3f8a1e", "#6cb82a", "#a8e04a", "#e0ff8a"]),
    ("holy", &["#2a1c08", "#5a3e10", "#9a6c18", "#d8a428", "#ffd84a", "#fff09a", "#fffbe6", "#ffffff"]),
    ("heal", &["#08201a", "#0f3f2c", "#17663e", "#24924e", "#3ec466", "#7ee88a", "#c8ffc4", "#ffffff"]),
    ("arcane", &["#140a26", "#2b1252", "#481c88", "#6b2cc0", "#9a48ec", "#c684ff", "#ecc8ff", "#ffffff"]),
    ("shadow", &["#050308", "#0f0a18", "#1c1230", "#2c1a4a", "#432468", "#5e3290", "#8048b8", "#a970e0"]),
    ("blood", &["#14040a", "#3a0a14", "#6a101c", "#a01824", "#d42a30", "#ff5a50", "#ff9a8a"]),
    ("earth", &["#140e0a", "#2a1e14", "#47321e", "#69492a", "#8e6638", "#b88a52", "#dcb47c"]),
    ("stone", &["#0e0e12", "#1c1c24", "#2c2c38", "#42424f", "#5c5c6b", "#7d7d8c", "#a4a4b2", "#d0d0da"]),
    ("gold", &["#1e1206", "#4a2c08", "#86520c", "#c48412", "#f0b424", "#ffe066", "#fff6c4", "#ffffff"]),
    ("leaf", &["#0c1a0e", "#163418", "#225224", "#357530", "#4f9a3a", "#7cc048", "#b4e070", "#e4ffb0"]),
    ("wood", &["#160c06", "#2e1a0c", "#4a2c14", "#6a421e", "#8e5c2c", "#b47c40", "#d8a464"]),
    ("metal", &["#0c0e14", "#1c2230", "#2e3648", "#4a5468", "#6e7a90", "#9aa6ba", "#d0d8e6", "#ffffff"]),
    ("bone", &["#1a1814", "#3a352c", "#5e5648", "#8a806c", "#b8ad96", "#e4dcc8", "#fffaf0"]),
    ("neon", &["#1a0516", "#3d0a33", "#701560", "#b0228f", "#f53cd1", "#ff86e6", "#ffd2f6", "#ffffff"]),
    ("cyber", &["#041418", "#08303a", "#0c5260", "#108088", "#18b4b0", "#4ce8d8", "#b4fff0", "#ffffff"]),
    ("matrix", &["#020a04", "#05200c", "#0a3c16", "#106624", "#1a9a34", "#3cd650", "#a4ffa8", "#e8ffe8"]),
    ("candy", &["#ff4a5a", "#ff9a2a", "#ffe03c", "#5ad84a", "#3cc8ff", "#7a6cff", "#f53cd1"]),
    ("night", &["#07060d", "#0c0a18", "#131026", "#1b1734", "#241f44", "#2f2956"]),
];

/// Look up a palette by name.
pub fn pal(name: &str) -> Option<&'static [&'static str]> {
    PAL.iter().find(|(k, _)| *k == name).map(|(_, v)| *v)
}

pub const WHITE: u32 = 0xFFFF_FFFF;
pub const BLACK: u32 = 0xFF00_0000;
/// `#0b0a10`
pub const INK: u32 = 0xFF10_0A0B;
/// `#f53cd1`
pub const PATCH: u32 = 0xFFD1_3CF5;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_rgb_roundtrip() {
        assert_eq!(rgb(1, 2, 3), 0xFF03_0201);
        assert_eq!(hex("#ff8000"), 0xFF00_80FF);
        assert_eq!(hex("#f80"), hex("#ff8800"));
        assert_eq!(hex("#ffffff"), WHITE);
        assert_eq!(INK, rgb(0x0b, 0x0a, 0x10));
    }

    #[test]
    fn mix_midpoint() {
        // 127.5 truncates to 127 like JS `& 255`.
        assert_eq!(mix(rgb(0, 0, 0), rgb(255, 255, 255), 0.5), rgb(127, 127, 127));
        assert_eq!(mix(rgb(10, 20, 30), rgb(200, 100, 50), 0.0), rgb(10, 20, 30));
        assert_eq!(mix(rgb(10, 20, 30), rgb(200, 100, 50), 2.0), rgb(200, 100, 50));
    }

    #[test]
    fn fit_resamples_ramp_to_n() {
        let r = [hex("#000000"), hex("#808080"), hex("#ffffff")];
        assert_eq!(fit(&r, 3), r.to_vec());
        assert_eq!(fit(&r, 5), vec![r[0], r[1], r[1], r[2], r[2]]);
        assert_eq!(fit(&r, 2), vec![r[0], r[2]]);
        assert_eq!(fit(&r, 1), vec![r[0]]);
    }
}
