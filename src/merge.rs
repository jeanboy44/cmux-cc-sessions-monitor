use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::model::{Session, Status};
use crate::source::claude_sessions::ClaudeSessionRecord;
use crate::source::cmux::{CmuxSessionRef, Notification, TreeLayout};
use crate::source::hook_sessions::HookRecord;
use crate::status::resolve_status;

/// Days since 1970-01-01 for a proleptic Gregorian date (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn num(s: &str) -> Option<i64> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

/// Parses `YYYY-MM-DDTHH:MM:SS(.fff)Z` or a plain number (seconds, or
/// milliseconds when 12+ digits). Anything else yields `None`.
fn parse_time(s: &str) -> Option<SystemTime> {
    let s = s.trim();
    if let Some(n) = num(s) {
        let ms = if n >= 100_000_000_000 { n as u64 } else { (n as u64).checked_mul(1000)? };
        return Some(UNIX_EPOCH + Duration::from_millis(ms));
    }
    let b = s.strip_suffix('Z')?.as_bytes();
    let s = std::str::from_utf8(b).ok()?;
    let (date, time) = s.split_once('T')?;
    let mut d = date.split('-');
    let ys = d.next()?;
    let (y, mo, da) = (num(ys)?, num(d.next()?)?, num(d.next()?)?);
    if ys.len() != 4 || d.next().is_some() || !(1..=12).contains(&mo) {
        return None;
    }
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let month_len = match mo {
        2 if leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    if !(1..=month_len).contains(&da) {
        return None;
    }
    let (hms, frac) = match time.split_once('.') {
        Some((h, f)) => (h, Some(f)),
        None => (time, None),
    };
    let mut t = hms.split(':');
    let (h, mi, se) = (num(t.next()?)?, num(t.next()?)?, num(t.next()?)?);
    if t.next().is_some() || h > 23 || mi > 59 || se > 60 {
        return None;
    }
    let mut ms = 0;
    if let Some(f) = frac {
        num(f)?;
        let digits: String = f.chars().take(3).collect();
        ms = num(&digits)? * 10_i64.pow(3 - digits.len() as u32);
    }
    let secs = days_from_civil(y, mo, da).checked_mul(86_400)?.checked_add(h * 3600 + mi * 60 + se)?;
    let total = secs.checked_mul(1000)?.checked_add(ms)?;
    u64::try_from(total).ok().map(|t| UNIX_EPOCH + Duration::from_millis(t))
}

fn first_line(s: &str) -> Option<String> {
    let l = s.lines().next()?.trim();
    (!l.is_empty()).then(|| l.to_string())
}

fn build(c: Option<&ClaudeSessionRecord>, h: Option<&HookRecord>, x: &CmuxSessionRef) -> Session {
    let cwd = [c.map(|c| c.cwd.clone()), h.map(|h| h.cwd.clone()), Some(x.cwd.clone())]
        .into_iter()
        .flatten()
        .find(|p| !p.as_os_str().is_empty())
        .unwrap_or_default();
    let name = c
        .and_then(|c| c.name.clone())
        .filter(|n| !n.is_empty())
        .or_else(|| cwd.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| x.session_id.clone());
    let claude_updated = c.and_then(|c| c.updated_at_ms).map(|ms| UNIX_EPOCH + Duration::from_millis(ms));
    let hook_updated = h.and_then(|h| h.updated_at.as_deref()).and_then(parse_time);
    Session {
        id: x.session_id.clone(),
        name,
        status: Status::Unknown,
        pid: c.map(|c| c.pid).or_else(|| h.and_then(|h| h.pid)).unwrap_or(0),
        cwd,
        workspace_id: x.workspace_id.clone(),
        surface_id: x.surface_id.clone(),
        started_at: c.and_then(|c| c.started_at_ms).map(|ms| UNIX_EPOCH + Duration::from_millis(ms)),
        updated_at: claude_updated.max(hook_updated),
        ended_at: None,
        summary: h.and_then(|h| h.last_body.as_deref()).and_then(first_line),
        branch: None,
        branch_guessed: false,
        pr: None,
        pr_unavailable: false,
        transcript: h.and_then(|h| h.transcript_path.clone()),
        window_id: None,
        ctx_percent: None,
        model: None,
        unread_notification: false,
    }
}

/// True when `b` should replace `a` as the ref of their session id: a live pid
/// wins, then the later `updated=` timestamp, then the later line (`b`).
fn later_ref_wins(a: &CmuxSessionRef, b: &CmuxSessionRef) -> bool {
    if a.pid_exists != b.pid_exists {
        return b.pid_exists;
    }
    let ta = a.updated.as_deref().and_then(parse_time);
    let tb = b.updated.as_deref().and_then(parse_time);
    match (ta, tb) {
        (Some(x), Some(y)) if x != y => y > x,
        (Some(_), None) => false,
        (None, Some(_)) => true,
        _ => true,
    }
}

/// One cmux ref per session id, in order of first appearance. The same id
/// can be listed more than once (e.g. a resumed session on a new surface);
/// see [`later_ref_wins`] for the choice. Refs without workspace or surface
/// are skipped. Used by both the merge and the refresh liveness check so they
/// always agree.
pub fn select_refs(refs: &[CmuxSessionRef]) -> Vec<&CmuxSessionRef> {
    let mut out: Vec<&CmuxSessionRef> = Vec::new();
    let mut index: HashMap<&str, usize> = HashMap::new();
    for r in refs {
        if r.workspace_id.is_empty() || r.surface_id.is_empty() {
            continue;
        }
        match index.get(r.session_id.as_str()) {
            Some(&i) => {
                if later_ref_wins(out[i], r) {
                    out[i] = r;
                }
            }
            None => {
                index.insert(r.session_id.as_str(), out.len());
                out.push(r);
            }
        }
    }
    out
}

/// Pure merge of the three sources into the session list (see Task 4 rulings):
/// sessions need a cmux ref with workspace and surface; sessions never seen
/// alive are hidden; a session turns `Done` (ended_at recorded once) when it
/// was live in `prev` and is now dead or gone, and is dropped after `retention`.
///
/// `open_surfaces` is the cmux tree (which surfaces still exist). When known, a session whose tab was closed is dropped at once (no
/// `Done` tile, no retention) whatever its pid or previous state; `None`
/// (the tree call failed) drops nothing for that reason.
pub fn merge_sessions(
    claude: &[ClaudeSessionRecord],
    hooks: &[HookRecord],
    cmux: &[CmuxSessionRef],
    now: SystemTime,
    retention: Duration,
    prev: &[Session],
    open_surfaces: Option<&TreeLayout>,
) -> Vec<Session> {
    let claude_by: HashMap<&str, &ClaudeSessionRecord> = claude.iter().map(|c| (c.session_id.as_str(), c)).collect();
    let hook_by: HashMap<&str, &HookRecord> = hooks.iter().map(|h| (h.session_id.as_str(), h)).collect();
    let prev_by: HashMap<&str, &Session> = prev.iter().map(|s| (s.id.as_str(), s)).collect();

    let mut out: Vec<Session> = Vec::new();
    let mut seen: HashSet<&str> = HashSet::new();

    for x in select_refs(cmux) {
        let id = x.session_id.as_str();
        let (c, h) = (claude_by.get(id).copied(), hook_by.get(id).copied());
        if c.is_none() && h.is_none() {
            continue;
        }
        seen.insert(id);
        // A prev-Done session whose pid is alive again is shown live again
        // (status re-resolved, `ended_at` stays cleared).
        let mut s = build(c, h, x);
        s.status = resolve_status(h, c, x.pid_exists, None);
        if s.status == Status::Done {
            match prev_by.get(id) {
                Some(p) if p.status == Status::Done => s.ended_at = Some(p.ended_at.unwrap_or(now)),
                Some(_) => s.ended_at = Some(now),
                None => continue, // never seen alive
            }
        }
        out.push(s);
    }

    for p in prev {
        if seen.contains(p.id.as_str()) {
            continue;
        }
        let mut s = p.clone();
        if s.status != Status::Done {
            s.status = Status::Done;
            s.ended_at = Some(now);
        } else if s.ended_at.is_none() {
            s.ended_at = Some(now);
        }
        out.push(s);
    }

    if let Some(open) = open_surfaces {
        out.retain(|s| open.has_surface(&s.surface_id));
    }
    out.retain(|s| match (s.status, s.ended_at) {
        (Status::Done, Some(e)) => now.duration_since(e).map_or(true, |age| age < retention),
        _ => true,
    });
    out.sort_by(|a, b| match (a.started_at, b.started_at) {
        (Some(x), Some(y)) => x.cmp(&y),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
    .then_with(|| a.id.cmp(&b.id)));
    out
}

/// Sets [`Session::unread_notification`] from the cmux notification list:
/// true when any unread notification belongs to the session's surface (ids
/// compare case-insensitively; cmux prints them in either case).
/// Notifications for surfaces without a session are ignored, and every other
/// session is cleared, so a read (or a failed, empty) list stops the blink.
pub fn apply_notifications(sessions: &mut [Session], notes: &[Notification]) {
    let unread: HashSet<String> =
        notes.iter().filter(|n| n.unread).map(|n| n.surface_id.to_ascii_uppercase()).collect();
    for s in sessions.iter_mut() {
        s.unread_notification = unread.contains(&s.surface_id.to_ascii_uppercase());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Status;
    use crate::source::cmux::Notification;
    use std::path::PathBuf;
    use std::time::UNIX_EPOCH;

    const RET: Duration = Duration::from_secs(60);

    fn now() -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(1_700_000_100)
    }

    fn cr(id: &str, pid: u32, status: &str, started_ms: Option<u64>) -> ClaudeSessionRecord {
        ClaudeSessionRecord {
            pid,
            session_id: id.into(),
            cwd: PathBuf::from("/home/user/proj"),
            name: None,
            status: Some(status.into()),
            started_at_ms: started_ms,
            updated_at_ms: None,
        }
    }

    fn hk(id: &str) -> HookRecord {
        HookRecord {
            session_id: id.into(),
            pid: Some(1),
            workspace_id: "w".into(),
            surface_id: "f".into(),
            cwd: PathBuf::from("/home/user/proj"),
            agent_lifecycle: Some("running".into()),
            hook_event_name: None,
            last_body: None,
            transcript_path: None,
            updated_at: None,
        }
    }

    fn cx(id: &str, alive: bool) -> CmuxSessionRef {
        CmuxSessionRef {
            session_id: id.into(),
            workspace_id: "w".into(),
            surface_id: "f".into(),
            cwd: PathBuf::from("/home/user/proj"),
            pid_exists: alive,
            updated: None,
        }
    }

    fn cx_at(id: &str, surface: &str, alive: bool, updated: Option<&str>) -> CmuxSessionRef {
        CmuxSessionRef { surface_id: surface.into(), updated: updated.map(String::from), ..cx(id, alive) }
    }

    fn picked(refs: &[CmuxSessionRef]) -> Vec<(String, String)> {
        select_refs(refs).iter().map(|r| (r.session_id.clone(), r.surface_id.clone())).collect()
    }

    #[test]
    fn select_refs_prefers_live_pid() {
        let refs = [cx_at("s", "old", false, Some("2026-01-02T00:00:00Z")), cx_at("s", "new", true, Some("2026-01-01T00:00:00Z"))];
        assert_eq!(picked(&refs), [("s".into(), "new".into())]);
        let refs = [cx_at("s", "new", true, None), cx_at("s", "old", false, Some("2026-01-02T00:00:00Z"))];
        assert_eq!(picked(&refs), [("s".into(), "new".into())]);
    }

    #[test]
    fn select_refs_then_latest_updated() {
        let refs = [cx_at("s", "b", true, Some("2026-01-01T00:00:05.000Z")), cx_at("s", "a", true, Some("2026-01-01T00:00:01.000Z"))];
        assert_eq!(picked(&refs), [("s".into(), "b".into())]);
        // a parsable timestamp beats a missing one
        let refs = [cx_at("s", "b", true, Some("2026-01-01T00:00:01.000Z")), cx_at("s", "a", true, None)];
        assert_eq!(picked(&refs), [("s".into(), "b".into())]);
    }

    #[test]
    fn select_refs_then_later_line_and_keeps_first_appearance_order() {
        let refs = [
            cx_at("x", "x1", true, None),
            cx_at("s", "s1", true, Some("2026-01-01T00:00:00Z")),
            cx_at("y", "y1", true, None),
            cx_at("s", "s2", true, Some("2026-01-01T00:00:00Z")),
        ];
        assert_eq!(
            picked(&refs),
            [("x".into(), "x1".into()), ("s".into(), "s2".into()), ("y".into(), "y1".into())]
        );
    }

    #[test]
    fn merge_resumed_session_on_new_surface_uses_live_ref() {
        let c = [cr("s1", 10, "busy", None)];
        for refs in [
            vec![cx_at("s1", "f-old", false, Some("2026-01-01T00:00:00Z")), cx_at("s1", "f-new", true, Some("2026-01-01T01:00:00Z"))],
            vec![cx_at("s1", "f-new", true, Some("2026-01-01T01:00:00Z")), cx_at("s1", "f-old", false, Some("2026-01-01T00:00:00Z"))],
        ] {
            let out = merge(&c, &[], &refs, &[]);
            assert_eq!(out.len(), 1);
            assert_eq!(out[0].status, Status::Running);
            assert_eq!(out[0].surface_id, "f-new");
        }
    }

    fn merge(c: &[ClaudeSessionRecord], h: &[HookRecord], x: &[CmuxSessionRef], prev: &[Session]) -> Vec<Session> {
        merge_sessions(c, h, x, now(), RET, prev, None)
    }

    #[test]
    fn merge_fills_summary_and_name_fallback() {
        let c = cr("s1", 10, "busy", Some(1_700_000_000_000));
        let mut h = hk("s1");
        h.last_body = Some("first line\nsecond line".into());
        let out = merge(&[c.clone()], &[h.clone()], &[cx("s1", true)], &[]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].summary.as_deref(), Some("first line"));
        assert_eq!(out[0].name, "proj");
        assert_eq!(out[0].status, Status::Running);
        assert_eq!(out[0].pid, 10);
        assert_eq!((out[0].workspace_id.as_str(), out[0].surface_id.as_str()), ("w", "f"));
        assert_eq!(out[0].started_at, Some(UNIX_EPOCH + Duration::from_millis(1_700_000_000_000)));
        assert_eq!((out[0].branch.clone(), out[0].branch_guessed, out[0].pr.clone()), (None, false, None));

        let mut named = c.clone();
        named.name = Some("demo".into());
        h.last_body = Some("".into());
        let out = merge(&[named], &[h], &[cx("s1", true)], &[]);
        assert_eq!(out[0].name, "demo");
        assert_eq!(out[0].summary, None);
    }

    #[test]
    fn merge_takes_transcript_path_from_hook() {
        let mut h = hk("s1");
        h.transcript_path = Some(PathBuf::from("/home/user/.t/s1.jsonl"));
        let out = merge(&[cr("s1", 10, "busy", None)], &[h], &[cx("s1", true)], &[]);
        assert_eq!(out[0].transcript, Some(PathBuf::from("/home/user/.t/s1.jsonl")));
        let out = merge(&[cr("s1", 10, "busy", None)], &[], &[cx("s1", true)], &[]);
        assert_eq!(out[0].transcript, None);
    }

    #[test]
    fn merge_updated_at_is_later_of_both_sources() {
        let mut c = cr("s1", 10, "busy", None);
        c.updated_at_ms = Some(1_700_000_005_000);
        let mut h = hk("s1");
        h.updated_at = Some("2023-11-14T22:13:30.500Z".into()); // 1_700_000_010.5 s
        let out = merge(&[c.clone()], &[h.clone()], &[cx("s1", true)], &[]);
        assert_eq!(out[0].updated_at, Some(UNIX_EPOCH + Duration::from_millis(1_700_000_010_500)));
        h.updated_at = Some("1700000001".into()); // seconds, older
        let out = merge(&[c.clone()], &[h.clone()], &[cx("s1", true)], &[]);
        assert_eq!(out[0].updated_at, Some(UNIX_EPOCH + Duration::from_millis(1_700_000_005_000)));
        h.updated_at = Some("garbage".into());
        c.updated_at_ms = None;
        let out = merge(&[c], &[h], &[cx("s1", true)], &[]);
        assert_eq!(out[0].updated_at, None);
    }

    #[test]
    fn parse_time_handles_iso_and_numbers() {
        let ms = |t: Option<SystemTime>| t.map(|t| t.duration_since(UNIX_EPOCH).unwrap().as_millis());
        assert_eq!(ms(parse_time("1970-01-01T00:00:00Z")), Some(0));
        assert_eq!(ms(parse_time("2023-11-14T22:13:20Z")), Some(1_700_000_000_000));
        assert_eq!(ms(parse_time("2024-02-29T00:00:00.250Z")), Some(1_709_164_800_250));
        assert_eq!(ms(parse_time("1700000009")), Some(1_700_000_009_000));
        assert_eq!(ms(parse_time("1700000009123")), Some(1_700_000_009_123));
        assert!(parse_time("2024-02-29T00:00:00Z").is_some());
        for bad in [
            "",
            "x",
            "garbage-in-here",
            "2023-13-01T00:00:00Z",
            "2023-11-14",
            "2023-11-14T25:00:00Z",
            "-5",
            "-2023-01-01T00:00:00Z",
            "9000000000000000000-01-01T00:00:00Z",
            "12345-01-01T00:00:00Z",
            "99999999999999999999",
            "2023-02-31T00:00:00Z",
            "2023-02-29T00:00:00Z",
            "2024-04-31T00:00:00Z",
            "2024-02-30T00:00:00Z",
        ] {
            assert_eq!(parse_time(bad), None, "{bad}");
        }
    }

    #[test]
    fn merge_excludes_sessions_without_surface() {
        let c = [cr("s1", 10, "busy", None), cr("s2", 11, "busy", None), cr("s3", 12, "busy", None)];
        let mut no_ws = cx("s1", true);
        no_ws.workspace_id.clear();
        let mut no_sf = cx("s2", true);
        no_sf.surface_id.clear();
        let out = merge(&c, &[], &[no_ws, no_sf], &[]);
        assert!(out.is_empty(), "s3 has no cmux ref at all");
    }

    #[test]
    fn merge_stale_never_seen_alive_session_not_shown() {
        let c = [cr("old", 10, "idle", None), cr("live", 11, "busy", None)];
        let out = merge(&c, &[hk("old")], &[cx("old", false), cx("live", true)], &[]);
        assert_eq!(out.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["live"]);
    }

    #[test]
    fn merge_hook_only_session_with_live_ref_appears() {
        let mut h = hk("h1");
        h.pid = Some(77);
        let out = merge(&[], &[h], &[cx("h1", true)], &[]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].pid, 77);
        assert_eq!(out[0].status, Status::Running);
        assert_eq!(out[0].name, "proj");
    }

    #[test]
    fn merge_live_session_with_unknown_status_and_no_hook_appears_as_unknown() {
        let c = [cr("s1", 10, "weird", None)];
        let out = merge(&c, &[], &[cx("s1", true)], &[]);
        assert_eq!(out.len(), 1, "first seen in this state, still shown");
        assert_eq!(out[0].status, Status::Unknown);
        assert_eq!(out[0].ended_at, None);
    }

    #[test]
    fn merge_keeps_done_within_retention_then_drops() {
        let c = [cr("s1", 10, "busy", None)];
        let x = [cx("s1", true)];
        let first = merge(&c, &[], &x, &[]);
        assert_eq!(first[0].status, Status::Running);
        // session vanished from all sources
        let t1 = now() + Duration::from_secs(10);
        let second = merge_sessions(&[], &[], &[], t1, RET, &first, None);
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].status, Status::Done);
        assert_eq!(second[0].ended_at, Some(t1));
        // still within retention
        let t2 = t1 + Duration::from_secs(59);
        let third = merge_sessions(&[], &[], &[], t2, RET, &second, None);
        assert_eq!(third.len(), 1);
        // retention elapsed
        let t3 = t1 + RET;
        assert!(merge_sessions(&[], &[], &[], t3, RET, &third, None).is_empty());
    }

    #[test]
    fn merge_done_when_pid_dies_but_sources_remain() {
        let c = [cr("s1", 10, "busy", None)];
        let first = merge(&c, &[], &[cx("s1", true)], &[]);
        let out = merge(&c, &[], &[cx("s1", false)], &first);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].status, Status::Done);
        assert_eq!(out[0].ended_at, Some(now()));
    }

    #[test]
    fn merge_ended_at_recorded_once() {
        let c = [cr("s1", 10, "busy", None)];
        let first = merge(&c, &[], &[cx("s1", true)], &[]);
        let t1 = now() + Duration::from_secs(5);
        let done = merge_sessions(&c, &[], &[cx("s1", false)], t1, RET, &first, None);
        assert_eq!(done[0].ended_at, Some(t1));
        let t2 = t1 + Duration::from_secs(20);
        let again = merge_sessions(&c, &[], &[cx("s1", false)], t2, RET, &done, None);
        assert_eq!(again[0].ended_at, Some(t1));
        let gone = merge_sessions(&[], &[], &[], t2, RET, &done, None);
        assert_eq!(gone[0].ended_at, Some(t1));
    }

    #[test]
    fn merge_pid_reuse_with_new_session_id_is_separate_session() {
        let first = merge(&[cr("old", 10, "busy", Some(1))], &[], &[cx("old", true)], &[]);
        let c = [cr("new", 10, "idle", Some(2))];
        let out = merge(&c, &[], &[cx("new", true)], &first);
        assert_eq!(out.len(), 2);
        let old = out.iter().find(|s| s.id == "old").unwrap();
        let new = out.iter().find(|s| s.id == "new").unwrap();
        assert_eq!(old.status, Status::Done);
        assert_eq!(new.status, Status::Idle);
        assert_eq!(new.ended_at, None);
    }

    #[test]
    fn merge_sorted_by_started_at() {
        let c = [
            cr("c", 1, "busy", None),
            cr("b", 2, "busy", Some(2000)),
            cr("a2", 3, "busy", Some(1000)),
            cr("a1", 4, "busy", Some(1000)),
        ];
        let x = [cx("c", true), cx("b", true), cx("a2", true), cx("a1", true)];
        let out = merge(&c, &[], &x, &[]);
        assert_eq!(out.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["a1", "a2", "b", "c"]);
    }

    #[test]
    fn merge_clock_skew_ended_in_future_keeps_session() {
        let first = merge(&[cr("s1", 10, "busy", None)], &[], &[cx("s1", true)], &[]);
        let later = now() + Duration::from_secs(100);
        let done = merge_sessions(&[], &[], &[], later, RET, &first, None);
        // `now` earlier than ended_at must not panic and keeps the session
        let out = merge_sessions(&[], &[], &[], now(), RET, &done, None);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].ended_at, Some(later));
    }

    #[test]
    fn merge_done_session_back_alive_is_live_again_with_ended_at_cleared() {
        let c = [cr("s1", 10, "busy", None)];
        let first = merge(&c, &[], &[cx("s1", true)], &[]);
        let done = merge(&c, &[], &[cx("s1", false)], &first);
        assert_eq!(done[0].status, Status::Done);
        let back = merge(&c, &[], &[cx("s1", true)], &done);
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].status, Status::Running);
        assert_eq!(back[0].ended_at, None);
    }

    fn open(ids: &[&str]) -> TreeLayout {
        TreeLayout { surfaces: ids.iter().map(|s| (s.to_string(), String::new())).collect(), window_order: Vec::new() }
    }

    #[test]
    fn merge_drops_session_whose_surface_closed_even_if_recent_done() {
        let c = [cr("s1", 10, "busy", None)];
        let first = merge(&c, &[], &[cx("s1", true)], &[]);
        // tab closed: claude died at once, cmux keeps the record (pid_exists=no)
        let t1 = now() + Duration::from_secs(1);
        let closed = merge_sessions(&[], &[], &[cx("s1", false)], t1, RET, &first, Some(&open(&["OTHER"])));
        assert!(closed.is_empty(), "no Done tile for a closed tab: {closed:?}");
        // a Done session still within retention goes as soon as its tab closes
        let done = merge(&c, &[], &[cx("s1", false)], &first);
        assert_eq!(done[0].status, Status::Done);
        let gone = merge_sessions(&[], &[], &[], t1, RET, &done, Some(&open(&["OTHER"])));
        assert!(gone.is_empty(), "{gone:?}");
    }

    #[test]
    fn merge_drops_live_looking_session_when_surface_missing() {
        let c = [cr("s1", 10, "busy", None), cr("s2", 11, "busy", None)];
        let x = [cx_at("s1", "f1", true, None), cx_at("s2", "f2", true, None)];
        let out = merge_sessions(&c, &[], &x, now(), RET, &[], Some(&open(&["F2"])));
        assert_eq!(out.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["s2"]);
    }

    #[test]
    fn merge_keeps_session_when_surfaces_unknown() {
        let c = [cr("s1", 10, "busy", None)];
        let first = merge_sessions(&c, &[], &[cx("s1", true)], now(), RET, &[], None);
        assert_eq!(first.len(), 1);
        let done = merge_sessions(&c, &[], &[cx("s1", false)], now(), RET, &first, None);
        assert_eq!(done.len(), 1);
        assert_eq!(done[0].status, Status::Done, "normal exit behaviour, nothing dropped");
    }

    #[test]
    fn merge_keeps_done_session_when_tab_still_open() {
        let c = [cr("s1", 10, "busy", None)];
        let first = merge(&c, &[], &[cx("s1", true)], &[]);
        // surface ids compare case-insensitively ("f" vs "F")
        let tab = open(&["F"]);
        let done = merge_sessions(&c, &[], &[cx("s1", false)], now(), RET, &first, Some(&tab));
        assert_eq!(done.len(), 1);
        assert_eq!(done[0].status, Status::Done);
        assert_eq!(done[0].surface_id, "f", "kept for Enter to focus the still-open tab");
        let later = merge_sessions(&[], &[], &[], now() + Duration::from_secs(30), RET, &done, Some(&tab));
        assert_eq!(later.len(), 1, "still within retention");
        assert!(merge_sessions(&[], &[], &[], now() + RET, RET, &later, Some(&tab)).is_empty());
    }

    fn note(surface: &str, unread: bool) -> Notification {
        Notification {
            id: format!("n-{surface}-{unread}"),
            workspace_id: "w".into(),
            surface_id: surface.into(),
            unread,
            title: "t".into(),
            timestamp: "2026-01-01T00:00:00Z".into(),
            workspace_title: String::new(),
        }
    }

    #[test]
    fn notifications_mark_sessions_by_surface() {
        let c = [cr("s1", 10, "busy", Some(1)), cr("s2", 11, "idle", Some(2)), cr("s3", 12, "idle", Some(3))];
        let x = [cx_at("s1", "f1", true, None), cx_at("s2", "f2", true, None), cx_at("s3", "f3", true, None)];
        let mut out = merge_sessions(&c, &[], &x, now(), RET, &[], None);
        assert!(out.iter().all(|s| !s.unread_notification), "merge alone sets nothing");
        let notes = [
            note("F1", true),     // case-insensitive surface match
            note("f2", false),    // read: no cue
            note("f3", false),    // several for one surface: unread if any is unread
            note("f3", true),
            note("f-gone", true), // no session on that surface: ignored
        ];
        apply_notifications(&mut out, &notes);
        let flags: Vec<(&str, bool)> = out.iter().map(|s| (s.id.as_str(), s.unread_notification)).collect();
        assert_eq!(flags, [("s1", true), ("s2", false), ("s3", true)]);
        assert_eq!(out.len(), 3, "unknown surfaces add no session");
        // read later (or the list failed = empty): the cue goes away
        apply_notifications(&mut out, &[note("f1", false)]);
        assert!(out.iter().all(|s| !s.unread_notification));
        apply_notifications(&mut out, &notes);
        apply_notifications(&mut out, &[]);
        assert!(out.iter().all(|s| !s.unread_notification));
    }
}
