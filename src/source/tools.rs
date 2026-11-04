//! Tool calls a session is running right now, read from its transcript.
//!
//! Foreground: a main-chain assistant `tool_use` block counts as running
//! until a `tool_result` with its id arrives; an `end_turn` reply, an
//! interrupt or a new prompt clears them all. `Agent` / `Task` calls are
//! subagents (see [`crate::source::subagents`]) and are left out. The hint
//! is the Bash `description` (else the first command line, at most
//! [`HINT_MAX_CHARS`] characters), the file name of Read / Edit / Write /
//! NotebookEdit, the Grep / Glob pattern, and empty for other tools.
//!
//! Background: a main-chain `tool_result` whose `toolUseResult` carries
//! `backgroundTaskId` (Bash, `run_in_background` or backgrounded later) or
//! `taskId` with `timeoutMs` / `persistent` (Monitor) starts a task. It ends
//! with a `<task-notification>` naming its `<task-id>` (queued
//! `queue-operation`, `attachment` or `user` records; any status) or a
//! `TaskStop` / `KillShell` call naming it. The output files under
//! `/tmp/claude-<uid>/<project>/<session>/tasks/` carry no exit marker, so
//! they only extend a task's life: an open task counts while it was launched
//! within [`BG_LAUNCH_FRESH_SECS`] or its output file was written within
//! [`BG_OUTPUT_FRESH_SECS`]; a non-persistent Monitor never outlives its
//! `timeoutMs`. A finished session counts nothing.
//!
//! Each transcript is read incrementally: the first scan starts
//! [`INITIAL_WINDOW`] bytes before the end, later scans read only the
//! appended whole lines (a half-written last line waits for the next round);
//! a file that shrank is read again from scratch.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde_json::Value;

use crate::model::Status;

/// How far before the end of a transcript the first scan starts.
pub const INITIAL_WINDOW: u64 = 2 * 1024 * 1024;
/// Longest Bash hint, in characters (then `…`).
pub const HINT_MAX_CHARS: usize = 30;
/// An Idle session's pending tool call older than this is stale (ignored).
pub const TOOL_IDLE_STALE_SECS: u64 = 120;
/// An open background task counts while launched within this…
pub const BG_LAUNCH_FRESH_SECS: u64 = 30 * 60;
/// …or while its output file was written within this.
pub const BG_OUTPUT_FRESH_SECS: u64 = 5 * 60;
/// Open background tasks older than this are forgotten.
const BG_FORGET_SECS: u64 = 24 * 3600;
/// Tool names that are subagents, shown by the subagent rows instead.
const SUBAGENT_TOOLS: [&str; 2] = ["Agent", "Task"];

/// A foreground tool call that has no result yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunningTool {
    /// Tool name (`Bash`, `Read`, …).
    pub name: String,
    /// Short argument hint, one line; empty when the tool has none.
    pub hint: String,
    /// When the assistant record that called it was written.
    pub started_at: Option<SystemTime>,
}

/// What a session is running besides its subagents.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolActivity {
    /// The oldest foreground call still waiting for its result.
    pub current: Option<RunningTool>,
    /// Further foreground calls waiting (parallel calls).
    pub more: usize,
    /// Background Bash / Monitor tasks still running.
    pub background: usize,
}

