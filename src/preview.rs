//! Read-only preview of the selected session's screen.
//!
//! A dedicated thread (like the branch thread) reads the screen of the one
//! [`PreviewTarget`] the UI published, about every [`PREVIEW_INTERVAL`] and
//! at once when the target changes, and stores the result in
//! [`PreviewCache`]. The UI only reads the cache; the refresh thread never
//! touches it. Nothing is read while the target is `None` (preview off, or
//! the selected session is done / has no ids). The target carries how many
//! screen lines to ask for (the panel's inner height, see
//! [`crate::ui::preview::preview_read_lines`]); a size change is a new
//! target and is read at once. Each read is bounded by the
//! cmux runner's 2 s timeout; a failing or panicking read is recorded as a
//! failure and the thread keeps going. Callers never join it.

use std::collections::HashMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::source::cmux::Cmux;
use crate::ui::tile::truncate_to_width;

/// The selected session is re-read about this often.
pub const PREVIEW_INTERVAL: Duration = Duration::from_secs(1);
/// How often the thread checks for a new target or the stop flag.
const PREVIEW_SLICE: Duration = Duration::from_millis(20);

/// The session whose screen the preview shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewTarget {
    pub session_id: String,
    pub workspace_id: String,
    pub surface_id: String,
    /// Screen lines to read (the panel's inner height, capped).
    pub lines: u16,
}

/// Shared between the UI (writer) and the preview thread (reader).
pub type PreviewSlot = Arc<Mutex<Option<PreviewTarget>>>;

/// One read: the raw screen text, or `Err(())` when the read failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewEntry {
    pub screen: Result<String, ()>,
    pub fetched_at: Instant,
}

/// Session id → last read. Only the latest target's entry is kept.
#[derive(Default)]
pub struct PreviewCache {
    map: Mutex<HashMap<String, PreviewEntry>>,
}

impl PreviewCache {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<String, PreviewEntry>> {
        self.map.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn get(&self, session_id: &str) -> Option<PreviewEntry> {
        self.lock().get(session_id).cloned()
    }

    /// Stores `entry` for `session_id` and forgets every other session.
    pub fn put(&self, session_id: &str, entry: PreviewEntry) {
        let mut map = self.lock();
        map.retain(|id, _| id == session_id);
        map.insert(session_id.to_string(), entry);
    }
}

/// Removes ANSI escape sequences (CSI `ESC [ … final`, OSC `ESC ] … BEL/ST`,
/// other two-byte `ESC x`) and turns the remaining control chars except
/// newlines into spaces (`\r` is dropped).
pub fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        match c {
            '\x1b' => match it.next() {
                // CSI: parameters/intermediates up to a final byte 0x40..=0x7E.
                Some('[') => {
                    for c in it.by_ref() {
                        if ('\x40'..='\x7e').contains(&c) {
                            break;
                        }
                    }
                }
                // OSC (and DCS/APC/PM/SOS): up to BEL or ST (`ESC \`).
                Some(']' | 'P' | '_' | '^' | 'X') => {
                    while let Some(c) = it.next() {
                        if c == '\x07' {
                            break;
                        }
                        if c == '\x1b' {
                            if it.peek() == Some(&'\\') {
                                it.next();
                            }
                            break;
                        }
                    }
                }
                // `ESC <intermediates> <final>` (e.g. `ESC ( B`, `ESC =`).
                Some(c) if ('\x20'..='\x2f').contains(&c) => {
                    while it.next_if(|c| ('\x20'..='\x2f').contains(c)).is_some() {}
                    it.next();
                }
                _ => {}
            },
            '\n' => out.push('\n'),
            '\r' => {}
            c if c.is_control() => out.push(' '),
            c => out.push(c),
        }
    }
    out
}

