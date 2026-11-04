//! Background data refresh: reads the session sources, merges them and fills
//! in screen-derived status plus the cached branch/PR details. The UI thread
//! only receives finished [`Snapshot`]s over a channel. Branches and PRs are
//! resolved on their own threads ([`crate::branch`], [`crate::pr`]); a round
//! only reads their caches, so it never waits on git, pgrep, lsof or az/gh.

use std::collections::HashMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime};

use crate::branch::{BranchCache, BranchTarget, BranchTargets};
use crate::config::Config;
use crate::merge::{merge_sessions, select_refs};
use crate::model::{Session, Status};
use crate::pr::{pr_note, PrCache, PrLookup};
use crate::source::claude_sessions::read_claude_sessions;
use crate::source::context::{context_percent, default_ctx_dir, safe_id, TranscriptCache};
use crate::source::cmux::{Cmux, CmuxCli, CmuxSessionRef, SCREEN_LINES};
use crate::source::hook_sessions::read_hook_sessions;
use crate::status::resolve_status;

/// How often the refresh thread rebuilds the snapshot.
pub const REFRESH_INTERVAL: Duration = Duration::from_secs(1);
/// At most this many `read-screen` calls per refresh (each may take up to the cmux timeout).
pub const MAX_SCREEN_READS: usize = 3;
/// Footer text when `cmux sessions list` fails.
pub const CMUX_UNAVAILABLE: &str = "cmux unavailable";

const STOP_SLICE: Duration = Duration::from_millis(50);

/// (repo dir, branch) pairs the PR poller should look up.
pub type PrTargets = Arc<Mutex<Vec<(PathBuf, String)>>>;

/// One refresh result. Besides the sessions it carries what the next refresh
/// needs: the last good cmux list and the transcript usage cache.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub sessions: Vec<Session>,
    /// Short source-failure note for the footer (`None` when all is well).
    pub message: Option<String>,
    /// PR availability note for the footer while PR info is on
    /// (e.g. "PR: az 로그인 필요"); see [`crate::pr::pr_note`].
    pub pr_note: Option<String>,
    /// cmux windows (uppercase ids) in tree order; empty when the tree is unknown.
    pub window_order: Vec<String>,
    /// The window holding the monitor's own surface, if known.
    pub current_window: Option<String>,
    pub(crate) cmux_refs: Vec<CmuxSessionRef>,
    pub(crate) seq: u64,
    /// Context usage read from transcripts, reused while a file is unchanged.
    pub(crate) transcripts: TranscriptCache,
}

impl Snapshot {
    /// Distinct (session cwd, branch) pairs of sessions that have a branch.
    pub fn pr_targets(&self) -> Vec<(PathBuf, String)> {
        let mut out: Vec<(PathBuf, String)> = Vec::new();
        for s in &self.sessions {
            if let Some(b) = &s.branch {
                let key = (s.cwd.clone(), b.clone());
                if !out.contains(&key) {
                    out.push(key);
                }
            }
        }
        out
    }

    /// What the branch thread should resolve (or keep) for these sessions.
    pub fn branch_targets(&self) -> Vec<BranchTarget> {
        self.sessions
            .iter()
            .map(|s| BranchTarget {
                id: s.id.clone(),
                cwd: s.cwd.clone(),
                pid: s.pid,
                transcript: s.transcript.clone(),
                done: s.status == Status::Done,
            })
            .collect()
    }
}

/// Inputs of [`build_snapshot`]; tests swap in temp dirs, `FakeCmux` and a
/// pre-filled branch cache.
pub struct RefreshDeps<'a> {
    pub claude_dir: PathBuf,
    pub hook_path: PathBuf,
    /// Statusline context files (`<session_id>.json`).
    pub ctx_dir: PathBuf,
    /// Claude transcripts (`<encoded cwd>/<session_id>.jsonl`).
    pub projects_dir: PathBuf,
    pub cmux: &'a dyn Cmux,
    pub config: Config,
    pub pr_cache: Option<&'a PrCache>,
    /// Filled by the branch thread; only read here (never blocks).
    pub branches: Option<&'a BranchCache>,
    pub now: SystemTime,
    /// The monitor's own surface (`$CMUX_SURFACE_ID`), to find its window.
    pub own_surface: Option<String>,
}

