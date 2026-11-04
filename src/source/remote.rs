//! Claude Code sessions in remote (`cmux mosh` / `cmux ssh`) tabs. Their
//! processes and files live on the other machine, so the local sources know
//! nothing about them. What this machine has: the tab list (`cmux tree`),
//! each tab's screen (`cmux read-screen`) and the hook records cmux relays to
//! `~/.cmuxterm/relay-hook-state/<app>/<workspace>/` (no pid, cwd or
//! transcript). A remote session is therefore keyed by its tab, and handed to
//! the merge as an ordinary (cmux ref, hook record) pair.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::merge::parse_time;
use crate::model::Status;
use crate::source::cmux::{Cmux, CmuxSessionRef, TreeLayout, SCREEN_LINES};
use crate::source::hook_sessions::{read_hook_sessions, HookRecord};
use crate::status::resolve_status;

/// Start of the rule above Claude Code's input box (it may carry a label).
const RULE: &str = "──────────";
/// Screen lines searched for a dialog; older text may just quote one.
const DIALOG_LINES: usize = 15;
/// Lines above the input box searched for the spinner (a tip or a todo list
/// can sit between the two); older text may just look like one.
const SPINNER_LINES: usize = 8;
/// While the relay says `running`, a screen without spinner is believed only
/// after this long: the spinner can be missed, a `Stop` lost with the relay.
const RUNNING_GRACE: Duration = Duration::from_secs(30);

/// A tab's screen is read at most this often.
pub const SCAN_INTERVAL: Duration = Duration::from_secs(5);
/// At most this many remote screens per refresh round.
pub const MAX_READS: usize = 3;

/// What the last screen reads of a remote tab showed.
#[derive(Debug, Clone, PartialEq)]
pub struct RemoteTab {
    pub workspace_id: String,
    /// Tab title without the `[mosh]` prefix.
    pub title: String,
    pub checked_at: Option<SystemTime>,
    /// The Claude session's state on screen; `None`: no Claude Code there.
    pub screen: Option<Status>,
    /// Reads in a row that saw no Claude Code (see [`scan_tabs`]).
    pub misses: u8,
}

pub fn default_relay_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_default().join(".cmuxterm").join("relay-hook-state")
}

/// The title of a remote tab without its prefix; `None` for a local tab.
pub fn remote_title(title: &str) -> Option<&str> {
    title.strip_prefix("[mosh] ").or_else(|| title.strip_prefix("[ssh] "))
}

/// Every record of `<dir>/<app>/<workspace>/claude-hook-sessions.json`.
pub fn read_relay_hooks(dir: &Path) -> Vec<HookRecord> {
    let subdirs = |d: &Path| -> Vec<PathBuf> {
        fs::read_dir(d).map(|it| it.flatten().map(|e| e.path()).collect()).unwrap_or_default()
    };
    subdirs(dir)
        .iter()
        .flat_map(|app| subdirs(app))
        .flat_map(|ws| read_hook_sessions(&ws.join("claude-hook-sessions.json")))
        .collect()
}

/// The Claude Code state a screen shows, `None` when it shows no Claude Code.
pub fn classify_screen(text: &str) -> Option<Status> {
    let lines: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    let tail = &lines[lines.len().saturating_sub(DIALOG_LINES)..];
    let dialog = |l: &&str| ["Do you want to proceed?", "Esc to cancel", "Enter to select"].iter().any(|m| l.contains(m));
    if tail.iter().any(dialog) {
        return Some(Status::NeedsInput);
    }
    let input = (1..lines.len()).rev().find(|&i| lines[i].starts_with('❯') && lines[i - 1].starts_with(RULE))?;
    let above = &lines[(input - 1).saturating_sub(SPINNER_LINES)..input - 1];
    Some(if above.iter().any(|l| is_spinner(l)) { Status::Running } else { Status::Idle })
}

/// `· Zigzagging… (43s · ↓ 3.6k tokens)` or `✻ Thinking… (esc to interrupt)`.
fn is_spinner(line: &str) -> bool {
    let timed = line.split_once("… (").is_some_and(|(_, rest)| rest.starts_with(|c: char| c.is_ascii_digit()));
    !line.starts_with(char::is_alphanumeric) && (timed || line.contains("esc to interrupt"))
}

