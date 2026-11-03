//! Background branch resolution.
//!
//! Resolving a session's branch can be slow: `git worktree list`, and when the
//! session sits in the main checkout with guessing on, a `pgrep`/`lsof` walk of
//! its child processes (up to 4 s) plus a transcript scan. None of that may
//! delay the 1 s status refresh, so it runs on its own thread (like the PR
//! poller). The refresh round only READS [`BranchCache`] and publishes the
//! current [`BranchTarget`]s; this thread re-resolves each session at most
//! every [`BRANCH_TTL`] (at once when it was never resolved or its cwd
//! changed), skips `Done` sessions, isolates failures and panics per session
//! and stops when its flag is set (callers never join it).

use std::collections::HashMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::config::Config;

/// A session is re-resolved at most this often.
pub const BRANCH_TTL: Duration = Duration::from_secs(30);
/// How often the thread looks for due targets.
const SCAN_INTERVAL: Duration = Duration::from_millis(500);
const STOP_SLICE: Duration = Duration::from_millis(50);

/// What the branch thread needs to know about one session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchTarget {
    pub id: String,
    pub cwd: PathBuf,
    pub pid: u32,
    /// `transcriptPath` from the hook file, if known.
    pub transcript: Option<PathBuf>,
    /// Finished sessions keep their last branch but are not re-resolved.
    pub done: bool,
}

/// A resolved branch and when/for which cwd it was resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchEntry {
    pub cwd: PathBuf,
    pub branch: Option<String>,
    pub guessed: bool,
    pub resolved_at: Instant,
}

/// Session id → last resolution. Reads take a short lock and never block on
/// a resolution in progress.
#[derive(Default)]
pub struct BranchCache {
    map: Mutex<HashMap<String, BranchEntry>>,
}

/// Targets shared between the refresh thread (writer) and the branch thread.
pub type BranchTargets = Arc<Mutex<Vec<BranchTarget>>>;

/// Resolves (branch, guessed) for a target; `bool` = guessing allowed.
pub type Resolver = Arc<dyn Fn(&BranchTarget, bool) -> (Option<String>, bool) + Send + Sync>;

impl BranchCache {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<String, BranchEntry>> {
        self.map.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The cached (branch, guessed) of `id`, only if it was resolved for `cwd`.
    pub fn get(&self, id: &str, cwd: &Path) -> Option<(Option<String>, bool)> {
        let map = self.lock();
        let e = map.get(id).filter(|e| e.cwd == cwd)?;
        Some((e.branch.clone(), e.guessed))
    }

    pub fn entry(&self, id: &str) -> Option<BranchEntry> {
        self.lock().get(id).cloned()
    }

    pub fn put(&self, id: &str, entry: BranchEntry) {
        self.lock().insert(id.to_string(), entry);
    }