pub fn default_claude_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_default().join(".claude").join("sessions")
}

pub fn default_projects_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_default().join(".claude").join("projects")
}

/// The session's transcript: the hook's `transcriptPath`, else
/// `<projects_dir>/<cwd with every non-alphanumeric char as '-'>/<id>.jsonl`
/// (Claude Code's layout; no directory scan, so it is cheap every round).
pub(crate) fn transcript_for(s: &Session, projects_dir: &Path) -> Option<PathBuf> {
    if let Some(t) = &s.transcript {
        return Some(t.clone());
    }
    let cwd = s.cwd.to_string_lossy();
    if cwd.is_empty() || !safe_id(&s.id) {
        return None;
    }
    let encoded: String = cwd.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    Some(projects_dir.join(encoded).join(format!("{}.jsonl", s.id)))
}

pub fn default_hook_path() -> PathBuf {
    dirs::home_dir().unwrap_or_default().join(".cmuxterm").join("claude-hook-sessions.json")
}

/// Builds the next snapshot from the sources and the previous one. A failing
/// cmux keeps the previous cmux list and sets [`Snapshot::message`]. Sessions
/// whose cmux tab no longer exists (per `cmux tree`) are dropped at once.
pub fn build_snapshot(deps: &RefreshDeps, prev: &Snapshot) -> Snapshot {
    let claude = read_claude_sessions(&deps.claude_dir);
    let hooks = read_hook_sessions(&deps.hook_path);
    let (cmux_refs, message) = match deps.cmux.sessions() {
        Ok(refs) => (refs, None),
        Err(_) => (prev.cmux_refs.clone(), Some(CMUX_UNAVAILABLE.to_string())),
    };
    // The list keeps closed tabs (stale `active_surface=yes`); the tree is
    // the only live view. A failing tree is "unknown": nothing is dropped.
    let tree = deps.cmux.tree().ok();
    let retention = Duration::from_secs(deps.config.done_retention_minutes.saturating_mul(60));
    let mut sessions =
        merge_sessions(&claude, &hooks, &cmux_refs, deps.now, retention, &prev.sessions, tree.as_ref());
    // Window of every tab, from the same tree call. Unknown tree = no window
    // info at all (one ungrouped list), never a stale mix from `prev`.
    for s in sessions.iter_mut() {
        s.window_id = tree.as_ref().and_then(|t| t.window_of(&s.surface_id)).map(String::from);
    }
    let window_order = tree.as_ref().map(|t| t.window_order.clone()).unwrap_or_default();
    let current_window = tree
        .as_ref()
        .zip(deps.own_surface.as_deref())
        .and_then(|(t, own)| t.window_of(own))
        .map(String::from);

    // Last-resort screen rule for live sessions the files could not classify.
    let alive: HashMap<&str, bool> =
        select_refs(&cmux_refs).into_iter().map(|r| (r.session_id.as_str(), r.pid_exists)).collect();
    let mut reads = 0;
    for s in sessions.iter_mut() {
        if s.status != Status::Unknown || !alive.get(s.id.as_str()).copied().unwrap_or(false) {
            continue;
        }
        if reads >= MAX_SCREEN_READS {
            break;
        }
        reads += 1;
        if let Ok(screen) = deps.cmux.read_screen(&s.workspace_id, &s.surface_id, SCREEN_LINES) {
            let h = hooks.iter().find(|h| h.session_id == s.id);
            let c = claude.iter().find(|c| c.session_id == s.id);
            s.status = resolve_status(h, c, true, Some(&screen));
        }
    }

    let seq = prev.seq.wrapping_add(1);
    let extras = &deps.config.extras;
    if extras.branch || extras.pr {
        if let Some(cache) = deps.branches {
            for s in sessions.iter_mut() {
                if let Some((branch, guessed)) = cache.get(&s.id, &s.cwd) {
                    (s.branch, s.branch_guessed) = (branch, guessed);
                }
            }
        }
    }
    let mut note = None;
    if extras.pr {
        if let Some(cache) = deps.pr_cache {
            let (mut reasons, mut pending) = (Vec::new(), false);
            for s in sessions.iter_mut() {
                let Some(b) = &s.branch else { continue };
                match cache.get(&s.cwd, b) {
                    PrLookup::Found(pr) => s.pr = Some(pr),
                    PrLookup::None => {}
                    PrLookup::Unknown => pending = true,
                    PrLookup::Unavailable(reason) => {
                        s.pr_unavailable = true;
                        reasons.push(reason);
                    }
                }
            }
            note = pr_note(&reasons, pending);
        }
    }
    // Context usage: a small file per session, else a cached transcript tail.
    let mut transcripts = prev.transcripts.clone();
    let max_age = Duration::from_secs(deps.config.ctx_fresh_seconds);
    let mut used = Vec::new();
    for s in sessions.iter_mut() {
        let t = transcript_for(s, &deps.projects_dir);
        s.ctx_percent = context_percent(
            &deps.ctx_dir,
            &s.id,
            t.as_deref(),
            deps.config.context_limit_tokens,
            max_age,
            deps.now,
            &mut transcripts,
        );
        used.extend(t);
    }
    transcripts.retain(&used);
    Snapshot { sessions, message, pr_note: note, window_order, current_window, cmux_refs, seq, transcripts }
}

