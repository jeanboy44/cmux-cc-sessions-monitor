//! "Pochi" — the monitor's own pixel mascot.
//!
//! Pochi is a small round mochi-dumpling sprite with a single antenna pixel
//! bent off its top-right corner whose tip glows in the status accent color,
//! two dot eyes and two stubby feet. The silhouette (dome body + antenna, no
//! side arms, no flat top) is deliberately different from Claude Code's
//! mascot. Kept tiny and flat on purpose: no shine, few sparks.
//!
//! Sprites are drawn with half-block characters: every terminal cell holds two
//! vertical pixels (`▀` fg = top pixel, bg = bottom pixel; `▄` for a lone
//! bottom pixel; `█` for two equal pixels). Small glyphs (`!`, `z`, `?`, `✓`)
//! are overlaid on free cells next to the head.
//!
//! Animation is a pure function of the 12 fps `tick`: a status owns a table of
//! poses with hold durations, and the frame is `tick % loop_len`. Poses follow
//! the pixelfx-style rules: 12 fps holds, anticipation (squash) before a jump,
//! small 4-color palettes (shade → body, dark, accent).

use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

use crate::model::Status;
use crate::ui::style;

/// Sprite sizes in terminal cells (each cell = 1×2 pixels).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpriteSize {
    /// 6×3 cells (6×6 px) — the view A character card.
    Small,
    /// 8×4 cells (8×8 px) — the view B character card and the empty screen.
    Large,
}

impl SpriteSize {
    /// (columns, rows) in terminal cells.
    pub const fn cells(self) -> (u16, u16) {
        match self {
            SpriteSize::Small => (6, 3),
            SpriteSize::Large => (8, 4),
        }
    }
}