/// Next state of the remote tabs: tabs of `tree` only, with the oldest due
/// screens read again. A failed read keeps the last state; one read without
/// Claude Code does too (full-screen dialogs hide the input box).
pub fn scan_tabs(
    tree: &TreeLayout,
    prev: &HashMap<String, RemoteTab>,
    cmux: &dyn Cmux,
    now: SystemTime,
) -> HashMap<String, RemoteTab> {
    let mut tabs: HashMap<String, RemoteTab> = HashMap::new();
    for (surface, tab) in &tree.tabs {
        let Some(title) = remote_title(&tab.title) else { continue };
        let old = prev.get(surface);
        tabs.insert(
            surface.clone(),
            RemoteTab {
                workspace_id: tab.workspace_id.clone(),
                title: title.to_string(),
                checked_at: old.and_then(|o| o.checked_at),
                screen: old.and_then(|o| o.screen),
                misses: old.map_or(0, |o| o.misses),
            },
        );
    }
    let due = |t: &RemoteTab| t.checked_at.is_none_or(|c| now.duration_since(c).map_or(true, |d| d >= SCAN_INTERVAL));
    let mut queue: Vec<(Option<SystemTime>, String)> =
        tabs.iter().filter(|(_, t)| due(t)).map(|(id, t)| (t.checked_at, id.clone())).collect();
    queue.sort();
    for (_, surface) in queue.into_iter().take(MAX_READS) {
        let Some(tab) = tabs.get_mut(&surface) else { continue };
        tab.checked_at = Some(now);
        let Ok(text) = cmux.read_screen(&tab.workspace_id, &surface, SCREEN_LINES) else { continue };
        match classify_screen(&text) {
            Some(status) => (tab.screen, tab.misses) = (Some(status), 0),
            None if tab.screen.is_some() && tab.misses == 0 => tab.misses = 1,
            None => tab.screen = None,
        }
    }
    tabs
}

/// The (cmux ref, hook record) of each tab running Claude Code. The session
/// id comes from the tab, so a card outlives the relay record appearing or a
/// new `claude` in the same tab.
pub fn remote_sessions(tabs: &HashMap<String, RemoteTab>, relay: &[HookRecord]) -> (Vec<CmuxSessionRef>, Vec<HookRecord>) {
    let updated = |h: &HookRecord| h.updated_at.as_deref().and_then(parse_time);
    let mut surfaces: Vec<&String> = tabs.keys().collect();
    surfaces.sort();
    let (mut refs, mut hooks) = (Vec::new(), Vec::new());
    for surface in surfaces {
        let tab = &tabs[surface];
        let Some(screen) = tab.screen else { continue };
        let record = relay.iter().filter(|h| h.surface_id.eq_ignore_ascii_case(surface)).max_by_key(|h| updated(h));
        let session_id = format!("remote-{surface}");
        refs.push(CmuxSessionRef {
            session_id: session_id.clone(),
            workspace_id: tab.workspace_id.clone(),
            surface_id: surface.clone(),
            cwd: PathBuf::new(),
            pid_exists: true,
            updated: None,
            title: Some(tab.title.clone()),
        });
        let status = merged_status(screen, record, tab.checked_at);
        let (lifecycle, event) = match status {
            Status::NeedsInput => ("needsInput", Some("Notification")),
            Status::Running => ("running", None),
            _ => ("idle", None),
        };
        hooks.push(HookRecord {
            session_id,
            pid: None,
            workspace_id: tab.workspace_id.clone(),
            surface_id: surface.clone(),
            cwd: PathBuf::new(),
            agent_lifecycle: Some(lifecycle.to_string()),
            hook_event_name: event.map(String::from),
            last_body: record.and_then(|h| h.last_body.clone()),
            transcript_path: None,
            started_at: record.and_then(|h| h.started_at.clone()),
            updated_at: record.and_then(|h| h.updated_at.clone()),
        });
    }
    (refs, hooks)
}