/// Starts the refresh thread: a snapshot every [`REFRESH_INTERVAL`] (first one
/// immediately) until `stop` is set or the receiver is gone. `config` is read
/// on every round so settings changes apply without a restart. A panic in one
/// round is caught and the previous data is resent with a footer note. After
/// each round the PR and branch targets are published to their threads.
pub fn spawn_refresh(
    tx: Sender<Snapshot>,
    config: Arc<Mutex<Config>>,
    pr: Option<(Arc<PrCache>, PrTargets)>,
    branches: Option<(Arc<BranchCache>, BranchTargets)>,
    stop: Arc<AtomicBool>,
) -> JoinHandle<()> {
    std::thread::Builder::new()
        .name("refresh".into())
        .spawn(move || {
            let cmux = CmuxCli::new();
            let own_surface = std::env::var("CMUX_SURFACE_ID").ok().filter(|v| !v.is_empty());
            let round = |_first: bool, prev: &Snapshot| {
                let cfg = config.lock().unwrap_or_else(|e| e.into_inner()).clone();
                let deps = RefreshDeps {
                    claude_dir: default_claude_dir(),
                    hook_path: default_hook_path(),
                    ctx_dir: default_ctx_dir(),
                    projects_dir: default_projects_dir(),
                    cmux: &cmux,
                    config: cfg,
                    pr_cache: pr.as_ref().map(|(c, _)| c.as_ref()),
                    branches: branches.as_ref().map(|(c, _)| c.as_ref()),
                    now: SystemTime::now(),
                    own_surface: own_surface.clone(),
                };
                build_snapshot(&deps, prev)
            };
            let send = |snap: &Snapshot| {
                if let Some((_, targets)) = &pr {
                    *targets.lock().unwrap_or_else(|e| e.into_inner()) = snap.pr_targets();
                }
                if let Some((_, targets)) = &branches {
                    *targets.lock().unwrap_or_else(|e| e.into_inner()) = snap.branch_targets();
                }
                tx.send(snap.clone()).is_ok()
            };
            run_rounds(round, send, REFRESH_INTERVAL, &stop);
        })
        .expect("spawn refresh thread")
}