/// Per-frame tile effect after a status change.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TileFx {
    /// (columns, rows) to shift the tile by.
    pub shake: (i8, i8),
    /// Brighten the border for this frame.
    pub flash: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Px {
    Clear,
    Body,
    Shade,
    Dark,
    Accent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Eyes {
    Open,
    Glance,
    Alert,
    Closed,
    Happy,
    Odd,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Feet {
    Both,
    Spread,
    StepL,
    StepR,
    Tucked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Glyph {
    Bang,
    ZLow,
    ZHigh,
    Question,
    Check,
}

#[derive(Debug, Clone, Copy)]
struct Pose {
    /// Vertical body offset in pixels (+ = down / squash, - = airborne).
    dy: i16,
    eyes: Eyes,
    feet: Feet,
    /// Antenna tip lit in the accent color (else shade).
    lit: bool,
    /// Indices into the size's spark slot table.
    sparks: &'static [usize],
    glyph: Option<Glyph>,
}

const fn pose(dy: i16, eyes: Eyes, feet: Feet, lit: bool, sparks: &'static [usize], glyph: Option<Glyph>) -> Pose {
    Pose { dy, eyes, feet, lit, sparks, glyph }
}

use Eyes::*;
use Feet::*;

/// Working loop: shuffling feet, bobbing, antenna crackling with sparks.
const RUNNING: &[(u64, Pose)] = &[
    (2, pose(0, Open, StepL, true, &[0], None)),
    (2, pose(1, Open, Both, false, &[1, 2], None)),
    (2, pose(0, Glance, StepR, true, &[3], None)),
    (2, pose(1, Glance, Both, false, &[0, 8], None)),
];

/// Alert loop: wait, squash (anticipation), jump, land; the `!` blinks.
const NEEDS_INPUT: &[(u64, Pose)] = &[
    (3, pose(0, Alert, Both, true, &[], Some(Glyph::Bang))),
    (2, pose(1, Alert, Spread, true, &[], Some(Glyph::Bang))),
    (3, pose(-1, Alert, Tucked, true, &[1, 3], Some(Glyph::Bang))),
    (1, pose(1, Alert, Spread, true, &[], None)),
    (3, pose(0, Alert, Both, false, &[], None)),
];

/// Dozing loop: slow breathing, closed eyes, a rising `z`.
const IDLE: &[(u64, Pose)] = &[
    (6, pose(0, Closed, Both, false, &[], Some(Glyph::ZLow))),
    (6, pose(0, Closed, Both, false, &[], Some(Glyph::ZHigh))),
    (6, pose(1, Closed, Both, false, &[], None)),
    (6, pose(1, Closed, Both, false, &[], Some(Glyph::ZLow))),
];

/// Confused loop: looking around under a blinking `?`.
const UNKNOWN: &[(u64, Pose)] = &[
    (8, pose(0, Odd, Both, false, &[], Some(Glyph::Question))),
    (4, pose(0, Odd, Both, false, &[], None)),
    (8, pose(0, Glance, Both, false, &[], Some(Glyph::Question))),
    (4, pose(0, Glance, Both, false, &[], None)),
];

/// Final "stage clear" pose, held forever.
const DONE_HELD: Pose = pose(0, Happy, Both, true, &[], Some(Glyph::Check));

/// One-shot celebration: crouch, jump in a ring of sparkles, land, check,
/// then settle into exactly [`DONE_HELD`] so the hold never "pops".
const DONE_ONESHOT: &[(u64, Pose)] = &[
    (2, pose(1, Closed, Spread, false, &[], None)),
    (4, pose(-1, Happy, Tucked, true, &[4, 5, 6, 7], None)),
    (2, pose(1, Happy, Spread, true, &[8, 2], None)),
    (4, pose(0, Happy, Both, true, &[4, 7], Some(Glyph::Check))),
    (2, pose(0, Happy, Both, true, &[5, 6], Some(Glyph::Check))),
    (2, DONE_HELD),
];

fn poses(status: Status) -> &'static [(u64, Pose)] {
    match status {
        Status::Running => RUNNING,
        Status::NeedsInput => NEEDS_INPUT,
        Status::Idle => IDLE,
        Status::Unknown => UNKNOWN,
        Status::Done => &[(1, DONE_HELD)],
    }
}

fn total(table: &[(u64, Pose)]) -> u64 {
    table.iter().map(|(d, _)| d).sum()
}

/// Pose at `t`, which must be below the table's total length.
fn pick(table: &[(u64, Pose)], mut t: u64) -> Pose {
    for (dur, p) in table {
        if t < *dur {
            return *p;
        }
        t -= dur;
    }
    table[table.len() - 1].1
}

/// Loop length in ticks of a status animation (`Done` is a 1-tick hold).
/// Only tests need it; the app passes the raw tick and `sprite` wraps it.
#[cfg(test)]
pub fn loop_len(status: Status) -> u64 {
    total(poses(status))
}

/// Length of the one-shot `Done` celebration in ticks.
pub fn done_oneshot_len() -> u64 {
    total(DONE_ONESHOT)
}

/// Sprite frame for `status` at 12 fps `tick`; pure and loops seamlessly.
/// `Done` returns the held final "clear" pose.
pub fn sprite(status: Status, size: SpriteSize, tick: u64) -> Vec<Line<'static>> {
    let table = poses(status);
    to_lines(status, size, &pick(table, tick % total(table)))
}

/// The `Done` celebration `tick_since_done` ticks after finishing; after
/// [`done_oneshot_len`] ticks it holds the final pose (= `sprite(Done, ..)`).
pub fn sprite_oneshot_done(size: SpriteSize, tick_since_done: u64) -> Vec<Line<'static>> {
    let p = if tick_since_done < done_oneshot_len() {
        pick(DONE_ONESHOT, tick_since_done)
    } else {
        DONE_HELD
    };
    to_lines(Status::Done, size, &p)
}

/// Shake/flash for the first ~0.3 s (4 ticks) after `from` → `to`.
/// Strongest into `NeedsInput`, a flash-only "clear" into `Done`.
pub fn status_transition_fx(from: Status, to: Status, ticks_since_change: u64) -> TileFx {
    const NEEDS_INPUT_SHAKE: [(i8, i8); 4] = [(2, 0), (-2, 1), (1, 0), (-1, 0)];
    const SOFT_SHAKE: [(i8, i8); 4] = [(1, 0), (-1, 0), (0, 0), (0, 0)];
    if from == to || ticks_since_change >= 4 {
        return TileFx::default();
    }
    let t = ticks_since_change as usize;
    match to {
        Status::NeedsInput => TileFx { shake: NEEDS_INPUT_SHAKE[t], flash: t % 2 == 0 },
        Status::Done => TileFx { shake: (0, 0), flash: t < 2 },
        _ => TileFx { shake: SOFT_SHAKE[t], flash: t == 0 },
    }
}

