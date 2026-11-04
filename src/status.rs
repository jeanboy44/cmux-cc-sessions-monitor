use std::time::{SystemTime, UNIX_EPOCH};

use crate::merge::parse_time;
use crate::model::Status;
use crate::source::claude_sessions::ClaudeSessionRecord;
use crate::source::context::Turn;
use crate::source::hook_sessions::HookRecord;

/// A sessions-file `busy` whose status has not changed for this long, while
/// the transcript shows the turn finished before that change, is not trusted.
pub const STALE_BUSY_MS: u64 = 5 * 60 * 1000;
/// Tolerance when comparing timestamps written by different processes.
const SKEW_MS: u64 = 1000;

fn ms_of(t: SystemTime) -> Option<u64> {
    t.duration_since(UNIX_EPOCH).ok().map(|d| d.as_millis() as u64)
}

/// The sessions file's status change time (`statusUpdatedAt`, else `updatedAt`), in ms.
fn sessions_status_ms(claude: Option<&ClaudeSessionRecord>) -> Option<u64> {
    claude.and_then(|c| c.status_updated_at_ms.or(c.updated_at_ms))
}

/// Whether the hook's last event ends a turn (`Stop`, or `StopFailure` after an API error).
fn hook_turn_ended(h: &HookRecord) -> bool {
    matches!(h.hook_event_name.as_deref(), Some("Stop" | "StopFailure"))
}

/// Implements the decision list from docs/spike-notes.md, amended for the
/// "stuck RUN" bug (see `.superpowers/sdd/2026-11-04-spirit-ui/stuck-run-report.md`).
/// `alive` is the pid liveness (checked by the caller).
///
/// Claude Code keeps the sessions file at `busy` while background work (a
/// background Bash/Monitor, a background subagent) is pending, long after the
/// turn ended, so `busy` alone does not mean the turn is in progress: a hook
/// `Stop` at or after the `busy` change wins. `shell` (and anything else) never
/// implies Running; it falls through to the hook and the screen.
/// [`settle_with_transcript`] then refines a remaining Running.
pub fn resolve_status(
    hook: Option<&HookRecord>,
    claude: Option<&ClaudeSessionRecord>,
    alive: bool,
    screen: Option<&str>,
) -> Status {
    // 1. dead pid
    if !alive {
        return Status::Done;
    }
    let sessions_status = claude
        .and_then(|c| c.status.as_deref())
        .filter(|s| matches!(*s, "waiting" | "busy" | "idle"));
    // 2. alive but neither a sessions record nor a hook entry. A record whose
    // status is missing/unrecognized is not "absent": it falls through to the
    // screen rule (and stays Unknown without a screen).
    if claude.is_none() && hook.is_none() {
        return Status::Done;
    }
    // 3-5. sessions status wins over any hook value, except that a hook turn
    // end at or after the `busy` change means only background work is left.
    match sessions_status {
        Some("waiting") => return Status::NeedsInput,
        Some("busy") => {
            let hook_end = hook
                .filter(|h| hook_turn_ended(h))
                .and_then(|h| h.updated_at.as_deref())
                .and_then(parse_time)
                .and_then(ms_of);
            return match (hook_end, sessions_status_ms(claude)) {
                (Some(end), Some(busy)) if end + SKEW_MS >= busy => Status::Idle,
                _ => Status::Running,
            };
        }
        Some("idle") => return Status::Idle,
        _ => {}
    }
    // 6-8. hook (only reached when sessions status is missing/unrecognized/`shell`)
    if let Some(h) = hook {
        match (h.agent_lifecycle.as_deref(), h.hook_event_name.as_deref()) {
            (Some("needsInput"), Some("Notification")) => return Status::NeedsInput,
            // turn ended; pending background work is not a running turn
            _ if hook_turn_ended(h) => return Status::Idle,
            (Some("backgroundWorkPending"), _) => return Status::Idle,
            (Some("running"), _) => return Status::Running,
            (Some("idle"), _) => return Status::Idle,
            _ => {}
        }
    }
    // 9. screen pattern as last resort
    // (a screen without the prompt means idle; no screen at all stays Unknown)
    match screen {
        Some(text) if text.contains("Do you want to proceed?") => Status::NeedsInput,
        Some(_) => Status::Idle,
        None => Status::Unknown,
    }
}