    fn retain_ids(&self, targets: &[BranchTarget]) {
        self.lock().retain(|id, _| targets.iter().any(|t| &t.id == id));
    }
}

/// Whether `t` must be (re-)resolved at `now`.
pub fn is_due(entry: Option<&BranchEntry>, t: &BranchTarget, now: Instant, ttl: Duration) -> bool {
    if t.done {
        return false;
    }
    match entry {
        None => true,
        Some(e) => e.cwd != t.cwd || now.saturating_duration_since(e.resolved_at) >= ttl,
    }
}

/// One pass over `targets`: resolves the due ones (never-resolved and
/// changed-cwd first, then stale), then forgets sessions that are gone.
pub fn resolve_due(
    cache: &BranchCache,
    targets: &[BranchTarget],
    resolver: &Resolver,
    guess: bool,
    now: &dyn Fn() -> Instant,
    ttl: Duration,
    stop: &AtomicBool,
) {
    let at = now();
    // (urgent, target): never resolved / moved first, then merely stale.
    let mut due: Vec<(bool, &BranchTarget)> = Vec::new();
    for t in targets {
        let e = cache.entry(&t.id);
        if is_due(e.as_ref(), t, at, ttl) {
            let urgent = e.as_ref().is_none_or(|e| e.cwd != t.cwd);
            due.push((urgent, t));
        }
    }
    due.sort_by_key(|(urgent, _)| !*urgent);
    for (_, t) in due {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        // The cache lock is not held while the resolver runs.
        let (branch, guessed) = match catch_unwind(AssertUnwindSafe(|| resolver(t, guess))) {
            Ok(r) => r,
            // Recorded without a branch so it is retried after the ttl only.
            Err(_) => (None, false),
        };
        cache.put(&t.id, BranchEntry { cwd: t.cwd.clone(), branch, guessed, resolved_at: now() });
    }
    cache.retain_ids(targets);
}

/// Starts the branch thread. `config` is read every pass: nothing is resolved
/// while both the branch and PR extras are off, and `guess_worktree` applies
/// without a restart.
pub fn spawn_branch_resolver(
    cache: Arc<BranchCache>,
    targets: BranchTargets,
    config: Arc<Mutex<Config>>,
    resolver: Resolver,
    stop: Arc<AtomicBool>,
) -> JoinHandle<()> {
    std::thread::Builder::new()
        .name("branch".into())
        .spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                let cfg = config.lock().unwrap_or_else(|e| e.into_inner()).clone();
                if cfg.extras.branch || cfg.extras.pr {
                    let list = targets.lock().unwrap_or_else(|e| e.into_inner()).clone();
                    let _ = catch_unwind(AssertUnwindSafe(|| {
                        resolve_due(&cache, &list, &resolver, cfg.guess_worktree, &Instant::now, BRANCH_TTL, &stop)
                    }));
                }
                let next = Instant::now() + SCAN_INTERVAL;
                while Instant::now() < next && !stop.load(Ordering::Relaxed) {
                    std::thread::sleep(STOP_SLICE.min(next.saturating_duration_since(Instant::now())));
                }
            }
        })
        .expect("spawn branch thread")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    fn target(id: &str, cwd: &str) -> BranchTarget {
        BranchTarget { id: id.into(), cwd: PathBuf::from(cwd), pid: 1, transcript: None, done: false }
    }

    fn entry(cwd: &str, branch: &str, at: Instant) -> BranchEntry {
        BranchEntry { cwd: PathBuf::from(cwd), branch: Some(branch.into()), guessed: false, resolved_at: at }
    }

    fn named_resolver(calls: Arc<Mutex<Vec<(String, bool)>>>) -> Resolver {
        Arc::new(move |t: &BranchTarget, guess: bool| {
            calls.lock().unwrap().push((t.id.clone(), guess));
            if t.id == "boom" {
                panic!("resolver bug");
            }
            (Some(format!("br-{}", t.id)), guess)
        })
    }

    fn wait_until(mut cond: impl FnMut() -> bool) -> bool {
        let end = Instant::now() + Duration::from_secs(3);
        while Instant::now() < end {
            if cond() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        false
    }

    #[test]
    fn is_due_hit_miss_ttl_cwd_change_and_done() {
        let t0 = Instant::now();
        let t = target("a", "/r");
        let ttl = Duration::from_secs(30);
        assert!(is_due(None, &t, t0, ttl), "never resolved");
        let e = entry("/r", "main", t0);
        assert!(!is_due(Some(&e), &t, t0 + Duration::from_secs(29), ttl), "fresh hit");
        assert!(is_due(Some(&e), &t, t0 + ttl, ttl), "stale after ttl");
        assert!(is_due(Some(&e), &target("a", "/other"), t0, ttl), "cwd changed");
        let done = BranchTarget { done: true, ..t.clone() };
        assert!(!is_due(None, &done, t0, ttl), "done sessions are skipped");
        assert!(!is_due(Some(&e), &done, t0 + ttl * 10, ttl));
    }

    #[test]
    fn cache_get_only_for_matching_cwd() {
        let c = BranchCache::new();
        assert_eq!(c.get("a", Path::new("/r")), None);
        c.put("a", entry("/r", "main", Instant::now()));
        assert_eq!(c.get("a", Path::new("/r")), Some((Some("main".into()), false)));
        assert_eq!(c.get("a", Path::new("/elsewhere")), None);
    }

    #[test]
    fn resolve_due_uses_injected_clock_for_ttl() {
        let cache = BranchCache::new();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let r = named_resolver(calls.clone());
        let stop = AtomicBool::new(false);
        let t0 = Instant::now();
        let clock = Mutex::new(t0);
        let now = || *clock.lock().unwrap();
        let ts = vec![target("a", "/r"), target("b", "/r")];
        resolve_due(&cache, &ts, &r, true, &now, BRANCH_TTL, &stop);
        assert_eq!(calls.lock().unwrap().len(), 2, "both missing: resolved");
        assert_eq!(cache.get("a", Path::new("/r")), Some((Some("br-a".into()), true)));
        resolve_due(&cache, &ts, &r, true, &now, BRANCH_TTL, &stop);
        assert_eq!(calls.lock().unwrap().len(), 2, "fresh: cache hit, no work");
        *clock.lock().unwrap() = t0 + BRANCH_TTL;
        resolve_due(&cache, &ts, &r, false, &now, BRANCH_TTL, &stop);
        assert_eq!(calls.lock().unwrap().len(), 4, "stale: resolved again");
        assert_eq!(calls.lock().unwrap()[3].1, false, "guess flag passed through");
        // cwd change: resolved at once, even before the ttl
        let moved = vec![target("a", "/wt"), target("b", "/r")];
        resolve_due(&cache, &moved, &r, true, &now, BRANCH_TTL, &stop);
        assert_eq!(calls.lock().unwrap().len(), 5);
        assert_eq!(cache.entry("a").unwrap().cwd, PathBuf::from("/wt"));
        // gone sessions are forgotten
        resolve_due(&cache, &[target("b", "/r")], &r, true, &now, BRANCH_TTL, &stop);
        assert!(cache.entry("a").is_none());
    }

    #[test]
    fn resolve_due_skips_done_sessions_but_keeps_their_branch() {
        let cache = BranchCache::new();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let r = named_resolver(calls.clone());
        let stop = AtomicBool::new(false);
        let t0 = Instant::now();
        cache.put("d", entry("/r", "old", t0));
        let mut d = target("d", "/r");
        d.done = true;
        let later = move || t0 + BRANCH_TTL * 3;
        resolve_due(&cache, &[d, target("never", "/r")], &r, true, &later, BRANCH_TTL, &stop);
        let ids: Vec<String> = calls.lock().unwrap().iter().map(|(i, _)| i.clone()).collect();
        assert_eq!(ids, vec!["never".to_string()]);
        assert_eq!(cache.get("d", Path::new("/r")), Some((Some("old".into()), false)));
    }

    #[test]
    fn panic_in_one_session_is_isolated() {
        let cache = BranchCache::new();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let r = named_resolver(calls.clone());
        let stop = AtomicBool::new(false);
        let ts = vec![target("boom", "/r"), target("ok", "/r")];
        resolve_due(&cache, &ts, &r, true, &Instant::now, BRANCH_TTL, &stop);
        assert_eq!(cache.get("ok", Path::new("/r")), Some((Some("br-ok".into()), true)));
        // the panicking one is recorded (no retry storm) without a branch
        assert_eq!(cache.get("boom", Path::new("/r")), Some((None, false)));
        resolve_due(&cache, &ts, &r, true, &Instant::now, BRANCH_TTL, &stop);
        assert_eq!(calls.lock().unwrap().len(), 2, "not retried before the ttl");
    }

    fn spawn_with(resolver: Resolver, cfg: Config) -> (Arc<BranchCache>, BranchTargets, Arc<Mutex<Config>>, Arc<AtomicBool>, JoinHandle<()>) {
        let cache = Arc::new(BranchCache::new());
        let targets: BranchTargets = Arc::new(Mutex::new(Vec::new()));
        let config = Arc::new(Mutex::new(cfg));
        let stop = Arc::new(AtomicBool::new(false));
        let h = spawn_branch_resolver(cache.clone(), targets.clone(), config.clone(), resolver, stop.clone());
        (cache, targets, config, stop, h)
    }

    #[test]
    fn thread_survives_a_panicking_session_and_picks_up_new_targets() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let (cache, targets, _cfg, stop, h) = spawn_with(named_resolver(calls), Config::default());
        *targets.lock().unwrap() = vec![target("boom", "/r")];
        assert!(wait_until(|| cache.entry("boom").is_some()));
        targets.lock().unwrap().push(target("later", "/r"));
        assert!(wait_until(|| cache.get("later", Path::new("/r")).is_some()), "thread still alive");
        stop.store(true, Ordering::Relaxed);
        h.join().unwrap();
    }

    #[test]
    fn thread_reads_guess_flag_and_extras_from_shared_config() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut cfg = Config::default();
        cfg.extras.branch = false;
        cfg.extras.pr = false;
        let (cache, targets, config, stop, h) = spawn_with(named_resolver(calls.clone()), cfg);
        *targets.lock().unwrap() = vec![target("a", "/r")];
        std::thread::sleep(SCAN_INTERVAL * 2);
        assert!(calls.lock().unwrap().is_empty(), "branch and PR extras off: nothing resolved");
        {
            let mut c = config.lock().unwrap();
            c.extras.branch = true;
            c.guess_worktree = false;
        }
        assert!(wait_until(|| cache.get("a", Path::new("/r")).is_some()));
        assert_eq!(calls.lock().unwrap()[0], ("a".to_string(), false));
        stop.store(true, Ordering::Relaxed);
        h.join().unwrap();
    }

    #[test]
    fn stop_flag_ends_thread_promptly() {
        let n = Arc::new(AtomicUsize::new(0));
        let n2 = n.clone();
        let slow: Resolver = Arc::new(move |_: &BranchTarget, _: bool| {
            n2.fetch_add(1, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(100));
            (None, false)
        });
        let (_cache, targets, _cfg, stop, h) = spawn_with(slow, Config::default());
        *targets.lock().unwrap() = (0..50).map(|i| target(&format!("s{i}"), "/r")).collect();
        assert!(wait_until(|| n.load(Ordering::SeqCst) > 0));
        let t0 = Instant::now();
        stop.store(true, Ordering::Relaxed);
        h.join().unwrap();
        assert!(t0.elapsed() < Duration::from_millis(500), "{:?}", t0.elapsed());
        assert!(n.load(Ordering::SeqCst) < 50, "stopped between sessions");
    }
}