struct Palette {
    body: Color,
    shade: Color,
    dark: Color,
    accent: Color,
}

fn palette(status: Status) -> Palette {
    let body = style::status_color(status);
    match status {
        Status::Running => Palette {
            body,
            shade: Color::Rgb(170, 80, 24),
            dark: Color::Rgb(44, 26, 18),
            accent: Color::Rgb(80, 176, 255),
        },
        Status::NeedsInput => Palette {
            body,
            shade: Color::Rgb(186, 134, 16),
            dark: Color::Rgb(48, 34, 10),
            accent: Color::Rgb(255, 84, 60),
        },
        Status::Idle => Palette {
            body,
            shade: Color::Rgb(92, 76, 64),
            dark: Color::Rgb(44, 36, 30),
            accent: Color::Rgb(150, 162, 196),
        },
        Status::Done => Palette {
            body,
            shade: Color::Rgb(40, 128, 70),
            dark: Color::Rgb(16, 44, 26),
            accent: Color::Rgb(255, 232, 110),
        },
        Status::Unknown => Palette {
            body,
            shade: Color::Rgb(96, 96, 96),
            dark: Color::Rgb(44, 44, 44),
            accent: Color::Rgb(210, 210, 210),
        },
    }
}

impl Palette {
    fn color(&self, px: Px) -> Option<Color> {
        match px {
            Px::Clear => None,
            Px::Body => Some(self.body),
            Px::Shade => Some(self.shade),
            Px::Dark => Some(self.dark),
            Px::Accent => Some(self.accent),
        }
    }
}

/// Pixel geometry of one sprite size (coordinates in pixels, y down).
struct Geo {
    w: i16,
    h: i16,
    tip: (i16, i16),
    body_top: i16,
    /// Inclusive x span per body row.
    rows: &'static [(i16, i16)],
    /// y of the eye row at dy = 0.
    eye_y: i16,
    feet_y: i16,
    slots: &'static [(i16, i16)],
}

/// 8×8 px: 6-wide body, antenna tip off the top-right corner.
const LARGE: Geo = Geo {
    w: 8,
    h: 8,
    tip: (6, 1),
    body_top: 2,
    rows: &[(2, 5), (1, 6), (1, 6), (1, 6), (2, 5)],
    eye_y: 4,
    feet_y: 7,
    slots: &[(7, 0), (5, 0), (7, 2), (7, 3), (0, 2), (7, 5), (0, 6), (7, 7), (0, 0)],
};

/// 6×6 px: 5-wide body in columns 0..=4; column 5 stays free for glyphs.
const SMALL: Geo = Geo {
    w: 6,
    h: 6,
    tip: (4, 0),
    body_top: 1,
    rows: &[(1, 3), (0, 4), (0, 4), (1, 3)],
    eye_y: 2,
    feet_y: 5,
    // Fewer free pixels: slots 1 and 4 share the top-left corner.
    slots:&[(5, 1), (0, 0), (5, 4), (5, 2), (0, 0), (5, 3), (0, 5), (5, 5), (2, 0)],
};

fn geo(size: SpriteSize) -> &'static Geo {
    match size {
        SpriteSize::Small => &SMALL,
        SpriteSize::Large => &LARGE,
    }
}

