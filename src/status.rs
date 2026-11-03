use crate::model::Status;
use crate::source::claude_sessions::ClaudeSessionRecord;
use crate::source::hook_sessions::HookRecord;

/// Implements the 9-step decision list from docs/spike-notes.md.
/// `alive` is the pid liveness (checked by the caller).
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
        .filter(|s| matches!(*s, "waiting" | "busy" | "shell" | "idle"));
    // 2. alive but neither a sessions record nor a hook entry. A record whose
    // status is missing/unrecognized is not "absent": it falls through to the
    // screen rule (and stays Unknown without a screen).
    if claude.is_none() && hook.is_none() {
        return Status::Done;
    }
    // 3-5. sessions status wins over any hook value
    match sessions_status {
        Some("waiting") => return Status::NeedsInput,
        Some("busy") | Some("shell") => return Status::Running,
        Some("idle") => return Status::Idle,
        _ => {}
    }
    // 6-8. hook (only reached when sessions status is missing/unrecognized)
    if let Some(h) = hook {
        match (h.agent_lifecycle.as_deref(), h.hook_event_name.as_deref()) {
            (Some("needsInput"), Some("Notification")) => return Status::NeedsInput,
            (Some("running"), _) | (Some("backgroundWorkPending"), _) => return Status::Running,
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn claude(status: Option<&str>) -> ClaudeSessionRecord {
        ClaudeSessionRecord {
            pid: 1,
            session_id: "s1".into(),
            cwd: PathBuf::from("/home/user/p"),
            name: None,
            status: status.map(String::from),
            started_at_ms: None,
            updated_at_ms: None,
        }
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
            updated_at: None,
        }
    }

    #[test]
    fn status_rule1_dead_pid_is_done() {
        let c = claude(Some("busy"));
        let h = hook(Some("running"), Some("PreToolUse"));
        assert_eq!(resolve_status(Some(&h), Some(&c), false, Some("Do you want to proceed?")), Status::Done);
    }

    #[test]
    fn status_rule2_alive_without_any_record_is_done() {
        assert_eq!(resolve_status(None, None, true, None), Status::Done);
    }

    #[test]
    fn status_unrecognized_claude_status_without_hook_falls_to_screen_rule() {
        // A claude record exists, so the session is not Done: rule 9 decides.
        for c in [claude(Some("weird")), claude(None)] {
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
    }

    #[test]
    fn status_rule4_stale_hook_needs_input_ignored_when_claude_busy() {
        let c = claude(Some("busy"));
        let h = hook(Some("needsInput"), Some("Notification"));
        assert_eq!(resolve_status(Some(&h), Some(&c), true, None), Status::Running);
        assert_eq!(resolve_status(None, Some(&claude(Some("shell"))), true, None), Status::Running);
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
    fn status_hook_background_work_pending_is_running() {
        let h = hook(Some("backgroundWorkPending"), Some("Stop"));
        assert_eq!(resolve_status(Some(&h), None, true, None), Status::Running);
        assert_eq!(resolve_status(Some(&h), Some(&claude(Some("weird"))), true, None), Status::Running);
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
}