/// The last lines of `screen` that fit a `height`×`width` panel: escapes
/// stripped, trailing blank lines dropped, each line cut to `width` display
/// columns (wide chars never split, no wrapping).
pub fn preview_lines(screen: &str, height: usize, width: usize) -> Vec<String> {
    let clean = strip_ansi(screen);
    let mut lines: Vec<&str> = clean.lines().map(str::trim_end).collect();
    while lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    let skip = lines.len().saturating_sub(height);
    lines[skip..].iter().map(|l| truncate_to_width(l, width)).collect()
}

/// Starts the preview thread reading through `cmux`. `interval` is the
/// re-read period of an unchanged target.
pub fn spawn_preview(
    cache: Arc<PreviewCache>,
    slot: PreviewSlot,
    cmux: Arc<dyn Cmux + Send + Sync>,
    interval: Duration,
    stop: Arc<AtomicBool>,
) -> JoinHandle<()> {
    std::thread::Builder::new()
        .name("preview".into())
        .spawn(move || {
            // The target read last and when.
            let mut last: Option<(PreviewTarget, Instant)> = None;
            while !stop.load(Ordering::Relaxed) {
                let target = slot.lock().unwrap_or_else(|e| e.into_inner()).clone();
                if let Some(t) = target {
                    let due = match &last {
                        Some((prev, at)) => *prev != t || at.elapsed() >= interval,
                        None => true,
                    };
                    if due {
                        // A panicking read is a failed read; the thread lives on.
                        let screen = catch_unwind(AssertUnwindSafe(|| cmux.read_screen(&t.workspace_id, &t.surface_id, t.lines)))
                            .ok()
                            .and_then(|r| r.ok())
                            .ok_or(());
                        cache.put(&t.session_id, PreviewEntry { screen, fetched_at: Instant::now() });
                        last = Some((t, Instant::now()));
                    }
                } else {
                    last = None;
                }
                std::thread::sleep(PREVIEW_SLICE.min(interval));
            }
        })
        .expect("spawn preview thread")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::cmux::{CmuxSessionRef, FakeCmux};
    use unicode_width::UnicodeWidthStr;

    #[test]
    fn strip_ansi_and_control_chars() {
        assert_eq!(strip_ansi("plain"), "plain");
        assert_eq!(strip_ansi("\x1b[1;31mred\x1b[0m text"), "red text");
        assert_eq!(strip_ansi("a\x1b[2Kb\x1b[?25lc"), "abc");
        assert_eq!(strip_ansi("\x1b]0;title\x07after"), "after");
        assert_eq!(strip_ansi("\x1b]8;;http://x\x1b\\link\x1b]8;;\x1b\\"), "link");
        assert_eq!(strip_ansi("\x1b(Bx\x1b=y"), "xy");
        assert_eq!(strip_ansi("tab\there\r\nnext\x07\x00"), "tab here\nnext  ");
        // a lone or cut-off escape at the end never panics
        assert_eq!(strip_ansi("end\x1b"), "end");
        assert_eq!(strip_ansi("end\x1b[12"), "end");
        assert_eq!(strip_ansi("한글\x1b[33m✳\x1b[0m"), "한글✳");
    }

    #[test]
    fn preview_lines_take_last_non_blank_lines_that_fit() {
        let screen = "l1\nl2\nl3\n  l4 indented\n\n   \n\n";
        assert_eq!(preview_lines(screen, 2, 40), vec!["l3", "  l4 indented"]);
        assert_eq!(preview_lines(screen, 10, 40), vec!["l1", "l2", "l3", "  l4 indented"]);
        // blank lines in the middle are kept
        assert_eq!(preview_lines("a\n\nb\n", 3, 10), vec!["a", "", "b"]);
        assert_eq!(preview_lines("\x1b[32mok\x1b[0m\n", 5, 10), vec!["ok"]);
        assert!(preview_lines("", 5, 10).is_empty());
        assert!(preview_lines("\n \n\t\n", 5, 10).is_empty());
        assert!(preview_lines("a\nb", 0, 10).is_empty());
        // trailing spaces are not shown
        assert_eq!(preview_lines("x   \ny", 5, 10), vec!["x", "y"]);
    }

    #[test]
    fn preview_truncates_wide_chars_without_panic() {
        let screen = "프로젝트 미리보기 테스트\nabcdefghijklmnop\n✳ 작업 중…";
        for w in 0..30 {
            for line in preview_lines(screen, 5, w) {
                assert!(line.width() <= w, "w={w} line={line:?}");
            }
        }
        assert_eq!(preview_lines(screen, 5, 7)[0], "프로젝…");
        assert_eq!(preview_lines(screen, 5, 6)[1], "abcde…");
        assert_eq!(preview_lines("짧음", 1, 4), vec!["짧음"]);
    }

    #[test]
    fn cache_keeps_only_latest_session() {
        let c = PreviewCache::new();
        let e = |s: &str| PreviewEntry { screen: Ok(s.into()), fetched_at: Instant::now() };
        c.put("a", e("A"));
        c.put("b", e("B"));
        assert!(c.get("a").is_none());
        assert_eq!(c.get("b").unwrap().screen, Ok("B".into()));
    }

    fn target(id: &str) -> PreviewTarget {
        PreviewTarget { session_id: id.into(), workspace_id: format!("ws-{id}"), surface_id: format!("sf-{id}"), lines: 20 }
    }

    fn fake_with_screens() -> Arc<FakeCmux> {
        let mut f = FakeCmux::new(Vec::<CmuxSessionRef>::new());
        f.screens.insert("sf-a".into(), Ok("screen of a".into()));
        f.screens.insert("sf-b".into(), Ok("screen of b".into()));
        f.screens.insert("sf-bad".into(), Err("read failed".into()));
        f.tree_result = Ok(Default::default());
        Arc::new(f)
    }

    fn wait_until(limit: Duration, mut cond: impl FnMut() -> bool) -> Option<Duration> {
        let t0 = Instant::now();
        while t0.elapsed() < limit {
            if cond() {
                return Some(t0.elapsed());
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        None
    }

    fn reads_of(f: &FakeCmux, sf: &str) -> usize {
        f.calls().iter().filter(|c| c.ends_with(&format!(" {sf}")) && c.starts_with("read_screen")).count()
    }

    #[test]
    fn preview_thread_updates_cache_for_target_and_switches_immediately() {
        let fake = fake_with_screens();
        let cache = Arc::new(PreviewCache::new());
        let slot: PreviewSlot = Arc::new(Mutex::new(None));
        let stop = Arc::new(AtomicBool::new(false));
        let interval = Duration::from_millis(400);
        let h = spawn_preview(cache.clone(), slot.clone(), fake.clone(), interval, stop.clone());

        std::thread::sleep(Duration::from_millis(100));
        assert!(fake.calls().is_empty(), "no target: no reads");

        *slot.lock().unwrap() = Some(target("a"));
        let got_a = wait_until(Duration::from_secs(2), || cache.get("a").is_some());
        assert!(got_a.is_some(), "a was read");
        assert_eq!(cache.get("a").unwrap().screen, Ok("screen of a".into()));
        assert!(fake.calls().contains(&"read_screen ws-a sf-a".to_string()), "{:?}", fake.calls());
        // re-read about every interval
        assert!(wait_until(Duration::from_secs(2), || reads_of(&fake, "sf-a") >= 2).is_some());

        // switching is immediate, not after the poll interval
        *slot.lock().unwrap() = Some(target("b"));
        let latency = wait_until(Duration::from_secs(2), || cache.get("b").is_some()).expect("b read");
        assert!(latency < interval / 4, "switch took {latency:?}");
        assert_eq!(cache.get("b").unwrap().screen, Ok("screen of b".into()));

        // turning the preview off stops the reads
        *slot.lock().unwrap() = None;
        std::thread::sleep(Duration::from_millis(50));
        let n = fake.calls().len();
        std::thread::sleep(interval * 2);
        assert_eq!(fake.calls().len(), n, "no reads without a target");
        stop.store(true, Ordering::Relaxed);
        h.join().unwrap();
    }

    #[test]
    fn preview_thread_reads_the_target_line_count_and_rereads_on_resize() {
        let fake = fake_with_screens();
        let cache = Arc::new(PreviewCache::new());
        let slot: PreviewSlot = Arc::new(Mutex::new(Some(PreviewTarget { lines: 28, ..target("a") })));
        let stop = Arc::new(AtomicBool::new(false));
        let h = spawn_preview(cache.clone(), slot.clone(), fake.clone(), Duration::from_secs(5), stop.clone());
        assert!(wait_until(Duration::from_secs(2), || cache.get("a").is_some()).is_some());
        assert_eq!(*fake.read_lines.lock().unwrap(), vec![28]);
        // the panel grew: same session, new line count, read at once (not after 5 s)
        *slot.lock().unwrap() = Some(PreviewTarget { lines: 80, ..target("a") });
        let t = wait_until(Duration::from_secs(2), || fake.read_lines.lock().unwrap().len() >= 2).expect("re-read");
        assert!(t < Duration::from_millis(300), "{t:?}");
        assert_eq!(fake.read_lines.lock().unwrap()[1], 80);
        stop.store(true, Ordering::Relaxed);
        h.join().unwrap();
    }

    #[test]
    fn preview_thread_stop_ends_promptly() {
        let fake = fake_with_screens();
        let slot: PreviewSlot = Arc::new(Mutex::new(Some(target("a"))));
        let stop = Arc::new(AtomicBool::new(false));
        let cache = Arc::new(PreviewCache::new());
        let h = spawn_preview(cache.clone(), slot, fake, Duration::from_secs(5), stop.clone());
        assert!(wait_until(Duration::from_secs(2), || cache.get("a").is_some()).is_some());
        let t0 = Instant::now();
        stop.store(true, Ordering::Relaxed);
        h.join().unwrap();
        assert!(t0.elapsed() < Duration::from_millis(200), "{:?}", t0.elapsed());
    }

    /// Panics for surface `sf-boom`, otherwise answers like the fake.
    struct PanickyCmux(Arc<FakeCmux>);

    impl Cmux for PanickyCmux {
        fn sessions(&self) -> anyhow::Result<Vec<CmuxSessionRef>> {
            self.0.sessions()
        }
        fn tree(&self) -> anyhow::Result<crate::source::cmux::TreeLayout> {
            self.0.tree()
        }
        fn focus(&self, w: &str, s: &str) -> anyhow::Result<()> {
            self.0.focus(w, s)
        }
        fn read_screen(&self, w: &str, s: &str, lines: u16) -> anyhow::Result<String> {
            if s == "sf-boom" {
                panic!("read exploded");
            }
            self.0.read_screen(w, s, lines)
        }
    }

    #[test]
    fn preview_thread_survives_read_failure_and_panic() {
        let fake = fake_with_screens();
        let cache = Arc::new(PreviewCache::new());
        let slot: PreviewSlot = Arc::new(Mutex::new(Some(target("bad"))));
        let stop = Arc::new(AtomicBool::new(false));
        let cmux = Arc::new(PanickyCmux(fake));
        let h = spawn_preview(cache.clone(), slot.clone(), cmux, Duration::from_millis(100), stop.clone());
        assert!(wait_until(Duration::from_secs(2), || cache.get("bad").is_some()).is_some());
        assert_eq!(cache.get("bad").unwrap().screen, Err(()));
        *slot.lock().unwrap() = Some(target("boom"));
        assert!(wait_until(Duration::from_secs(2), || cache.get("boom").is_some()).is_some());
        assert_eq!(cache.get("boom").unwrap().screen, Err(()), "a panic counts as a failed read");
        *slot.lock().unwrap() = Some(target("a"));
        assert!(
            wait_until(Duration::from_secs(2), || cache.get("a").is_some()).is_some(),
            "thread still alive after the panic"
        );
        assert_eq!(cache.get("a").unwrap().screen, Ok("screen of a".into()));
        stop.store(true, Ordering::Relaxed);
        h.join().unwrap();
    }
}