/// The relay is pushed, the screen polled, so the newer one wins. An older
/// relay record only keeps `running` through a short spinner gap, and the
/// screen corrects what a relay can get wrong: the permission `Notification`
/// comes seconds late and stays after the prompt is answered.
fn merged_status(screen: Status, record: Option<&HookRecord>, checked_at: Option<SystemTime>) -> Status {
    let Some(h) = record else { return screen };
    let relay = resolve_status(Some(h), None, true, None);
    let at = h.updated_at.as_deref().and_then(parse_time);
    let age = at.zip(checked_at).map(|(at, checked)| checked.duration_since(at));
    match age {
        Some(Err(_)) if relay != Status::Unknown => relay,
        Some(Ok(age)) if relay == Status::Running && screen == Status::Idle && age < RUNNING_GRACE => relay,
        _ => screen,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::cmux::{parse_tree, FakeCmux};
    use std::time::UNIX_EPOCH;

    const BOX: &str = "────────────────────────────────────────";

    fn at(secs: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(secs)
    }

    fn idle_screen() -> String {
        format!("some reply\n{BOX}\n❯ \n{BOX}\n  ⏵⏵ auto mode on (shift+tab to cycle)")
    }

    fn working_screen() -> String {
        format!("· Zigzagging… (43s · ↓ 3.6k tokens)\n  ⎿  Tip: paste images\n{BOX}\n❯ \n{BOX}\n  ⏵⏵ auto mode on")
    }

    fn tab(screen: Option<Status>, checked: u64) -> RemoteTab {
        RemoteTab { workspace_id: "WS".into(), title: "task".into(), checked_at: Some(at(checked)), screen, misses: 0 }
    }

    fn relay(surface: &str, lifecycle: &str, event: &str, updated: u64) -> HookRecord {
        HookRecord {
            session_id: "real-id".into(),
            pid: None,
            workspace_id: "WS".into(),
            surface_id: surface.into(),
            cwd: PathBuf::new(),
            agent_lifecycle: Some(lifecycle.into()),
            hook_event_name: Some(event.into()),
            last_body: Some("hello".into()),
            transcript_path: None,
            started_at: None,
            updated_at: Some(updated.to_string()),
        }
    }

    fn tree() -> TreeLayout {
        parse_tree(concat!(
            "window AAAAAAAA-0000-0000-0000-000000000001\n",
            "├── workspace BBBBBBBB-0000-0000-0000-000000000001 \"[mosh] ws\"\n",
            "│   └── pane bbbbbbbb-0000-0000-0000-000000000001 [focused]\n",
            "│       ├── surface 00000000-0000-0000-0000-000000000001 [terminal] \"[mosh] first task\" tty=ttys001\n",
            "│       └── surface 00000000-0000-0000-0000-000000000002 [terminal] \"[ssh] second\" [selected] tty=ttys002\n",
            "└── workspace BBBBBBBB-0000-0000-0000-000000000002 \"local\"\n",
            "    └── pane bbbbbbbb-0000-0000-0000-000000000002\n",
            "        └── surface 00000000-0000-0000-0000-000000000003 [terminal] \"zsh\" tty=ttys003\n",
        ))
    }

    const S1: &str = "00000000-0000-0000-0000-000000000001";
    const S2: &str = "00000000-0000-0000-0000-000000000002";

    #[test]
    fn remote_title_strips_the_prefix_of_remote_tabs_only() {
        assert_eq!(remote_title("[mosh] my task"), Some("my task"));
        assert_eq!(remote_title("[ssh] my task"), Some("my task"));
        assert_eq!(remote_title("my task [mosh]"), None);
        assert_eq!(remote_title("zsh"), None);
    }

    #[test]
    fn relay_hooks_are_read_from_every_workspace_folder() {
        let d = tempfile::tempdir().unwrap();
        for (ws, id) in [("WS1", "a"), ("WS2", "b")] {
            let dir = d.path().join("com.cmuxterm.app").join(ws);
            fs::create_dir_all(&dir).unwrap();
            let json = format!(r#"{{"sessions":{{"{id}":{{"sessionId":"{id}","surfaceId":"S","updatedAt":1.5}}}}}}"#);
            fs::write(dir.join("claude-hook-sessions.json"), json).unwrap();
        }
        let mut ids: Vec<String> = read_relay_hooks(d.path()).into_iter().map(|h| h.session_id).collect();
        ids.sort();
        assert_eq!(ids, ["a", "b"]);
        assert!(read_relay_hooks(&d.path().join("missing")).is_empty());
    }

    #[test]
    fn screen_with_input_box_is_idle() {
        assert_eq!(classify_screen(&idle_screen()), Some(Status::Idle));
    }

    #[test]
    fn screen_with_spinner_above_the_box_is_running() {
        assert_eq!(classify_screen(&working_screen()), Some(Status::Running));
        let old = format!("✻ Thinking… (esc to interrupt)\n{BOX}\n❯ \n{BOX}");
        assert_eq!(classify_screen(&old), Some(Status::Running));
    }

    #[test]
    fn spinner_like_text_far_above_the_box_is_not_running() {
        let filler = "reply line\n".repeat(12);
        let s = format!("· Zigzagging… (43s\n{filler}{BOX}\n❯ \n{BOX}");
        assert_eq!(classify_screen(&s), Some(Status::Idle));
    }

    #[test]
    fn dialogs_need_input_without_an_input_box() {
        assert_eq!(classify_screen("Do you want to proceed?\n 1. Yes\n 2. No\n Esc to cancel"), Some(Status::NeedsInput));
        assert_eq!(classify_screen("Pick one\n 1. A\n Enter to select · ↑/↓ to navigate"), Some(Status::NeedsInput));
    }

    #[test]
    fn other_programs_are_not_claude() {
        assert_eq!(classify_screen("❯ ls\nCargo.toml src\n❯ "), None);
        assert_eq!(classify_screen(" NOR  96B  forks"), None);
        assert_eq!(classify_screen(""), None);
    }

    #[test]
    fn scan_finds_remote_tabs_and_skips_local_ones() {
        let mut cmux = FakeCmux::new(vec![]);
        cmux.screens.insert(S1.into(), Ok(idle_screen()));
        cmux.screens.insert(S2.into(), Ok("$ ".into()));
        let tabs = scan_tabs(&tree(), &HashMap::new(), &cmux, at(100));
        assert_eq!(tabs.len(), 2);
        assert_eq!(tabs[S1].title, "first task");
        assert_eq!(tabs[S1].workspace_id, "BBBBBBBB-0000-0000-0000-000000000001");
        assert_eq!(tabs[S1].screen, Some(Status::Idle));
        assert_eq!(tabs[S2].screen, None);
    }

    #[test]
    fn scan_reads_a_tab_only_once_per_interval() {
        let cmux = FakeCmux::new(vec![]);
        let first = scan_tabs(&tree(), &HashMap::new(), &cmux, at(100));
        scan_tabs(&tree(), &first, &cmux, at(102));
        assert_eq!(cmux.read_lines.lock().unwrap().len(), 2);
        scan_tabs(&tree(), &first, &cmux, at(105));
        assert_eq!(cmux.read_lines.lock().unwrap().len(), 4);
    }

    #[test]
    fn scan_caps_reads_and_serves_the_oldest_first() {
        let mut lines = String::from("window AAAAAAAA-0000-0000-0000-000000000001\n");
        lines.push_str("workspace BBBBBBBB-0000-0000-0000-000000000001 \"w\"\n");
        let mut prev = HashMap::new();
        for i in 0..5u64 {
            let id = format!("00000000-0000-0000-0000-00000000000{i}");
            lines.push_str(&format!("surface {id} [terminal] \"[mosh] t{i}\"\n"));
            prev.insert(id, tab(Some(Status::Idle), 50 - i));
        }
        let cmux = FakeCmux::new(vec![]);
        let tabs = scan_tabs(&parse_tree(&lines), &prev, &cmux, at(100));
        assert_eq!(cmux.read_lines.lock().unwrap().len(), MAX_READS);
        // Checked longest ago: the highest numbers.
        assert_eq!(tabs["00000000-0000-0000-0000-000000000000"].checked_at, Some(at(50)));
        assert_eq!(tabs["00000000-0000-0000-0000-000000000004"].checked_at, Some(at(100)));
    }

    #[test]
    fn failed_read_keeps_the_last_state() {
        let mut cmux = FakeCmux::new(vec![]);
        cmux.read_screen_result = Err("timed out".into());
        let prev = HashMap::from([(S1.to_string(), tab(Some(Status::Running), 1))]);
        let tabs = scan_tabs(&tree(), &prev, &cmux, at(100));
        assert_eq!(tabs[S1].screen, Some(Status::Running));
        assert_eq!(tabs[S1].checked_at, Some(at(100)));
    }

    #[test]
    fn claude_is_gone_only_after_two_reads_in_a_row() {
        let cmux = FakeCmux::new(vec![]);
        let prev = HashMap::from([(S1.to_string(), tab(Some(Status::Idle), 1))]);
        let one = scan_tabs(&tree(), &prev, &cmux, at(100));
        assert_eq!(one[S1].screen, Some(Status::Idle));
        let two = scan_tabs(&tree(), &one, &cmux, at(110));
        assert_eq!(two[S1].screen, None);
    }

    #[test]
    fn closed_tabs_leave_the_scan() {
        let cmux = FakeCmux::new(vec![]);
        let prev = HashMap::from([("GONE".to_string(), tab(Some(Status::Idle), 1))]);
        assert!(!scan_tabs(&tree(), &prev, &cmux, at(100)).contains_key("GONE"));
    }

    #[test]
    fn tabs_without_claude_give_no_session() {
        let tabs = HashMap::from([(S1.to_string(), tab(None, 100))]);
        let (refs, hooks) = remote_sessions(&tabs, &[relay(S1, "idle", "Stop", 90)]);
        assert!(refs.is_empty() && hooks.is_empty());
    }

    #[test]
    fn session_is_keyed_by_tab_and_marked_alive() {
        let tabs = HashMap::from([(S1.to_string(), tab(Some(Status::Idle), 100))]);
        let (refs, hooks) = remote_sessions(&tabs, &[]);
        assert_eq!(refs[0].session_id, format!("remote-{S1}"));
        assert_eq!(refs[0].surface_id, S1);
        assert_eq!(refs[0].workspace_id, "WS");
        assert_eq!(refs[0].title.as_deref(), Some("task"));
        assert!(refs[0].pid_exists);
        assert_eq!(hooks[0].session_id, refs[0].session_id);
        assert_eq!(hooks[0].agent_lifecycle.as_deref(), Some("idle"));
    }

    #[test]
    fn newest_relay_record_of_the_tab_gives_the_summary() {
        let tabs = HashMap::from([(S1.to_string(), tab(Some(Status::Idle), 100))]);
        let old = HookRecord { last_body: Some("old".into()), ..relay(S1, "idle", "Stop", 80) };
        let other = HookRecord { last_body: Some("other tab".into()), ..relay(S2, "idle", "Stop", 95) };
        let (_, hooks) = remote_sessions(&tabs, &[old, relay(S1, "idle", "Stop", 90), other]);
        assert_eq!(hooks[0].last_body.as_deref(), Some("hello"));
    }

    fn lifecycle(screen: Status, r: HookRecord) -> (Option<String>, Option<String>) {
        let tabs = HashMap::from([(S1.to_string(), tab(Some(screen), 100))]);
        let (_, hooks) = remote_sessions(&tabs, &[r]);
        (hooks[0].agent_lifecycle.clone(), hooks[0].hook_event_name.clone())
    }

    fn status_of(screen: Status, r: HookRecord) -> Status {
        let tabs = HashMap::from([(S1.to_string(), tab(Some(screen), 100))]);
        let (_, hooks) = remote_sessions(&tabs, &[r]);
        crate::status::resolve_status(hooks.first(), None, true, None)
    }

    #[test]
    fn relay_newer_than_the_screen_wins() {
        assert_eq!(status_of(Status::Running, relay(S1, "idle", "Stop", 103)), Status::Idle);
    }

    #[test]
    fn screen_catches_a_prompt_before_the_delayed_notification() {
        assert_eq!(status_of(Status::NeedsInput, relay(S1, "running", "PreToolUse", 90)), Status::NeedsInput);
    }

    #[test]
    fn screen_clears_a_stale_needs_input() {
        assert_eq!(status_of(Status::Running, relay(S1, "needsInput", "Notification", 90)), Status::Running);
        assert_eq!(status_of(Status::Idle, relay(S1, "needsInput", "Notification", 90)), Status::Idle);
    }

    #[test]
    fn screen_finds_work_the_relay_missed() {
        assert_eq!(status_of(Status::Running, relay(S1, "idle", "Stop", 90)), Status::Running);
        assert_eq!(lifecycle(Status::Running, relay(S1, "idle", "Stop", 90)).1, None);
    }

    #[test]
    fn missing_spinner_does_not_stop_a_running_relay_session() {
        assert_eq!(status_of(Status::Idle, relay(S1, "running", "PreToolUse", 90)), Status::Running);
    }

    #[test]
    fn screen_ends_a_running_session_whose_stop_was_lost() {
        assert_eq!(status_of(Status::Idle, relay(S1, "running", "PreToolUse", 60)), Status::Idle);
    }

    #[test]
    fn background_work_stays_idle() {
        assert_eq!(status_of(Status::Idle, relay(S1, "backgroundWorkPending", "Stop", 90)), Status::Idle);
    }
}