/// The refresh loop: `round(first, prev)` builds a snapshot, `send` delivers
/// it (false = receiver gone, stop). The first round is followed at once by
/// a second one; every later round waits `interval`. A panicking round yields
/// the previous data with a footer note and is paced like any other round.
/// (Production rounds no longer treat the first round specially: branches are
/// only read from the branch cache, so no round is slow.)
pub(crate) fn run_rounds(
    mut round: impl FnMut(bool, &Snapshot) -> Snapshot,
    mut send: impl FnMut(&Snapshot) -> bool,
    interval: Duration,
    stop: &AtomicBool,
) {
    let mut prev = Snapshot::default();
    let mut first = true;
    while !stop.load(Ordering::Relaxed) {
        let is_first = std::mem::replace(&mut first, false);
        let snap = catch_unwind(AssertUnwindSafe(|| round(is_first, &prev))).unwrap_or_else(|_| {
            let mut p = prev.clone();
            p.message = Some("refresh failed".into());
            p.seq = p.seq.wrapping_add(1);
            p
        });
        if !send(&snap) {
            return;
        }
        prev = snap;
        // One-shot: only the very first round skips the wait.
        if is_first {
            continue;
        }
        let next = Instant::now() + interval;
        while Instant::now() < next && !stop.load(Ordering::Relaxed) {
            std::thread::sleep(STOP_SLICE.min(next.saturating_duration_since(Instant::now())));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::branch::BranchEntry;
    use crate::source::cmux::FakeCmux;
    use std::path::Path;

    fn cref(id: &str, alive: bool) -> CmuxSessionRef {
        CmuxSessionRef {
            session_id: id.into(),
            workspace_id: format!("ws-{id}"),
            surface_id: format!("sf-{id}"),
            cwd: PathBuf::from("/home/user/proj"),
            pid_exists: alive,
            updated: None,
        }
    }

    fn write_claude(dir: &Path, pid: u32, id: &str, status: &str) {
        let json = format!(
            r#"{{"pid":{pid},"sessionId":"{id}","cwd":"/home/user/{id}","name":"{id}","status":"{status}","startedAt":{}}}"#,
            1_700_000_000_000u64 + u64::from(pid)
        );
        std::fs::write(dir.join(format!("{pid}.json")), json).unwrap();
    }

    fn write_hooks(path: &Path, entries: &[(&str, &str)]) {
        let body: Vec<String> = entries
            .iter()
            .map(|(id, lifecycle)| {
                format!(
                    r#""{id}":{{"sessionId":"{id}","pid":1,"workspaceId":"ws-{id}","surfaceId":"sf-{id}","cwd":"/home/user/{id}","agentLifecycle":"{lifecycle}"}}"#
                )
            })
            .collect();
        std::fs::write(path, format!(r#"{{"sessions":{{{}}}}}"#, body.join(","))).unwrap();
    }

    fn deps<'a>(dir: &Path, cmux: &'a dyn Cmux) -> RefreshDeps<'a> {
        RefreshDeps {
            claude_dir: dir.join("sessions"),
            hook_path: dir.join("hooks.json"),
            ctx_dir: dir.join("ctx"),
            projects_dir: dir.join("projects"),
            cmux,
            config: Config::default(),
            pr_cache: None,
            branches: None,
            now: SystemTime::now(),
            own_surface: None,
        }
    }

    fn setup() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("sessions")).unwrap();
        dir
    }

    fn tree_of(surfaces: &[(&str, &str)]) -> crate::source::cmux::TreeLayout {
        let mut t = crate::source::cmux::TreeLayout::default();
        for (sf, w) in surfaces {
            t.surfaces.insert(sf.to_string(), w.to_string());
            if !w.is_empty() && !t.window_order.contains(&w.to_string()) {
                t.window_order.push(w.to_string());
            }
        }
        t
    }

    #[test]
    fn build_snapshot_fills_window_ids_from_tree_and_clears_them_when_unknown() {
        let dir = setup();
        let sd = dir.path().join("sessions");
        write_claude(&sd, 11, "a", "busy");
        write_claude(&sd, 12, "b", "busy");
        let mut cmux = FakeCmux::new(vec![cref("a", true), cref("b", true)]);
        cmux.tree_result = Ok(tree_of(&[("SF-A", "W2"), ("SF-B", "W1"), ("SF-ME", "W1")]));
        let mut d = deps(dir.path(), &cmux);
        d.own_surface = Some("sf-me".into());
        let snap = build_snapshot(&d, &Snapshot::default());
        let win = |s: &Snapshot, id: &str| s.sessions.iter().find(|x| x.id == id).unwrap().window_id.clone();
        assert_eq!(win(&snap, "a").as_deref(), Some("W2"));
        assert_eq!(win(&snap, "b").as_deref(), Some("W1"));
        assert_eq!(snap.window_order, vec!["W2".to_string(), "W1".to_string()]);
        assert_eq!(snap.current_window.as_deref(), Some("W1"));
        // tree fails: no window info, no stale mix from the previous snapshot
        cmux.tree_result = Err("down".into());
        let d = deps(dir.path(), &cmux);
        let next = build_snapshot(&d, &snap);
        assert!(next.sessions.iter().all(|s| s.window_id.is_none()));
        assert!(next.window_order.is_empty() && next.current_window.is_none());
    }

    fn ids(s: &Snapshot) -> Vec<&str> {
        s.sessions.iter().map(|s| s.id.as_str()).collect()
    }

    fn count_rounds(mut round: impl FnMut(bool, &Snapshot) -> Snapshot, run_for: Duration) -> usize {
        let stop = AtomicBool::new(false);
        let sent = std::cell::Cell::new(0usize);
        let start = Instant::now();
        run_rounds(
            |first, prev| round(first, prev),
            |_| {
                sent.set(sent.get() + 1);
                if start.elapsed() >= run_for {
                    stop.store(true, Ordering::Relaxed);
                }
                sent.get() < 10_000
            },
            Duration::from_millis(100),
            &stop,
        );
        sent.get()
    }

    #[test]
    fn refresh_loop_paces_rounds_even_when_rounds_panic() {
        // ~350 ms at a 100 ms interval: first + immediate follow-up + ~3 paced rounds.
        let n = count_rounds(|_, _| panic!("parser exploded"), Duration::from_millis(350));
        assert!((3..=6).contains(&n), "rounds = {n}");
    }

    #[test]
    fn refresh_loop_follows_first_round_immediately_once() {
        let firsts = std::cell::Cell::new(0);
        let n = count_rounds(
            |first, prev| {
                firsts.set(firsts.get() + usize::from(first));
                Snapshot { seq: prev.seq + 1, ..Default::default() }
            },
            Duration::from_millis(350),
        );
        assert_eq!(firsts.get(), 1);
        assert!((3..=6).contains(&n), "rounds = {n}");
    }

    #[test]
    fn build_snapshot_filters_stale_sessions() {
        let dir = setup();
        let sd = dir.path().join("sessions");
        write_claude(&sd, 11, "live", "busy");
        write_claude(&sd, 12, "not-in-cmux", "busy");
        write_claude(&sd, 13, "dead", "idle");
        let cmux = FakeCmux::new(vec![cref("live", true), cref("dead", false), cref("no-files", true)]);
        let snap = build_snapshot(&deps(dir.path(), &cmux), &Snapshot::default());
        assert_eq!(ids(&snap), vec!["live"]);
        assert_eq!(snap.sessions[0].status, Status::Running);
        assert_eq!(snap.message, None);
    }

    #[test]
    fn build_snapshot_survives_cmux_failure_keeps_previous() {
        let dir = setup();
        write_claude(&dir.path().join("sessions"), 11, "live", "idle");
        let ok = FakeCmux::new(vec![cref("live", true)]);
        let first = build_snapshot(&deps(dir.path(), &ok), &Snapshot::default());
        assert_eq!(ids(&first), vec!["live"]);

        let mut broken = FakeCmux::new(vec![]);
        broken.sessions_result = Err("socket gone".into());
        broken.tree_result = Err("socket gone".into());
        let second = build_snapshot(&deps(dir.path(), &broken), &first);
        assert_eq!(ids(&second), vec!["live"]);
        assert_eq!(second.sessions[0].status, Status::Idle);
        assert_eq!(second.message.as_deref(), Some(CMUX_UNAVAILABLE));

        let third = build_snapshot(&deps(dir.path(), &ok), &second);
        assert_eq!(third.message, None);
    }

    #[test]
    fn build_snapshot_uses_screen_for_unknown_sessions() {
        let dir = setup();
        write_hooks(&dir.path().join("hooks.json"), &[("odd", "mystery"), ("plain", "mystery")]);
        let mut cmux = FakeCmux::new(vec![cref("odd", true)]);
        cmux.read_screen_result = Ok("Do you want to proceed?".into());
        let snap = build_snapshot(&deps(dir.path(), &cmux), &Snapshot::default());
        assert_eq!(snap.sessions[0].status, Status::NeedsInput);
        assert!(cmux.calls().contains(&"read_screen ws-odd sf-odd".to_string()), "{:?}", cmux.calls());

        // a failing read keeps Unknown
        let mut cmux = FakeCmux::new(vec![cref("plain", true)]);
        cmux.read_screen_result = Err("nope".into());
        let snap = build_snapshot(&deps(dir.path(), &cmux), &Snapshot::default());
        assert_eq!(snap.sessions[0].status, Status::Unknown);
    }

    #[test]
    fn build_snapshot_liveness_uses_the_same_ref_as_merge() {
        let dir = setup();
        write_hooks(&dir.path().join("hooks.json"), &[("dup", "mystery")]);
        // live ref first, a dead leftover of the same session id last
        let live = CmuxSessionRef { surface_id: "sf-new".into(), ..cref("dup", true) };
        let dead = CmuxSessionRef { surface_id: "sf-old".into(), ..cref("dup", false) };
        let mut cmux = FakeCmux::new(vec![live, dead]);
        cmux.read_screen_result = Ok("Do you want to proceed?".into());
        let snap = build_snapshot(&deps(dir.path(), &cmux), &Snapshot::default());
        assert_eq!(snap.sessions.len(), 1);
        assert_eq!(snap.sessions[0].surface_id, "sf-new");
        assert_eq!(snap.sessions[0].status, Status::NeedsInput, "screen read for the live ref");
        assert!(cmux.calls().iter().any(|c| c == "read_screen ws-dup sf-new"), "{:?}", cmux.calls());
    }

    #[test]
    fn build_snapshot_skips_screen_for_known_status() {
        let dir = setup();
        write_claude(&dir.path().join("sessions"), 11, "live", "busy");
        let cmux = FakeCmux::new(vec![cref("live", true)]);
        build_snapshot(&deps(dir.path(), &cmux), &Snapshot::default());
        assert_eq!(cmux.calls(), vec!["sessions".to_string(), "tree".to_string()]);
    }

    #[test]
    fn refresh_round_uses_surfaces_and_survives_tree_failure() {
        let dir = setup();
        let sd = dir.path().join("sessions");
        write_claude(&sd, 11, "a", "busy");
        write_claude(&sd, 12, "b", "busy");
        let mut cmux = FakeCmux::new(vec![cref("a", true), cref("b", true)]);
        let first = build_snapshot(&deps(dir.path(), &cmux), &Snapshot::default());
        assert_eq!(ids(&first), vec!["a", "b"]);
        assert!(cmux.calls().contains(&"tree".to_string()), "{:?}", cmux.calls());

        // b's tab was closed: claude is gone, cmux still lists the dead record
        std::fs::remove_file(sd.join("12.json")).unwrap();
        cmux.sessions_result = Ok(vec![cref("a", true), cref("b", false)]);
        cmux.tree_result = Ok(tree_of(&[("SF-A", "")]));
        let second = build_snapshot(&deps(dir.path(), &cmux), &first);
        assert_eq!(ids(&second), vec!["a"], "dropped at once, no Done tile");
        assert_eq!(second.message, None);

        // tree fails: nothing is dropped for that reason, no blank screen
        cmux.tree_result = Err("tree timed out".into());
        cmux.sessions_result = Ok(vec![cref("a", true)]);
        let third = build_snapshot(&deps(dir.path(), &cmux), &second);
        assert_eq!(ids(&third), vec!["a"]);
        assert_eq!(third.sessions[0].status, Status::Running);
        // and a normal exit with an open tab still becomes Done
        cmux.sessions_result = Ok(vec![cref("a", false)]);
        cmux.tree_result = Ok(tree_of(&[("SF-A", "")]));
        let fourth = build_snapshot(&deps(dir.path(), &cmux), &third);
        assert_eq!(ids(&fourth), vec!["a"]);
        assert_eq!(fourth.sessions[0].status, Status::Done);
    }

    fn cached(cache: &BranchCache, id: &str, branch: &str) {
        cache.put(
            id,
            BranchEntry { cwd: PathBuf::from(format!("/home/user/{id}")), branch: Some(branch.into()), guessed: true, resolved_at: Instant::now() },
        );
    }

    #[test]
    fn build_snapshot_reads_branches_from_cache_and_pr_targets() {
        let dir = setup();
        write_claude(&dir.path().join("sessions"), 11, "a", "busy");
        write_claude(&dir.path().join("sessions"), 12, "b", "busy");
        let cmux = FakeCmux::new(vec![cref("a", true), cref("b", true)]);
        let cache = BranchCache::new();
        cached(&cache, "a", "feature-x");
        cached(&cache, "b", "feature-x");
        let mut d = deps(dir.path(), &cmux);
        d.branches = Some(&cache);
        let snap = build_snapshot(&d, &Snapshot::default());
        assert!(snap.sessions.iter().all(|s| s.branch.as_deref() == Some("feature-x") && s.branch_guessed));
        assert_eq!(snap.pr_targets().len(), 2);

        d.config.extras.branch = false;
        d.config.extras.pr = false;
        let snap = build_snapshot(&d, &Snapshot::default());
        assert!(snap.sessions.iter().all(|s| s.branch.is_none()));
        assert!(snap.pr_targets().is_empty());
    }

    #[test]
    fn build_snapshot_fills_context_usage() {
        let dir = setup();
        for (pid, id) in [(11, "a"), (12, "b"), (13, "c")] {
            write_claude(&dir.path().join("sessions"), pid, id, "busy");
        }
        let cmux = FakeCmux::new(vec![cref("a", true), cref("b", true), cref("c", true)]);
        let d = deps(dir.path(), &cmux);
        // a: fresh statusline file; b: transcript under the projects dir (encoded cwd); c: nothing
        std::fs::create_dir_all(&d.ctx_dir).unwrap();
        std::fs::write(d.ctx_dir.join("a.json"), r#"{"session_id":"a","used_percentage":72}"#).unwrap();
        let proj = d.projects_dir.join("-home-user-b");
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::write(
            proj.join("b.jsonl"),
            "{\"type\":\"assistant\",\"message\":{\"usage\":{\"input_tokens\":10,\"cache_read_input_tokens\":99990,\"cache_creation_input_tokens\":0}}}\n",
        )
        .unwrap();
        let snap = build_snapshot(&d, &Snapshot::default());
        let pct = |id: &str| snap.sessions.iter().find(|s| s.id == id).unwrap().ctx_percent;
        assert_eq!(pct("a"), Some(72.0));
        assert_eq!(pct("b"), Some(50.0));
        assert_eq!(pct("c"), None);
        // the transcript cache is carried to the next round and pruned to live sessions
        assert_eq!(snap.transcripts.len(), 1);
        let again = build_snapshot(&d, &snap);
        assert_eq!(again.sessions.iter().find(|s| s.id == "b").unwrap().ctx_percent, Some(50.0));
    }

    #[test]
    fn transcript_path_falls_back_to_encoded_cwd() {
        let projects = Path::new("/home/user/.claude/projects");
        let mut s = crate::app::fake_session("sess-1", Status::Running);
        s.cwd = PathBuf::from("/home/user/my.proj_x");
        assert_eq!(
            transcript_for(&s, projects),
            Some(projects.join("-home-user-my-proj-x").join("sess-1.jsonl"))
        );
        s.cwd = PathBuf::new();
        assert_eq!(transcript_for(&s, projects), None, "no cwd, no hook path");
        s.transcript = Some(PathBuf::from("/home/user/t.jsonl"));
        assert_eq!(transcript_for(&s, projects), Some(PathBuf::from("/home/user/t.jsonl")), "hook path wins");
        s.transcript = None;
        s.cwd = PathBuf::from("/home/user/p");
        s.id = "../evil".into();
        assert_eq!(transcript_for(&s, projects), None, "unsafe id");
    }

    #[test]
    fn build_snapshot_surfaces_pr_availability() {
        let dir = setup();
        for (pid, id) in [(11, "a"), (12, "b"), (13, "c")] {
            write_claude(&dir.path().join("sessions"), pid, id, "busy");
        }
        let cmux = FakeCmux::new(vec![cref("a", true), cref("b", true), cref("c", true)]);
        let branches = BranchCache::new();
        for id in ["a", "b", "c"] {
            cached(&branches, id, &format!("br-{id}"));
        }
        let prs = PrCache::new();
        let key = |id: &str| (PathBuf::from(format!("/home/user/{id}")), format!("br-{id}"));
        prs.put(key("a"), PrLookup::Unavailable("az failed (not logged in?)".into()));
        prs.put(key("b"), PrLookup::None);
        let mut d = deps(dir.path(), &cmux);
        d.branches = Some(&branches);
        d.pr_cache = Some(&prs);
        d.config.extras.pr = true;
        let snap = build_snapshot(&d, &Snapshot::default());
        let by = |id: &str| snap.sessions.iter().find(|s| s.id == id).unwrap();
        assert!(by("a").pr_unavailable && by("a").pr.is_none());
        assert!(!by("b").pr_unavailable && by("b").pr.is_none(), "no PR: nothing shown");
        assert!(!by("c").pr_unavailable, "not looked up yet");
        assert_eq!(snap.pr_note.as_deref(), Some("PR: az 로그인 필요"));

        prs.put(key("a"), PrLookup::None);
        let snap = build_snapshot(&d, &Snapshot::default());
        assert_eq!(snap.pr_note.as_deref(), Some(crate::pr::PR_PENDING), "c still unknown");
        prs.put(key("c"), PrLookup::None);
        assert_eq!(build_snapshot(&d, &Snapshot::default()).pr_note, None);

        prs.put(key("a"), PrLookup::Unavailable("az failed (not logged in?)".into()));
        d.config.extras.pr = false;
        let snap = build_snapshot(&d, &Snapshot::default());
        assert_eq!(snap.pr_note, None, "PR off: no note");
        assert!(snap.sessions.iter().all(|s| !s.pr_unavailable));
    }

    #[test]
    fn snapshot_branch_targets_carry_pid_transcript_and_done() {
        let mut live = crate::app::fake_session("a", Status::Running);
        live.transcript = Some(PathBuf::from("/home/user/t.jsonl"));
        let done = crate::app::fake_session("b", Status::Done);
        let snap = Snapshot { sessions: vec![live, done], ..Default::default() };
        let t = snap.branch_targets();
        assert_eq!(t.len(), 2);
        assert_eq!((t[0].id.as_str(), t[0].pid, t[0].done), ("a", 100, false));
        assert_eq!(t[0].transcript.as_deref(), Some(Path::new("/home/user/t.jsonl")));
        assert_eq!(t[0].cwd, PathBuf::from("/home/user/proj-a"));
        assert!(t[1].done);
    }

    #[test]
    fn refresh_round_does_not_wait_for_slow_branch_resolver() {
        use crate::branch::{spawn_branch_resolver, BranchTargets, Resolver};
        use std::sync::atomic::AtomicUsize;
        let dir = setup();
        for (pid, id) in [(11, "a"), (12, "b"), (13, "c")] {
            write_claude(&dir.path().join("sessions"), pid, id, "busy");
        }
        let cmux = FakeCmux::new(vec![cref("a", true), cref("b", true), cref("c", true)]);
        let started = Arc::new(AtomicUsize::new(0));
        let s2 = started.clone();
        let hang: Resolver = Arc::new(move |_, _| {
            s2.fetch_add(1, Ordering::SeqCst);
            std::thread::sleep(Duration::from_secs(3));
            (Some("late".into()), false)
        });
        let cache = Arc::new(BranchCache::new());
        let targets: BranchTargets = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let cfg = Arc::new(Mutex::new(Config::default()));
        let h = spawn_branch_resolver(cache.clone(), targets.clone(), cfg, hang, stop.clone());

        let mut d = deps(dir.path(), &cmux);
        d.branches = Some(&cache);
        let first = build_snapshot(&d, &Snapshot::default());
        *targets.lock().unwrap() = first.branch_targets();
        let end = Instant::now() + Duration::from_secs(2);
        while started.load(Ordering::SeqCst) == 0 && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(started.load(Ordering::SeqCst) > 0, "resolver is running (and hanging)");
        let t0 = Instant::now();
        let second = build_snapshot(&d, &first);
        assert!(t0.elapsed() < Duration::from_millis(200), "round blocked for {:?}", t0.elapsed());
        assert_eq!(second.sessions.len(), 3);
        assert!(second.sessions.iter().all(|s| s.branch.is_none()), "not resolved yet: no branch");
        stop.store(true, Ordering::Relaxed);
        drop(h); // detach: never wait for the hanging resolver
    }
}