/// Refines a Running status with the main-chain turn state of the transcript
/// tail. A finished turn (assistant `end_turn`, `stop_hook_summary` /
/// `turn_duration`, an interrupt, or a turn parked on background agents)
/// makes it Idle when it finished at or after the sessions file's `busy`
/// change, or when that change is older than [`STALE_BUSY_MS`] (no new turn
/// reached the transcript since). An active turn or no transcript keeps Running.
pub fn settle_with_transcript(
    status: Status,
    turn: Option<Turn>,
    claude: Option<&ClaudeSessionRecord>,
    now: SystemTime,
) -> Status {
    let Some(Turn::Finished(at)) = turn else {
        return status;
    };
    if status != Status::Running {
        return status;
    }
    let Some(busy) = sessions_status_ms(claude) else {
        return Status::Idle;
    };
    if at.is_some_and(|at| at + SKEW_MS >= busy) {
        return Status::Idle;
    }
    let now = ms_of(now).unwrap_or(0);
    if now.saturating_sub(busy) > STALE_BUSY_MS {
        Status::Idle
    } else {
        Status::Running
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::Duration;

    /// 2026-11-04T05:00:00Z in ms.
    const T0: u64 = 1_793_768_400_000;
    const MIN: u64 = 60_000;

    fn claude(status: Option<&str>) -> ClaudeSessionRecord {
        ClaudeSessionRecord {
            pid: 1,
            session_id: "s1".into(),
            cwd: PathBuf::from("/home/user/p"),
            name: None,
            status: status.map(String::from),
            started_at_ms: None,
            updated_at_ms: None,
            status_updated_at_ms: None,
        }
    }

    fn claude_at(status: &str, status_ms: u64) -> ClaudeSessionRecord {
        ClaudeSessionRecord { status_updated_at_ms: Some(status_ms), ..claude(Some(status)) }
    }

    fn hook(lifecycle: Option<&str>, event: Option<&str>) -> HookRecord {
        HookRecord {
            session_id: "s1".into(),
            pid: Some(1),
            workspace_id: "w".into(),
            surface_id: "f".into(),
            cwd: PathBuf::from("/home/user/p"),
            agent_lifecycle: lifecycle.map(String::from),
            hook_event_name: event.map(String::from),
            last_body: None,
            transcript_path: None,
            started_at: None,
            updated_at: None,
        }
    }

    /// A hook record as cmux writes it: `updatedAt` in float seconds.
    fn hook_at(lifecycle: &str, event: &str, ms: u64) -> HookRecord {
        let secs = format!("{}.{:03}", ms / 1000, ms % 1000);
        HookRecord { updated_at: Some(secs), ..hook(Some(lifecycle), Some(event)) }
    }

    fn at(ms: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_millis(ms)
    }

    #[test]
    fn status_rule1_dead_pid_is_done() {
        let c = claude(Some("busy"));
        let h = hook(Some("running"), Some("PreToolUse"));
        assert_eq!(resolve_status(Some(&h), Some(&c), false, Some("Do you want to proceed?")), Status::Done);
        let c = claude_at("busy", T0);
        let h = hook_at("backgroundWorkPending", "Stop", T0 + MIN);
        assert_eq!(resolve_status(Some(&h), Some(&c), false, None), Status::Done);
    }

    #[test]
    fn status_rule2_alive_without_any_record_is_done() {
        assert_eq!(resolve_status(None, None, true, None), Status::Done);
    }

    #[test]
    fn status_unrecognized_claude_status_without_hook_falls_to_screen_rule() {
        // A claude record exists, so the session is not Done: rule 9 decides.
        for c in [claude(Some("weird")), claude(None), claude(Some("shell"))] {
            assert_eq!(resolve_status(None, Some(&c), true, None), Status::Unknown);
            assert_eq!(resolve_status(None, Some(&c), true, Some("plain prompt")), Status::Idle);
            assert_eq!(resolve_status(None, Some(&c), true, Some("Do you want to proceed?")), Status::NeedsInput);
            assert_eq!(resolve_status(None, Some(&c), false, None), Status::Done, "dead pid still wins");
        }
    }

    #[test]
    fn status_rule3_claude_waiting_is_needs_input() {
        let c = claude(Some("waiting"));
        assert_eq!(resolve_status(None, Some(&c), true, None), Status::NeedsInput);
        // a newer hook Stop does not override a pending question
        let c = claude_at("waiting", T0);
        let h = hook_at("backgroundWorkPending", "Stop", T0 + MIN);
        assert_eq!(resolve_status(Some(&h), Some(&c), true, None), Status::NeedsInput);
    }

    #[test]
    fn status_rule4_stale_hook_needs_input_ignored_when_claude_busy() {
        let c = claude(Some("busy"));
        let h = hook(Some("needsInput"), Some("Notification"));
        assert_eq!(resolve_status(Some(&h), Some(&c), true, None), Status::Running);
    }

    #[test]
    fn status_shell_never_implies_running() {
        let c = claude(Some("shell"));
        assert_eq!(resolve_status(None, Some(&c), true, None), Status::Unknown);
        let stop = hook_at("backgroundWorkPending", "Stop", T0);
        assert_eq!(resolve_status(Some(&stop), Some(&c), true, None), Status::Idle);
        let idle = hook(Some("idle"), Some("Stop"));
        assert_eq!(resolve_status(Some(&idle), Some(&c), true, None), Status::Idle);
        // the hook still reports a genuine running turn
        let run = hook(Some("running"), Some("UserPromptSubmit"));
        assert_eq!(resolve_status(Some(&run), Some(&c), true, None), Status::Running);
    }

    /// Measured shapes: sessions `busy` vs the hook's last event and its time.
    #[test]
    fn status_busy_against_hook_turn_end_table() {
        let cases: &[(&str, &str, i64, Status)] = &[
            // (lifecycle, event, hook time - busy time in minutes, expected)
            ("backgroundWorkPending", "Stop", 30, Status::Idle),
            ("backgroundWorkPending", "Stop", 0, Status::Idle),
            ("idle", "Stop", 79, Status::Idle),
            ("needsInput", "StopFailure", 5, Status::Idle),
            ("backgroundWorkPending", "Stop", -1, Status::Running),
            ("idle", "Stop", -20, Status::Running),
            ("running", "UserPromptSubmit", 1, Status::Running),
            ("running", "PreToolUse", 30, Status::Running),
            ("running", "PostToolUse", -5, Status::Running),
        ];
        let busy = T0 + 100 * MIN;
        for &(lifecycle, event, delta, want) in cases {
            let hook_ms = (busy as i64 + delta * MIN as i64) as u64;
            let h = hook_at(lifecycle, event, hook_ms);
            let c = claude_at("busy", busy);
            assert_eq!(resolve_status(Some(&h), Some(&c), true, None), want, "{lifecycle}/{event} {delta:+}min");
        }
    }

    #[test]
    fn status_busy_hook_time_units_and_skew() {
        let busy = T0;
        let c = claude_at("busy", busy);
        // float seconds, integer seconds, integer ms and ISO all compare in ms
        for ts in [
            format!("{}.5", (busy + 2000) / 1000),
            format!("{}", (busy + 2000) / 1000),
            format!("{}", busy + 2000),
            "2026-11-04T05:00:02Z".to_string(),
        ] {
            let h = HookRecord { updated_at: Some(ts.clone()), ..hook(Some("idle"), Some("Stop")) };
            assert_eq!(resolve_status(Some(&h), Some(&c), true, None), Status::Idle, "{ts}");
        }
        // within the skew tolerance the Stop still counts as newer
        let h = hook_at("idle", "Stop", busy - 500);
        assert_eq!(resolve_status(Some(&h), Some(&c), true, None), Status::Idle);
        let h = hook_at("idle", "Stop", busy - 5000);
        assert_eq!(resolve_status(Some(&h), Some(&c), true, None), Status::Running);
    }

    #[test]
    fn status_busy_without_comparable_times_stays_running() {
        // missing or unparsable timestamps cannot prove the turn ended
        let h = hook(Some("backgroundWorkPending"), Some("Stop"));
        assert_eq!(resolve_status(Some(&h), Some(&claude_at("busy", T0)), true, None), Status::Running);
        let h = hook_at("backgroundWorkPending", "Stop", T0);
        assert_eq!(resolve_status(Some(&h), Some(&claude(Some("busy"))), true, None), Status::Running);
        let h = HookRecord { updated_at: Some("soon".into()), ..hook(Some("idle"), Some("Stop")) };
        assert_eq!(resolve_status(Some(&h), Some(&claude_at("busy", T0)), true, None), Status::Running);
        // updatedAt is the fallback for a missing statusUpdatedAt
        let c = ClaudeSessionRecord { updated_at_ms: Some(T0), ..claude(Some("busy")) };
        assert_eq!(resolve_status(Some(&hook_at("idle", "Stop", T0 + MIN)), Some(&c), true, None), Status::Idle);
    }

    #[test]
    fn status_rule5_claude_idle_beats_hook_running() {
        let c = claude(Some("idle"));
        let h = hook(Some("running"), Some("PreToolUse"));
        assert_eq!(resolve_status(Some(&h), Some(&c), true, None), Status::Idle);
    }

    #[test]
    fn status_rule6_hook_needs_input_requires_notification_event() {
        let ok = hook(Some("needsInput"), Some("Notification"));
        assert_eq!(resolve_status(Some(&ok), None, true, None), Status::NeedsInput);
        let other = hook(Some("needsInput"), Some("PreToolUse"));
        assert_eq!(resolve_status(Some(&other), None, true, None), Status::Unknown);
    }

    #[test]
    fn status_rule7_hook_running_is_running() {
        let h = hook(Some("running"), Some("PreToolUse"));
        assert_eq!(resolve_status(Some(&h), None, true, None), Status::Running);
    }

    #[test]
    fn status_hook_background_work_pending_is_idle() {
        let h = hook(Some("backgroundWorkPending"), Some("Stop"));
        assert_eq!(resolve_status(Some(&h), None, true, None), Status::Idle);
        assert_eq!(resolve_status(Some(&h), Some(&claude(Some("weird"))), true, None), Status::Idle);
        let h = hook(Some("backgroundWorkPending"), None);
        assert_eq!(resolve_status(Some(&h), None, true, None), Status::Idle);
    }

    #[test]
    fn status_sessions_status_beats_background_work_pending() {
        let h = hook(Some("backgroundWorkPending"), Some("Stop"));
        assert_eq!(resolve_status(Some(&h), Some(&claude(Some("idle"))), true, None), Status::Idle);
        assert_eq!(resolve_status(Some(&h), Some(&claude(Some("waiting"))), true, None), Status::NeedsInput);
    }

    #[test]
    fn status_rule8_hook_idle_is_idle() {
        let h = hook(Some("idle"), Some("Stop"));
        assert_eq!(resolve_status(Some(&h), None, true, None), Status::Idle);
    }

    #[test]
    fn status_rule6_hook_used_when_claude_status_unrecognized() {
        let h = hook(Some("running"), None);
        assert_eq!(resolve_status(Some(&h), Some(&claude(Some("weird"))), true, None), Status::Running);
    }

    #[test]
    fn status_falls_back_to_claude_when_no_hook() {
        assert_eq!(resolve_status(None, Some(&claude(Some("idle"))), true, None), Status::Idle);
    }

    #[test]
    fn status_unknown_values_become_unknown() {
        let h = hook(Some("xyz"), None);
        assert_eq!(resolve_status(Some(&h), None, true, None), Status::Unknown);
    }

    #[test]
    fn status_rule9_screen_pattern_is_last_resort() {
        let h = hook(Some("xyz"), None);
        assert_eq!(resolve_status(Some(&h), None, true, Some("...\nDo you want to proceed?\n1. Yes")), Status::NeedsInput);
        assert_eq!(resolve_status(Some(&h), None, true, Some("nothing special")), Status::Idle);
        assert_eq!(resolve_status(Some(&h), None, true, None), Status::Unknown);
        // the screen never overrides a decided status
        let c = claude(Some("busy"));
        assert_eq!(resolve_status(None, Some(&c), true, Some("Do you want to proceed?")), Status::Running);
    }

    /// Measured shapes: Running sessions against the transcript's turn state.
    #[test]
    fn settle_with_transcript_table() {
        let busy = T0;
        let now = at(T0 + 80 * MIN);
        let cases: &[(Option<Turn>, Option<u64>, SystemTime, Status, &str)] = &[
            (Some(Turn::Finished(Some(busy + 30 * MIN))), Some(busy), now, Status::Idle, "turn ended after busy"),
            (Some(Turn::Finished(Some(busy))), Some(busy), now, Status::Idle, "ended at busy"),
            (Some(Turn::Finished(Some(busy - 500))), Some(busy), at(busy + 1000), Status::Idle, "within skew"),
            (Some(Turn::Finished(Some(busy - 60_000))), Some(busy), at(busy + 2000), Status::Running, "new turn not in transcript yet"),
            (Some(Turn::Finished(Some(busy - 60_000))), Some(busy), at(busy + 6 * MIN), Status::Idle, "stale busy"),
            (Some(Turn::Finished(None)), Some(busy), at(busy + MIN), Status::Running, "no end time, fresh busy"),
            (Some(Turn::Finished(None)), Some(busy), now, Status::Idle, "no end time, stale busy"),
            (Some(Turn::Finished(Some(busy))), None, now, Status::Idle, "no sessions time"),
            (Some(Turn::Active), Some(busy), now, Status::Running, "tool_use / prompt in progress"),
            (None, Some(busy), now, Status::Running, "no transcript"),
            (Some(Turn::Finished(Some(busy - 60_000))), Some(busy), at(busy - 10 * MIN), Status::Running, "clock behind"),
        ];
        for &(turn, busy_ms, now, want, why) in cases {
            let c = ClaudeSessionRecord { status_updated_at_ms: busy_ms, ..claude(Some("busy")) };
            assert_eq!(settle_with_transcript(Status::Running, turn, Some(&c), now), want, "{why}");
        }
        assert_eq!(settle_with_transcript(Status::Running, Some(Turn::Finished(Some(1))), None, now), Status::Idle);
    }

    #[test]
    fn settle_with_transcript_only_touches_running() {
        let fin = Some(Turn::Finished(Some(T0 + MIN)));
        let c = claude_at("waiting", T0);
        let now = at(T0 + 80 * MIN);
        for s in [Status::NeedsInput, Status::Done, Status::Idle, Status::Unknown] {
            assert_eq!(settle_with_transcript(s, fin, Some(&c), now), s);
            assert_eq!(settle_with_transcript(s, Some(Turn::Active), Some(&c), now), s);
        }
    }
}