/// Eye (and mouth) pixels as (x, offset from eye row).
fn face(size: SpriteSize, eyes: Eyes) -> &'static [(i16, i16)] {
    match (size, eyes) {
        (SpriteSize::Large, Open) => &[(2, 0), (5, 0)],
        (SpriteSize::Large, Glance) => &[(3, 0), (6, 0)],
        (SpriteSize::Large, Alert) => &[(2, -1), (2, 0), (5, -1), (5, 0)],
        (SpriteSize::Large, Closed) => &[(1, 0), (2, 0), (5, 0), (6, 0)],
        (SpriteSize::Large, Happy) => &[(2, -1), (5, -1), (3, 1), (4, 1)],
        (SpriteSize::Large, Odd) => &[(2, 1), (5, 0)],
        (SpriteSize::Small, Open) => &[(1, 0), (3, 0)],
        (SpriteSize::Small, Glance) => &[(2, 0), (4, 0)],
        (SpriteSize::Small, Alert) => &[(1, 0), (1, 1), (3, 0), (3, 1)],
        (SpriteSize::Small, Closed) => &[(0, 1), (1, 1), (3, 1), (4, 1)],
        (SpriteSize::Small, Happy) => &[(1, -1), (3, -1), (2, 1)],
        (SpriteSize::Small, Odd) => &[(1, 1), (3, 0)],
    }
}

fn feet(size: SpriteSize, f: Feet) -> &'static [i16] {
    match (size, f) {
        (SpriteSize::Large, Both) => &[2, 5],
        (SpriteSize::Large, Spread) => &[1, 6],
        (SpriteSize::Large, StepL) => &[3, 5],
        (SpriteSize::Large, StepR) => &[2, 4],
        (SpriteSize::Large, Tucked) => &[3, 4],
        (SpriteSize::Small, Both) => &[1, 3],
        (SpriteSize::Small, Spread) => &[0, 4],
        (SpriteSize::Small, StepL) => &[2, 3],
        (SpriteSize::Small, StepR) => &[1, 2],
        (SpriteSize::Small, Tucked) => &[2],
    }
}

/// Cell (col, row) and text of an overlay glyph, always in the free last
/// column: `z` one row lower than the rest so it "rises" into `Z`.
fn glyph_cell(size: SpriteSize, g: Glyph) -> (usize, usize, &'static str) {
    let (w, _) = size.cells();
    let last = w as usize - 1;
    match g {
        Glyph::Bang => (last, 0, "!"),
        Glyph::ZLow => (last, 1, "z"),
        Glyph::ZHigh => (last, 0, "Z"),
        Glyph::Question => (last, 0, "?"),
        Glyph::Check => (last, 0, "✓"),
    }
}

fn pixels(size: SpriteSize, p: &Pose) -> Vec<Vec<Px>> {
    let g = geo(size);
    let mut grid = vec![vec![Px::Clear; g.w as usize]; g.h as usize];
    let mut put = |x: i16, y: i16, px: Px| {
        if (0..g.w).contains(&x) && (0..g.h).contains(&y) {
            grid[y as usize][x as usize] = px;
        }
    };
    let dy = p.dy;
    put(g.tip.0, g.tip.1 + dy, if p.lit { Px::Accent } else { Px::Shade });
    for (i, (x0, x1)) in g.rows.iter().enumerate() {
        for x in *x0..=*x1 {
            put(x, g.body_top + dy + i as i16, Px::Body);
        }
    }
    for (x, off) in face(size, p.eyes) {
        put(*x, g.eye_y + dy + off, Px::Dark);
    }
    // Grounded feet stay put (a squash hides the body over them);
    // airborne feet hang right under the body.
    let fy = if p.feet == Tucked { g.feet_y + dy } else { g.feet_y };
    for x in feet(size, p.feet) {
        put(*x, fy, Px::Shade);
    }
    for i in p.sparks {
        if let Some((x, y)) = g.slots.get(*i) {
            put(*x, *y, Px::Accent);
        }
    }
    grid
}

fn to_lines(status: Status, size: SpriteSize, p: &Pose) -> Vec<Line<'static>> {
    let pal = palette(status);
    let grid = pixels(size, p);
    let glyph = p.glyph.map(|g| glyph_cell(size, g));
    grid.chunks(2)
        .enumerate()
        .map(|(row, pair)| {
            let spans = (0..pair[0].len())
                .map(|col| {
                    if let Some((gc, gr, text)) = glyph {
                        if gc == col && gr == row {
                            return Span::styled(text, Style::new().fg(pal.accent));
                        }
                    }
                    cell(pal.color(pair[0][col]), pal.color(pair[1][col]))
                })
                .collect::<Vec<_>>();
            Line::from(spans)
        })
        .collect()
}