impl ToolActivity {
    /// Every running call: foreground and background.
    pub fn count(&self) -> usize {
        usize::from(self.current.is_some()) + self.more + self.background
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BgKind {
    Bash,
    Monitor { timeout: Option<Duration>, persistent: bool },
}

#[derive(Debug, Clone)]
struct BgTask {
    kind: BgKind,
    launched: Option<SystemTime>,
}

/// The parse state of one transcript: pending calls in call order and the
/// open background tasks by task id.
#[derive(Debug, Clone, Default)]
pub struct ScanState {
    pending: Vec<(String, RunningTool)>,
    tasks: HashMap<String, BgTask>,
    /// Bytes of the file consumed (always at a line start).
    offset: u64,
}

fn time_of(v: &Value) -> Option<SystemTime> {
    v.get("timestamp").and_then(Value::as_str).and_then(crate::merge::parse_time)
}

fn first_line(s: &str) -> &str {
    s.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("")
}

fn cut_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect::<String>().trim_end().to_string();
    out.push('…');
    out
}

/// The short argument hint of a call (see the module docs).
pub fn hint(name: &str, input: &Value) -> String {
    let field = |k: &str| input.get(k).and_then(Value::as_str).map(first_line).filter(|s| !s.is_empty());
    let base = |k: &str| field(k).map(|p| p.rsplit(['/', '\\']).next().unwrap_or(p).to_string());
    match name {
        "Bash" => field("description").or_else(|| field("command")).map(|s| cut_chars(s, HINT_MAX_CHARS)),
        "Read" | "Edit" | "Write" | "MultiEdit" => base("file_path"),
        "NotebookEdit" => base("notebook_path"),
        "Grep" | "Glob" => field("pattern").map(|s| cut_chars(s, HINT_MAX_CHARS)),
        _ => None,
    }
    .unwrap_or_default()
}

/// Every `<task-id>…</task-id>` inside a `<task-notification>` text.
fn notified_ids(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    if !text.contains("<task-notification>") {
        return out;
    }
    let mut rest = text;
    while let Some(i) = rest.find("<task-id>") {
        rest = &rest[i + "<task-id>".len()..];
        let Some(end) = rest.find("</task-id>") else { break };
        let id = rest[..end].trim();
        if !id.is_empty() && id.len() <= 64 && !id.contains('<') {
            out.push(id);
        }
        rest = &rest[end..];
    }
    out
}

impl ScanState {
    /// Parses whole transcript lines (malformed lines are skipped).
    pub fn feed(&mut self, text: &str) {
        for line in text.lines() {
            // cheap pre-filter: only records that can change the state
            if !(line.contains("tool_use")
                || line.contains("task-notification")
                || line.contains("stop_reason")
                || line.contains(r#""type":"user""#))
            {
                continue;
            }
            if let Ok(v) = serde_json::from_str::<Value>(line) {
                self.record(&v, line);
            }
        }
    }

    fn record(&mut self, v: &Value, line: &str) {
        let kind = v.get("type").and_then(Value::as_str).unwrap_or("");
        // completions can be queued in records of these kinds only (a marker
        // quoted in a tool input or reply must not end a task)
        if matches!(kind, "queue-operation" | "attachment" | "user") {
            for id in notified_ids(line) {
                self.tasks.remove(id);
            }
        }
        if v.get("isSidechain").and_then(Value::as_bool) == Some(true) {
            return;
        }
        match kind {
            "assistant" => self.assistant(v),
            "user" if v.get("isMeta").and_then(Value::as_bool) != Some(true) => self.user(v),
            _ => {}
        }
    }

    fn assistant(&mut self, v: &Value) {
        if matches!(v.pointer("/message/stop_reason").and_then(Value::as_str), Some("end_turn" | "stop_sequence")) {
            self.pending.clear();
            return;
        }
        let Some(blocks) = v.pointer("/message/content").and_then(Value::as_array) else { return };
        let at = time_of(v);
        for b in blocks.iter().filter(|b| b.get("type").and_then(Value::as_str) == Some("tool_use")) {
            let (Some(id), Some(name)) = (b.get("id").and_then(Value::as_str), b.get("name").and_then(Value::as_str)) else {
                continue;
            };
            let input = b.get("input").cloned().unwrap_or(Value::Null);
            if matches!(name, "TaskStop" | "KillShell" | "KillBash") {
                for k in ["task_id", "shell_id", "bash_id"] {
                    if let Some(t) = input.get(k).and_then(Value::as_str) {
                        self.tasks.remove(t);
                    }
                }
            }
            if SUBAGENT_TOOLS.contains(&name) || self.pending.iter().any(|(p, _)| p == id) {
                continue;
            }
            let tool = RunningTool { name: name.to_string(), hint: hint(name, &input), started_at: at };
            self.pending.push((id.to_string(), tool));
        }
    }

    fn user(&mut self, v: &Value) {
        match v.pointer("/message/content") {
            Some(Value::String(s)) => {
                // a new prompt (not a delivered notification) ends the old calls
                if !s.trim_start().starts_with("<task-notification>") {
                    self.pending.clear();
                }
            }
            Some(Value::Array(items)) => {
                for it in items {
                    match it.get("type").and_then(Value::as_str) {
                        Some("tool_result") => {
                            if let Some(id) = it.get("tool_use_id").and_then(Value::as_str) {
                                self.pending.retain(|(p, _)| p != id);
                            }
                        }
                        Some("text") => {
                            let t = it.get("text").and_then(Value::as_str).unwrap_or("");
                            if t.starts_with("[Request interrupted by user") {
                                self.pending.clear();
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
        let Some(r) = v.get("toolUseResult").filter(|r| r.is_object()) else { return };
        let launched = time_of(v);
        if let Some(id) = r.get("backgroundTaskId").and_then(Value::as_str) {
            self.tasks.insert(id.to_string(), BgTask { kind: BgKind::Bash, launched });
        } else if let Some(id) = r.get("taskId").and_then(Value::as_str) {
            // Monitor results carry its timeout / persistence; other task
            // kinds (workflows, …) are not counted
            let timeout = r.get("timeoutMs").and_then(Value::as_u64).map(Duration::from_millis);
            let persistent = r.get("persistent").and_then(Value::as_bool);
            if timeout.is_some() || persistent.is_some() {
                let kind = BgKind::Monitor { timeout, persistent: persistent.unwrap_or(false) };
                self.tasks.insert(id.to_string(), BgTask { kind, launched });
            }
        }
    }

    /// Drops open tasks too old to matter, so the map stays small.
    fn forget_old(&mut self, now: SystemTime) {
        let limit = Duration::from_secs(BG_FORGET_SECS);
        self.tasks.retain(|_, t| t.launched.is_none_or(|l| now.duration_since(l).unwrap_or_default() <= limit));
    }

    /// What runs at `now` for a session in `status`; `output_mtime` gives the
    /// last write of a task's output file, if any.
    pub fn activity(&self, status: Status, now: SystemTime, output_mtime: &dyn Fn(&str) -> Option<SystemTime>) -> ToolActivity {
        if status == Status::Done {
            return ToolActivity::default();
        }
        let age = |t: SystemTime| now.duration_since(t).unwrap_or_default();
        let stale_idle = |t: &RunningTool| {
            status == Status::Idle && t.started_at.is_some_and(|s| age(s) > Duration::from_secs(TOOL_IDLE_STALE_SECS))
        };
        let live: Vec<&RunningTool> = self.pending.iter().map(|(_, t)| t).filter(|t| !stale_idle(t)).collect();
        let background = self
            .tasks
            .iter()
            .filter(|(id, t)| {
                if let (BgKind::Monitor { timeout: Some(to), persistent: false }, Some(l)) = (t.kind, t.launched) {
                    if age(l) > to {
                        return false;
                    }
                }
                let launched_fresh = t.launched.is_some_and(|l| age(l) <= Duration::from_secs(BG_LAUNCH_FRESH_SECS));
                let persistent = matches!(t.kind, BgKind::Monitor { persistent: true, .. });
                let output_fresh = || output_mtime(id).is_some_and(|m| age(m) <= Duration::from_secs(BG_OUTPUT_FRESH_SECS));
                launched_fresh || persistent || output_fresh()
            })
            .count();
        ToolActivity { current: live.first().map(|t| (*t).clone()), more: live.len().saturating_sub(1), background }
    }
}

/// `/tmp/claude-<uid>`: where Claude Code keeps background task outputs.
pub fn default_tasks_root() -> Option<PathBuf> {
    use std::os::unix::fs::MetadataExt;
    let uid = std::fs::metadata(dirs::home_dir()?).ok()?.uid();
    Some(PathBuf::from(format!("/tmp/claude-{uid}")))
}

/// Per transcript parse states, read incrementally (see the module docs).
#[derive(Debug, Clone)]
pub struct ToolCache {
    tasks_root: Option<PathBuf>,
    entries: HashMap<PathBuf, ScanState>,
}

impl Default for ToolCache {
    fn default() -> Self {
        ToolCache::with_tasks_root(default_tasks_root())
    }
}

impl ToolCache {
    /// A cache that looks for task output files under `root` (none: the
    /// launch time alone decides).
    pub fn with_tasks_root(root: Option<PathBuf>) -> Self {
        ToolCache { tasks_root: root, entries: HashMap::new() }
    }

    /// Reads what was appended to `transcript` and returns what runs now.
    pub fn scan(&mut self, transcript: &Path, status: Status, now: SystemTime) -> ToolActivity {
        let Ok(len) = std::fs::metadata(transcript).map(|m| m.len()) else {
            self.entries.remove(transcript);
            return ToolActivity::default();
        };
        let state = self.entries.entry(transcript.to_path_buf()).or_default();
        if len < state.offset {
            *state = ScanState::default();
        }
        if len > state.offset {
            if let Some((text, consumed)) = read_lines_from(transcript, state.offset, len) {
                state.feed(&text);
                state.offset = consumed;
            }
        }
        state.forget_old(now);
        let tasks_dir = self.tasks_root.as_ref().and_then(|root| {
            let project = transcript.parent()?.file_name()?;
            let session = transcript.file_stem()?;
            Some(root.join(project).join(session).join("tasks"))
        });
        let mtime = |id: &str| -> Option<SystemTime> {
            let dir = tasks_dir.as_ref()?;
            if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
                return None;
            }
            std::fs::metadata(dir.join(format!("{id}.output"))).and_then(|m| m.modified()).ok()
        };
        state.activity(status, now, &mtime)
    }

    /// Forgets every transcript not in `paths`.
    pub fn retain(&mut self, paths: &[PathBuf]) {
        self.entries.retain(|p, _| paths.contains(p));
    }

    /// Number of transcripts held.
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

/// The whole lines of `path` from `offset` (a line start; `0` on a first
/// read of a big file means the last [`INITIAL_WINDOW`] bytes, cut to a line
/// start) up to `len`, and the offset after the last one.
fn read_lines_from(path: &Path, offset: u64, len: u64) -> Option<(String, u64)> {
    let mut f = File::open(path).ok()?;
    let (start, skip_partial) =
        if offset == 0 && len > INITIAL_WINDOW { (len - INITIAL_WINDOW, true) } else { (offset, false) };
    f.seek(SeekFrom::Start(start)).ok()?;
    let mut buf = Vec::with_capacity((len - start) as usize);
    f.take(len - start).read_to_end(&mut buf).ok()?;
    let mut from = 0;
    if skip_partial {
        from = buf.iter().position(|&b| b == b'\n').map_or(buf.len(), |i| i + 1);
    }
    let end = buf.iter().rposition(|&b| b == b'\n').map_or(from, |i| i + 1).max(from);
    let text = String::from_utf8_lossy(&buf[from..end]).into_owned();
    Some((text, start + end as u64))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, UNIX_EPOCH};

    /// 2026-11-04T05:00:00.000Z
    const T0_MS: u64 = 1_793_768_400_000;

    fn at(secs: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_millis(T0_MS + secs * 1000)
    }

    /// ISO timestamp `secs` after T0 (under one hour).
    fn ts(secs: u64) -> String {
        format!("2026-11-04T05:{:02}:{:02}.000Z", secs / 60, secs % 60)
    }

    fn tool_use(id: &str, name: &str, input: &str, secs: u64) -> String {
        format!(
            r#"{{"type":"assistant","isSidechain":false,"timestamp":"{}","message":{{"stop_reason":"tool_use","content":[{{"type":"tool_use","id":"{id}","name":"{name}","input":{input}}}]}}}}"#,
            ts(secs)
        )
    }

    fn side_tool_use(id: &str, name: &str, secs: u64) -> String {
        tool_use(id, name, "{}", secs).replace(r#""isSidechain":false"#, r#""isSidechain":true"#)
    }

    fn result(id: &str, secs: u64, tur: &str) -> String {
        format!(
            r#"{{"type":"user","isSidechain":false,"timestamp":"{}","message":{{"content":[{{"type":"tool_result","tool_use_id":"{id}","content":"ok"}}]}},"toolUseResult":{tur}}}"#,
            ts(secs)
        )
    }

    fn bash_bg(tool: &str, task: &str, secs: u64) -> Vec<String> {
        vec![
            tool_use(tool, "Bash", r#"{"command":"sleep 1","description":"wait","run_in_background":true}"#, secs),
            result(tool, secs, &format!(r#"{{"stdout":"","backgroundTaskId":"{task}"}}"#)),
        ]
    }

    fn monitor(tool: &str, task: &str, secs: u64, timeout_ms: u64, persistent: bool) -> Vec<String> {
        vec![
            tool_use(tool, "Monitor", r#"{"command":"tail -f log","description":"watch"}"#, secs),
            result(tool, secs, &format!(r#"{{"taskId":"{task}","timeoutMs":{timeout_ms},"persistent":{persistent}}}"#)),
        ]
    }

    fn notification(task: &str, secs: u64) -> String {
        format!(
            r#"{{"type":"queue-operation","operation":"enqueue","timestamp":"{}","content":"<task-notification>\n<task-id>{task}</task-id>\n<status>completed</status>\n</task-notification>"}}"#,
            ts(secs)
        )
    }

    fn notification_attachment(task: &str, secs: u64) -> String {
        format!(
            r#"{{"type":"attachment","isSidechain":false,"timestamp":"{}","attachment":{{"type":"queued_command","prompt":"<task-notification><task-id>{task}</task-id><status>killed</status></task-notification>"}}}}"#,
            ts(secs)
        )
    }

    fn end_turn(secs: u64) -> String {
        format!(
            r#"{{"type":"assistant","isSidechain":false,"timestamp":"{}","message":{{"stop_reason":"end_turn","content":[{{"type":"text","text":"done"}}]}}}}"#,
            ts(secs)
        )
    }

    fn prompt(text: &str, secs: u64) -> String {
        format!(r#"{{"type":"user","isSidechain":false,"timestamp":"{}","message":{{"content":"{text}"}}}}"#, ts(secs))
    }

    fn interrupted(secs: u64) -> String {
        format!(
            r#"{{"type":"user","isSidechain":false,"timestamp":"{}","message":{{"content":[{{"type":"text","text":"[Request interrupted by user for tool use]"}}]}}}}"#,
            ts(secs)
        )
    }

    fn activity(lines: &[String], status: Status, now_secs: u64) -> ToolActivity {
        let mut st = ScanState::default();
        st.feed(&(lines.join("\n") + "\n"));
        st.activity(status, at(now_secs), &|_| None)
    }

    fn current(lines: &[String], status: Status, now_secs: u64) -> Option<(String, String, usize)> {
        let a = activity(lines, status, now_secs);
        a.current.map(|c| (c.name, c.hint, a.more))
    }

    fn cur(name: &str, hint: &str, more: usize) -> Option<(String, String, usize)> {
        Some((name.to_string(), hint.to_string(), more))
    }

    #[test]
    fn current_tool_table() {
        use Status::*;
        let bash_desc = tool_use("t1", "Bash", r#"{"command":"cargo test --quiet","description":"Run the tests"}"#, 10);
        let bash_cmd = tool_use("t1", "Bash", r#"{"command":"cargo build --release --locked && ls target/release\nsecond line"}"#, 10);
        let read = tool_use("t1", "Read", r#"{"file_path":"/home/user/proj/src/main.rs"}"#, 10);
        let edit = tool_use("t1", "Edit", r#"{"file_path":"/home/user/proj/README.md","old_string":"a","new_string":"b"}"#, 10);
        let grep = tool_use("t1", "Grep", r#"{"pattern":"fn main","path":"src"}"#, 10);
        let glob = tool_use("t1", "Glob", r#"{"pattern":"**/*.rs"}"#, 10);
        let fetch = tool_use("t1", "WebFetch", r#"{"url":"https://example.com","prompt":"x"}"#, 10);
        let agent = tool_use("t1", "Agent", r#"{"description":"explore","prompt":"x"}"#, 10);
        let task = tool_use("t1", "Task", r#"{"description":"explore","prompt":"x"}"#, 10);
        let second = tool_use("t2", "Read", r#"{"file_path":"/home/user/proj/b.rs"}"#, 11);
        let blob = r#"{"type":"api-request-blob","message":{"content":[{"type":"tool_use","id":"t9","name":"Bash","input":{"command":"x"}}]}}"#.to_string();
        let cases: Vec<(Vec<String>, Status, u64, Option<(String, String, usize)>, &str)> = vec![
            (vec![bash_desc.clone()], Running, 22, cur("Bash", "Run the tests", 0), "bash: description"),
            (vec![bash_cmd.clone()], Running, 22, cur("Bash", "cargo build --release --locked…", 0), "bash: first command line, 30 chars"),
            (vec![read.clone()], Running, 22, cur("Read", "main.rs", 0), "read: basename"),
            (vec![edit.clone()], Running, 22, cur("Edit", "README.md", 0), "edit: basename"),
            (vec![grep.clone()], Running, 22, cur("Grep", "fn main", 0), "grep: pattern"),
            (vec![glob.clone()], Running, 22, cur("Glob", "**/*.rs", 0), "glob: pattern"),
            (vec![fetch.clone()], Running, 22, cur("WebFetch", "", 0), "others: name only"),
            (vec![bash_desc.clone(), result("t1", 12, "{}")], Running, 22, None, "result arrived"),
            (vec![agent.clone()], Running, 22, None, "Agent is a subagent, not a tool row"),
            (vec![task.clone()], Running, 22, None, "Task is a subagent, not a tool row"),
            (vec![read.clone(), second.clone()], Running, 22, cur("Read", "main.rs", 1), "parallel: oldest first, +1"),
            (vec![read.clone(), second.clone(), result("t1", 12, "{}")], Running, 22, cur("Read", "b.rs", 0), "parallel: first done"),
            (vec![side_tool_use("t1", "Bash", 10)], Running, 22, None, "sidechain skipped"),
            (vec![bash_desc.clone(), end_turn(12)], Running, 22, None, "turn ended"),
            (vec![bash_desc.clone(), interrupted(12)], Running, 22, None, "interrupted"),
            (vec![bash_desc.clone(), prompt("next please", 12)], Running, 22, None, "new prompt"),
            (vec![bash_desc.clone()], NeedsInput, 500, cur("Bash", "Run the tests", 0), "permission prompt keeps it"),
            (vec![bash_desc.clone()], Idle, 40, cur("Bash", "Run the tests", 0), "idle but fresh"),
            (vec![bash_desc.clone()], Idle, 10 + 121, None, "idle and stale"),
            (vec![bash_desc.clone()], Done, 22, None, "done shows nothing"),
            (vec![blob, "not json".into(), r#"{"type":"assistant""#.into()], Running, 22, None, "blobs and broken lines"),
        ];
        for (lines, status, now, want, why) in cases {
            assert_eq!(current(&lines, status, now), want, "{why}");
        }
        let a = activity(&[bash_desc], Running, 22);
        assert_eq!(a.current.unwrap().started_at, Some(at(10)), "started at the assistant record");
    }

    #[test]
    fn background_task_table() {
        use Status::*;
        let mut stop = vec![tool_use("k1", "TaskStop", r#"{"task_id":"bk1"}"#, 30)];
        stop.splice(0..0, bash_bg("t1", "bk1", 10));
        let mut kill = vec![tool_use("k1", "KillShell", r#"{"shell_id":"bk1"}"#, 30)];
        kill.splice(0..0, bash_bg("t1", "bk1", 10));
        let with = |mut a: Vec<String>, b: Vec<String>| {
            a.extend(b);
            a
        };
        let workflow = vec![
            tool_use("w1", "Workflow", "{}", 10),
            result("w1", 10, r#"{"status":"async_launched","taskId":"bw1","taskType":"local_workflow"}"#),
        ];
        let side_launch = bash_bg("t1", "bk1", 10)
            .into_iter()
            .map(|l| l.replace(r#""isSidechain":false"#, r#""isSidechain":true"#))
            .collect::<Vec<_>>();
        let quoted = tool_use("q1", "Bash", r#"{"command":"echo '<task-notification><task-id>bk1</task-id>'"}"#, 20);
        let cases: Vec<(Vec<String>, Status, u64, usize, &str)> = vec![
            (bash_bg("t1", "bk1", 10), Running, 60, 1, "bash in the background"),
            (bash_bg("t1", "bk1", 10), Idle, 60, 1, "idle session, task still runs"),
            (with(bash_bg("t1", "bk1", 10), vec![notification("bk1", 50)]), Running, 60, 0, "notified (queue)"),
            (with(bash_bg("t1", "bk1", 10), vec![notification_attachment("bk1", 50)]), Running, 60, 0, "notified (attachment)"),
            (with(bash_bg("t1", "bk1", 10), vec![notification("other", 50)]), Running, 60, 1, "another task notified"),
            (stop, Running, 60, 0, "TaskStop"),
            (kill, Running, 60, 0, "KillShell"),
            (with(bash_bg("t1", "bk1", 10), bash_bg("t2", "bk2", 11)), Running, 60, 2, "two tasks"),
            (bash_bg("t1", "bk1", 10), Running, 10 + 30 * 60 + 1, 0, "launched over 30 min ago, no fresh output"),
            (monitor("m1", "bm1", 10, 600_000, false), Running, 100, 1, "monitor within its timeout"),
            (monitor("m1", "bm1", 10, 600_000, false), Running, 10 + 601, 0, "monitor past its timeout"),
            (monitor("m1", "bm1", 10, 600_000, true), Running, 10 + 1200, 1, "persistent monitor"),
            (workflow, Running, 60, 0, "workflows are not counted"),
            (side_launch, Running, 60, 0, "sidechain launches belong to the subagent"),
            (with(bash_bg("t1", "bk1", 10), vec![quoted]), Running, 60, 1, "a quoted marker in a tool input finishes nothing"),
            (bash_bg("t1", "bk1", 10), Done, 60, 0, "done session"),
        ];
        for (lines, status, now, want, why) in cases {
            assert_eq!(activity(&lines, status, now).background, want, "{why}");
        }
    }

    #[test]
    fn background_bash_output_freshness_extends_the_window() {
        let mut st = ScanState::default();
        st.feed(&(bash_bg("t1", "bk1", 10).join("\n") + "\n"));
        let now = 10 + 45 * 60;
        assert_eq!(st.activity(Status::Running, at(now), &|_| None).background, 0);
        let fresh = |id: &str| (id == "bk1").then(|| at(now - 60));
        assert_eq!(st.activity(Status::Running, at(now), &fresh).background, 1, "output written a minute ago");
        let old = |id: &str| (id == "bk1").then(|| at(now - 10 * 60));
        assert_eq!(st.activity(Status::Running, at(now), &old).background, 0, "output idle for ten minutes");
    }

    #[test]
    fn count_sums_foreground_and_background() {
        let mut lines = bash_bg("t1", "bk1", 10);
        lines.push(tool_use("t2", "Read", r#"{"file_path":"/home/user/a.rs"}"#, 12));
        lines.push(tool_use("t3", "Read", r#"{"file_path":"/home/user/b.rs"}"#, 12));
        let a = activity(&lines, Status::Running, 20);
        assert_eq!((a.more, a.background, a.count()), (1, 1, 3));
        assert_eq!(ToolActivity::default().count(), 0);
    }

    // --- the cache over real files ---

    fn layout(root: &Path) -> (PathBuf, PathBuf) {
        let proj = root.join("projects").join("-home-user-proj");
        std::fs::create_dir_all(&proj).unwrap();
        let tasks = root.join("tmp").join("-home-user-proj").join("sess-1").join("tasks");
        std::fs::create_dir_all(&tasks).unwrap();
        (proj.join("sess-1.jsonl"), tasks)
    }

    fn append(path: &Path, text: &str) {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path).unwrap();
        f.write_all(text.as_bytes()).unwrap();
    }

    #[test]
    fn cache_detects_background_tasks_from_transcript_and_output_files() {
        let dir = tempfile::tempdir().unwrap();
        let (t, tasks) = layout(dir.path());
        let mut cache = ToolCache::with_tasks_root(Some(dir.path().join("tmp")));
        append(&t, &(bash_bg("t1", "bk1", 10).join("\n") + "\n"));
        assert_eq!(cache.scan(&t, Status::Running, at(60)).background, 1);
        // long past the launch window: only a fresh output file keeps it
        let now = at(10 + 45 * 60);
        assert_eq!(cache.scan(&t, Status::Running, now).background, 0, "no output file");
        let out = tasks.join("bk1.output");
        std::fs::write(&out, "building\n").unwrap();
        let f = std::fs::File::options().write(true).open(&out).unwrap();
        f.set_modified(now - Duration::from_secs(30)).unwrap();
        assert_eq!(cache.scan(&t, Status::Running, now).background, 1, "fresh output");
        // the completion is appended: read incrementally on the next scan
        append(&t, &(notification("bk1", 60) + "\n"));
        assert_eq!(cache.scan(&t, Status::Running, now).background, 0, "notified");
        assert_eq!(cache.len(), 1);
        cache.retain(&[]);
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn cache_reads_appends_once_waits_for_whole_lines_and_resets_on_truncation() {
        let dir = tempfile::tempdir().unwrap();
        let (t, _) = layout(dir.path());
        let mut cache = ToolCache::with_tasks_root(None);
        let line = tool_use("t1", "Bash", r#"{"command":"make","description":"Build"}"#, 10);
        // a half-written line is not parsed yet
        let (head, tail) = line.split_at(40);
        append(&t, head);
        assert_eq!(cache.scan(&t, Status::Running, at(20)).current, None);
        append(&t, &format!("{tail}\n"));
        assert_eq!(cache.scan(&t, Status::Running, at(20)).current.map(|c| c.hint), Some("Build".into()));
        // unchanged file: same answer
        assert!(cache.scan(&t, Status::Running, at(20)).current.is_some());
        append(&t, &(result("t1", 12, "{}") + "\n"));
        assert_eq!(cache.scan(&t, Status::Running, at(20)).current, None);
        // rewritten shorter (e.g. a new file at the same path): start over
        std::fs::write(&t, tool_use("t5", "Grep", r#"{"pattern":"x"}"#, 10) + "\n").unwrap();
        assert_eq!(cache.scan(&t, Status::Running, at(20)).current.map(|c| c.name), Some("Grep".into()));
        // a missing file is nothing
        assert_eq!(cache.scan(&dir.path().join("missing.jsonl"), Status::Running, at(20)), ToolActivity::default());
    }

    #[test]
    fn cache_starts_from_a_window_at_the_end_of_big_transcripts() {
        let dir = tempfile::tempdir().unwrap();
        let (t, _) = layout(dir.path());
        // an old pending tool far before the window, then filler, then a live one
        append(&t, &(tool_use("old", "Read", r#"{"file_path":"/home/user/old.rs"}"#, 1) + "\n"));
        let filler = prompt(&"x".repeat(4096), 2) + "\n";
        for _ in 0..(INITIAL_WINDOW as usize / filler.len() + 2) {
            append(&t, &filler);
        }
        append(&t, &(tool_use("t1", "Glob", r#"{"pattern":"*.md"}"#, 10) + "\n"));
        let a = ToolCache::with_tasks_root(None).scan(&t, Status::Running, at(20));
        assert_eq!((a.current.map(|c| c.name), a.more), (Some("Glob".into()), 0));
    }
}