/// One half-block cell from its top and bottom pixel colors.
fn cell(top: Option<Color>, bottom: Option<Color>) -> Span<'static> {
    match (top, bottom) {
        (None, None) => Span::raw(" "),
        (Some(t), None) => Span::styled("▀", Style::new().fg(t)),
        (None, Some(b)) => Span::styled("▄", Style::new().fg(b)),
        (Some(t), Some(b)) if t == b => Span::styled("█", Style::new().fg(t)),
        (Some(t), Some(b)) => Span::styled("▀", Style::new().fg(t).bg(b)),
    }
}

/// Test-only pixel dump: `.` clear, `o` body, `=` shade, `#` dark, `*` accent.
#[cfg(test)]
pub fn gallery_pixels(status: Status, size: SpriteSize, tick: u64) -> Vec<String> {
    let table = poses(status);
    let p = pick(table, tick % total(table));
    pixels(size, &p)
        .iter()
        .map(|row| {
            row.iter()
                .map(|px| match px {
                    Px::Clear => '.',
                    Px::Body => 'o',
                    Px::Shade => '=',
                    Px::Dark => '#',
                    Px::Accent => '*',
                })
                .collect()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [Status; 5] = [
        Status::Running,
        Status::NeedsInput,
        Status::Idle,
        Status::Done,
        Status::Unknown,
    ];

    fn text(lines: &[Line]) -> String {
        lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn sprite_sizes_are_compact() {
        assert_eq!(SpriteSize::Small.cells(), (6, 3));
        assert_eq!(SpriteSize::Large.cells(), (8, 4));
    }

    #[test]
    fn sprite_has_declared_cell_size() {
        for size in [SpriteSize::Small, SpriteSize::Large] {
            let (w, h) = size.cells();
            for st in ALL {
                for tick in 0..30 {
                    let lines = sprite(st, size, tick);
                    assert_eq!(lines.len(), h as usize);
                    for l in &lines {
                        assert_eq!(l.width(), w as usize, "{st:?} {size:?} {tick}");
                    }
                }
            }
        }
    }

    #[test]
    fn sprite_each_status_differs() {
        for size in [SpriteSize::Small, SpriteSize::Large] {
            let frames: Vec<_> = ALL.iter().map(|s| sprite(*s, size, 0)).collect();
            for i in 0..ALL.len() {
                for j in (i + 1)..ALL.len() {
                    assert_ne!(frames[i], frames[j], "{:?} vs {:?}", ALL[i], ALL[j]);
                }
            }
            // Shapes (not just colors) differ too.
            let shapes: Vec<_> = frames.iter().map(|f| text(f)).collect();
            assert_ne!(shapes[0], shapes[2]); // running vs idle
            assert_ne!(shapes[1], shapes[2]); // input vs idle
        }
    }

    #[test]
    fn sprite_needs_input_frames_differ() {
        let len = loop_len(Status::NeedsInput);
        for size in [SpriteSize::Small, SpriteSize::Large] {
            let mut distinct: Vec<Vec<Line>> = Vec::new();
            for t in 0..len {
                let f = sprite(Status::NeedsInput, size, t);
                if !distinct.contains(&f) {
                    distinct.push(f);
                }
            }
            assert!(distinct.len() >= 3, "{size:?}: {} frames", distinct.len());
            let with_bang = (0..len).filter(|t| text(&sprite(Status::NeedsInput, size, *t)).contains('!'));
            let count = with_bang.count() as u64;
            assert!(count > 0 && count < len, "the ! must blink ({count}/{len})");
        }
    }

    #[test]
    fn sprite_running_loop_has_at_least_four_frames() {
        let len = loop_len(Status::Running);
        let mut distinct: Vec<Vec<Line>> = Vec::new();
        for t in 0..len {
            let f = sprite(Status::Running, SpriteSize::Large, t);
            if !distinct.contains(&f) {
                distinct.push(f);
            }
        }
        assert!(distinct.len() >= 4);
    }

    #[test]
    fn sprite_idle_shows_z_and_unknown_shows_question_mark() {
        let len = loop_len(Status::Idle);
        assert!((0..len).any(|t| text(&sprite(Status::Idle, SpriteSize::Large, t)).contains('z')));
        assert!(text(&sprite(Status::Unknown, SpriteSize::Small, 0)).contains('?'));
        assert!(text(&sprite(Status::Done, SpriteSize::Small, 0)).contains('✓'));
    }

    #[test]
    fn sprite_is_pure_function_of_tick() {
        for st in ALL {
            let len = loop_len(st);
            assert!(len > 0);
            for size in [SpriteSize::Small, SpriteSize::Large] {
                for t in 0..len {
                    assert_eq!(sprite(st, size, t), sprite(st, size, t));
                    assert_eq!(sprite(st, size, t), sprite(st, size, t + len));
                    assert_eq!(sprite(st, size, t), sprite(st, size, t + 1000 * len));
                }
            }
        }
        // Huge ticks don't overflow.
        let _ = sprite(Status::Running, SpriteSize::Large, u64::MAX);
    }

    #[test]
    fn sprite_done_oneshot_holds_last_frame() {
        for size in [SpriteSize::Small, SpriteSize::Large] {
            let held = sprite(Status::Done, size, 0);
            assert_eq!(sprite(Status::Done, size, 7), held);
            assert_ne!(sprite_oneshot_done(size, 0), held, "celebration starts differently");
            let distinct: std::collections::HashSet<String> =
                (0..18).map(|t| format!("{:?}", sprite_oneshot_done(size, t))).collect();
            assert!(distinct.len() >= 3, "a few celebration frames");
            let last = done_oneshot_len() - 1;
            assert_eq!(sprite_oneshot_done(size, last), held, "settles into the held pose (no pop)");
            assert_ne!(sprite_oneshot_done(size, last.saturating_sub(4)), held, "still celebrating shortly before");
            for t in (last..last + 40).chain([100, u64::MAX]) {
                assert_eq!(sprite_oneshot_done(size, t), held, "tick {t}");
            }
        }
    }

    #[test]
    fn transition_fx_decays_to_default_after_about_0_3s() {
        for to in ALL {
            for from in ALL {
                for t in 4..40 {
                    assert_eq!(status_transition_fx(from, to, t), TileFx::default());
                }
            }
        }
        // No change, no effect.
        assert_eq!(status_transition_fx(Status::Idle, Status::Idle, 0), TileFx::default());
        // A change produces an effect right away.
        let first = status_transition_fx(Status::Running, Status::NeedsInput, 0);
        assert_ne!(first, TileFx::default());
        assert!(first.flash);
        assert_ne!(status_transition_fx(Status::NeedsInput, Status::Running, 0), TileFx::default());
        // Strongest into NeedsInput.
        let peak = |to: Status| {
            (0..4)
                .map(|t| {
                    let fx = status_transition_fx(Status::Idle, to, t);
                    fx.shake.0.unsigned_abs() + fx.shake.1.unsigned_abs()
                })
                .max()
                .unwrap()
        };
        for to in [Status::Running, Status::Done, Status::Unknown] {
            assert!(peak(Status::NeedsInput) > peak(to));
        }
        // Deterministic.
        assert_eq!(
            status_transition_fx(Status::Running, Status::NeedsInput, 1),
            status_transition_fx(Status::Running, Status::NeedsInput, 1)
        );
    }

    #[test]
    fn overlay_glyphs_sit_on_empty_cells() {
        for size in [SpriteSize::Small, SpriteSize::Large] {
            for st in ALL {
                let table = poses(st);
                for (_, p) in table.iter().chain(DONE_ONESHOT.iter()) {
                    if let Some(g) = p.glyph {
                        let (col, row, _) = glyph_cell(size, g);
                        let grid = pixels(size, p);
                        assert_eq!(grid[row * 2][col], Px::Clear, "{st:?} {size:?} {g:?}");
                        assert_eq!(grid[row * 2 + 1][col], Px::Clear, "{st:?} {size:?} {g:?}");
                    }
                }
            }
        }
    }
}
