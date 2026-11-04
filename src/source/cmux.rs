use anyhow::{anyhow, bail, Context, Result};
use std::collections::HashMap;
use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(2);
/// Lines read by `read-screen` for status detection (the permission prompt
/// sits at the bottom).
pub const SCREEN_LINES: u16 = 40;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CmuxSessionRef {
    pub session_id: String,
    pub workspace_id: String,
    pub surface_id: String,
    pub cwd: PathBuf,
    pub pid_exists: bool,
    /// Raw `updated=` value (ISO-8601), used to pick among duplicate refs.
    pub updated: Option<String>,
    /// Set for a remote tab's session (see [`crate::source::remote`]): the
    /// card name, as no cwd or session name is known.
    pub title: Option<String>,
}

fn is_kv(tok: &str) -> bool {
    match tok.split_once('=') {
        Some((k, _)) => !k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'),
        None => false,
    }
}

/// Parses `cmux sessions list` output. Unknown or malformed lines are skipped.
pub fn parse_sessions_list(output: &str) -> Vec<CmuxSessionRef> {
    output.lines().filter_map(parse_line).collect()
}

fn parse_line(line: &str) -> Option<CmuxSessionRef> {
    let mut toks = line.split_whitespace();
    if toks.next()? != "claude" {
        return None;
    }
    let session_id = toks.next()?.to_string();
    if is_kv(&session_id) {
        return None;
    }
    let mut pairs: Vec<(String, String)> = Vec::new();
    for tok in toks {
        if is_kv(tok) {
            let (k, v) = tok.split_once('=').unwrap();
            pairs.push((k.to_string(), v.to_string()));
        } else if let Some((k, v)) = pairs.last_mut() {
            // A value containing spaces (e.g. cwd) was split on whitespace.
            if k == "cwd" {
                v.push(' ');
                v.push_str(tok);
            }
        }
    }
    let get = |key: &str| pairs.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone());
    let workspace_id = get("workspace").filter(|v| !v.is_empty())?;
    let surface_id = get("surface").filter(|v| !v.is_empty())?;
    Some(CmuxSessionRef {
        session_id,
        workspace_id,
        surface_id,
        cwd: PathBuf::from(get("cwd").unwrap_or_default()),
        pid_exists: get("pid_exists").as_deref() == Some("yes"),
        updated: get("updated").filter(|v| !v.is_empty()),
        title: None,
    })
}

/// What `cmux tree --all --id-format uuids` says about the open tabs: every
/// surface (uppercase id) with the window it sits in, and the windows in
/// printed order. The output is a `window <UUID> …` line per window followed
/// by its workspaces / panes / surfaces, e.g.
/// `│   └── surface <UUID> [terminal] "title" [selected] tty=ttys004`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TreeLayout {
    /// Surface id → window id (empty when no `window` line came before it).
    pub surfaces: HashMap<String, String>,
    pub window_order: Vec<String>,
    /// Surface id → workspace and title of its tab, for terminal tabs.
    pub tabs: HashMap<String, TreeTab>,
}

/// Where a terminal tab sits and what it is called, from `cmux tree`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TreeTab {
    /// Uppercase id; empty when no `workspace` line came before the tab.
    pub workspace_id: String,
    pub title: String,
}

impl TreeLayout {
    /// Whether the surface (any case) still exists.
    pub fn has_surface(&self, surface_id: &str) -> bool {
        self.surfaces.contains_key(&surface_id.to_ascii_uppercase())
    }

    /// The window of a surface (any case), if known.
    pub fn window_of(&self, surface_id: &str) -> Option<&str> {
        self.surfaces.get(&surface_id.to_ascii_uppercase()).map(String::as_str).filter(|w| !w.is_empty())
    }
}

/// Parses `cmux tree --all --id-format uuids` output. Lines that are neither
/// window nor surface lines, or whose id looks wrong, are ignored.
pub fn parse_tree(output: &str) -> TreeLayout {
    let mut out = TreeLayout::default();
    let mut window = String::new();
    let mut workspace = String::new();
    for line in output.lines() {
        match tree_item(line) {
            Some(("window", id)) => {
                if !out.window_order.contains(&id) {
                    out.window_order.push(id.clone());
                }
                window = id;
            }
            Some(("workspace", id)) => workspace = id,
            Some(("surface", id)) => {
                if let Some(title) = terminal_title(line) {
                    out.tabs.insert(id.clone(), TreeTab { workspace_id: workspace.clone(), title });
                }
                out.surfaces.insert(id, window.clone());
            }
            _ => {}
        }
    }
    out
}

/// Surface ids (uppercased) of a tree output; see [`parse_tree`].
#[cfg(test)]
pub fn parse_tree_surfaces(output: &str) -> std::collections::HashSet<String> {
    parse_tree(output).surfaces.into_keys().collect()
}

/// (`window` | `surface`, uppercase id) of a tree line.
fn tree_item(line: &str) -> Option<(&'static str, String)> {
    // Drop the tree drawing (`│ ├── └──`) and indentation in front.
    let rest = line.trim_start_matches(|c: char| c.is_whitespace() || matches!(c, '│' | '├' | '└' | '─' | '|' | '`'));
    let mut toks = rest.split_whitespace();
    let kind = match toks.next()? {
        "window" => "window",
        "workspace" => "workspace",
        "surface" => "surface",
        _ => return None,
    };
    let id = toks.next()?;
    let ok = id.len() >= 8 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    ok.then(|| (kind, id.to_ascii_uppercase()))
}

/// The title of a `surface <id> [terminal] "title" …` line. It may hold
/// quotes itself, and nothing after it does.
fn terminal_title(line: &str) -> Option<String> {
    let (_, rest) = line.split_once("[terminal] \"")?;
    Some(rest.rsplit_once('"').map_or("", |(title, _)| title).to_string())
}

/// One entry of `cmux list-notifications`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notification {
    pub id: String,
    pub workspace_id: String,
    pub surface_id: String,
    /// The read column says `unread`.
    pub unread: bool,
    pub title: String,
    /// Raw ISO-8601 time (`2026-01-01T00:00:00Z`).
    pub timestamp: String,
    pub workspace_title: String,
}

/// Whether a field looks like the notification time: `YYYY-MM-DDTHH:MM:SS…Z`.
fn is_timestamp(field: &str) -> bool {
    let b = field.as_bytes();
    b.len() >= 20
        && b[..4].iter().all(u8::is_ascii_digit)
        && b[4] == b'-'
        && b[7] == b'-'
        && b[10] == b'T'
        && b[13] == b':'
        && b.ends_with(b"Z")
        && field.bytes().all(|c| c.is_ascii_digit() || matches!(c, b'-' | b':' | b'.' | b'T' | b'Z'))
}

/// Whether a line starts a notification record: `index:id|ws|surface|read|…`.
/// Anything else continues the previous record's body (bodies may hold newlines).
fn starts_record(line: &str) -> bool {
    let mut f = line.split('|');
    let head = f.next().unwrap_or_default();
    let index_ok = match head.split_once(':') {
        Some((i, id)) => !i.is_empty() && i.bytes().all(|b| b.is_ascii_digit()) && !id.is_empty(),
        None => false,
    };
    index_ok && matches!(f.nth(2), Some("read" | "unread"))
}

/// Parses `cmux list-notifications` output, one record per
/// `index:id|workspace|surface|read|app|title|body|timestamp|workspace title`.
/// The body may contain `|` and newlines, so the leading five fields are taken
/// from the left and the timestamp is the rightmost field that looks like one;
/// the workspace title is everything after it. The title is the field after
/// the app. Records with a bad read column, empty ids or no timestamp are
/// skipped, as is any text before the first record.
pub fn parse_notifications(output: &str) -> Vec<Notification> {
    let mut records: Vec<String> = Vec::new();
    for line in output.lines() {
        if starts_record(line) {
            records.push(line.to_string());
        } else if let Some(last) = records.last_mut() {
            last.push('\n');
            last.push_str(line);
        }
    }
    records.iter().filter_map(|r| parse_notification(r)).collect()
}

fn parse_notification(record: &str) -> Option<Notification> {
    let mut head = record.splitn(6, '|');
    let (_, id) = head.next()?.split_once(':')?;
    let workspace_id = head.next()?;
    let surface_id = head.next()?;
    let unread = match head.next()? {
        "unread" => true,
        "read" => false,
        _ => return None,
    };
    let _app = head.next()?;
    let rest: Vec<&str> = head.next()?.split('|').collect();
    // rest = title | body… | timestamp | workspace title…
    let ts = (1..rest.len()).rev().find(|&i| is_timestamp(rest[i]))?;
    if id.is_empty() || workspace_id.is_empty() || surface_id.is_empty() {
        return None;
    }
    Some(Notification {
        id: id.to_string(),
        workspace_id: workspace_id.to_string(),
        surface_id: surface_id.to_string(),
        unread,
        title: rest[0].to_string(),
        timestamp: rest[ts].to_string(),
        workspace_title: rest[ts + 1..].join("|"),
    })
}

pub trait Cmux {
    fn sessions(&self) -> Result<Vec<CmuxSessionRef>>;
    /// Every surface (tab) that currently exists in cmux, with its window.
    /// `cmux sessions list` is a saved-record list and keeps closed tabs, so
    /// this is the only source for "is this tab still open".
    fn tree(&self) -> Result<TreeLayout>;
    fn focus(&self, workspace_id: &str, surface_id: &str) -> Result<()>;
    /// Last `lines` non-blank lines of a surface's screen. `workspace_id` is
    /// required: cmux otherwise defaults to the caller's own workspace.
    fn read_screen(&self, workspace_id: &str, surface_id: &str, lines: u16) -> Result<String>;
    /// Every queued notification (read and unread), read-only.
    fn notifications(&self) -> Result<Vec<Notification>>;
}

/// Real implementation shelling out to the `cmux` binary, 2s timeout per call.
pub struct CmuxCli {
    bin: PathBuf,
    timeout: Duration,
}

impl Default for CmuxCli {
    fn default() -> Self {
        Self::new()
    }
}

impl CmuxCli {
    /// Uses `$CMUX_BIN` if set, otherwise `cmux` from PATH.
    pub fn new() -> Self {
        let bin = std::env::var_os("CMUX_BIN")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("cmux"));
        Self::with_bin(bin, TIMEOUT)
    }

    pub fn with_bin(bin: PathBuf, timeout: Duration) -> Self {
        Self { bin, timeout }
    }

    fn run(&self, args: &[&str]) -> Result<String> {
        let mut child = Command::new(&self.bin)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("failed to spawn {}", self.bin.display()))?;
        let deadline = Instant::now() + self.timeout;
        let mut stdout = child.stdout.take().ok_or_else(|| anyhow!("no stdout"))?;
        let mut stderr = child.stderr.take().ok_or_else(|| anyhow!("no stderr"))?;
        let (out_tx, out_rx) = mpsc::channel();
        let (err_tx, err_rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = stdout.read_to_end(&mut bytes);
            let _ = out_tx.send(bytes);
        });
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = stderr.read_to_end(&mut bytes);
            let _ = err_tx.send(bytes);
        });

        // Everything below is bounded by the single `deadline`.
        let outcome = (|| -> Result<(std::process::ExitStatus, Vec<u8>, Vec<u8>)> {
            let status = loop {
                if let Some(st) = child.try_wait()? {
                    break st;
                }
                if Instant::now() >= deadline {
                    bail!("timed out waiting for exit");
                }
                std::thread::sleep(Duration::from_millis(5));
            };
            let left = |d: Instant| d.saturating_duration_since(Instant::now());
            let out = out_rx
                .recv_timeout(left(deadline))
                .map_err(|_| anyhow!("timed out reading stdout"))?;
            let err = err_rx
                .recv_timeout(left(deadline))
                .map_err(|_| anyhow!("timed out reading stderr"))?;
            Ok((status, out, err))
        })();

        match outcome {
            Ok((status, out, err)) => {
                if !status.success() {
                    bail!(
                        "cmux {:?} failed ({}): {}",
                        args,
                        status,
                        String::from_utf8_lossy(&err).trim()
                    );
                }
                Ok(String::from_utf8_lossy(&out).into_owned())
            }
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                bail!("cmux {:?} aborted after {:?}: {}", args, self.timeout, e)
            }
        }
    }
}

impl Cmux for CmuxCli {
    fn sessions(&self) -> Result<Vec<CmuxSessionRef>> {
        Ok(parse_sessions_list(&self.run(&["sessions", "list"])?))
    }

    /// An output without any surface is treated as an error: the monitor's
    /// own tab always exists, so an empty tree means cmux answered oddly and
    /// must not make every tile disappear.
    fn tree(&self) -> Result<TreeLayout> {
        let layout = parse_tree(&self.run(&["tree", "--all", "--id-format", "uuids"])?);
        if layout.surfaces.is_empty() {
            bail!("cmux tree listed no surfaces");
        }
        Ok(layout)
    }

    fn focus(&self, workspace_id: &str, surface_id: &str) -> Result<()> {
        self.run(&["select-workspace", "--workspace", workspace_id])?;
        self.run(&["focus-panel", "--panel", surface_id, "--workspace", workspace_id])?;
        Ok(())
    }

    fn read_screen(&self, workspace_id: &str, surface_id: &str, lines: u16) -> Result<String> {
        let lines = lines.max(1).to_string();
        self.run(&["read-screen", "--workspace", workspace_id, "--surface", surface_id, "--lines", &lines])
    }

    fn notifications(&self) -> Result<Vec<Notification>> {
        Ok(parse_notifications(&self.run(&["list-notifications"])?))
    }
}

#[cfg(test)]
pub struct FakeCmux {
    pub sessions_result: Result<Vec<CmuxSessionRef>, String>,
    /// Defaults to the surfaces of the sessions passed to [`FakeCmux::new`]
    /// (no window info).
    pub tree_result: Result<TreeLayout, String>,
    pub read_screen_result: Result<String, String>,
    /// Per-surface screens; surfaces not listed get `read_screen_result`.
    pub screens: std::collections::HashMap<String, Result<String, String>>,
    pub focus_result: Result<(), String>,
    /// Defaults to no notifications.
    pub notifications_result: Result<Vec<Notification>, String>,
    /// Recorded calls, e.g. `sessions`, `focus ws surface`, `read_screen ws surface`.
    pub calls: std::sync::Mutex<Vec<String>>,
    /// `lines` argument of every `read_screen` call, in order.
    pub read_lines: std::sync::Mutex<Vec<u16>>,
}

#[cfg(test)]
impl FakeCmux {
    pub fn new(sessions: Vec<CmuxSessionRef>) -> Self {
        let surfaces = TreeLayout {
            surfaces: sessions.iter().map(|r| (r.surface_id.to_ascii_uppercase(), String::new())).collect(),
            ..Default::default()
        };
        Self {
            sessions_result: Ok(sessions),
            tree_result: Ok(surfaces),
            read_screen_result: Ok(String::new()),
            screens: Default::default(),
            focus_result: Ok(()),
            notifications_result: Ok(Vec::new()),
            calls: Default::default(),
            read_lines: Default::default(),
        }
    }

    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

#[cfg(test)]
impl Cmux for FakeCmux {
    fn sessions(&self) -> Result<Vec<CmuxSessionRef>> {
        self.calls.lock().unwrap().push("sessions".into());
        self.sessions_result.clone().map_err(|e| anyhow!(e))
    }

    fn tree(&self) -> Result<TreeLayout> {
        self.calls.lock().unwrap().push("tree".into());
        self.tree_result.clone().map_err(|e| anyhow!(e))
    }

    fn focus(&self, workspace_id: &str, surface_id: &str) -> Result<()> {
        self.calls.lock().unwrap().push(format!("focus {workspace_id} {surface_id}"));
        self.focus_result.clone().map_err(|e| anyhow!(e))
    }

    fn read_screen(&self, workspace_id: &str, surface_id: &str, lines: u16) -> Result<String> {
        self.calls.lock().unwrap().push(format!("read_screen {workspace_id} {surface_id}"));
        self.read_lines.lock().unwrap().push(lines);
        let result = self.screens.get(surface_id).unwrap_or(&self.read_screen_result);
        result.clone().map_err(|e| anyhow!(e))
    }

    fn notifications(&self) -> Result<Vec<Notification>> {
        self.calls.lock().unwrap().push("notifications".into());
        self.notifications_result.clone().map_err(|e| anyhow!(e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    const FIXTURE: &str = include_str!("../../tests/fixtures/cmux_sessions_list.txt");

    // Serializes tests that write+exec scripts: a concurrent fork in another test
    // thread can otherwise hold the script's write fd open (ETXTBSY).
    static EXEC_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn lock() -> std::sync::MutexGuard<'static, ()> {
        EXEC_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn script(dir: &tempfile::TempDir, body: &str) -> PathBuf {
        let p = dir.path().join("fake-cmux");
        std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    #[test]
    fn parse_tree_gives_workspace_and_title_of_terminal_tabs() {
        let t = parse_tree(concat!(
            "window aaaaaaaa-0000-0000-0000-000000000001\n",
            "├── workspace bbbbbbbb-0000-0000-0000-000000000001 \"[mosh] ws\" [selected]\n",
            "│   └── pane bbbbbbbb-0000-0000-0000-000000000001 [focused]\n",
            "│       ├── surface 00000000-0000-0000-0000-000000000001 [terminal] \"[mosh] say \"hi\" 계통수\" [selected] tty=ttys001\n",
            "│       └── surface 00000000-0000-0000-0000-000000000002 [browser] \"docs\"\n",
        ));
        let tab = &t.tabs["00000000-0000-0000-0000-000000000001"];
        assert_eq!(tab.workspace_id, "BBBBBBBB-0000-0000-0000-000000000001");
        assert_eq!(tab.title, "[mosh] say \"hi\" 계통수");
        assert!(!t.tabs.contains_key("00000000-0000-0000-0000-000000000002"));
        assert!(t.has_surface("00000000-0000-0000-0000-000000000002"));
    }

    #[test]
    fn parse_sessions_list_fixture() {
        let v = parse_sessions_list(FIXTURE);
        assert_eq!(v.len(), 3);
        assert_eq!(v[0].session_id, "00000000-0000-0000-0000-000000000001");
        assert_eq!(v[0].workspace_id, "AAAAAAAA-0000-0000-0000-000000000001");
        assert_eq!(v[0].surface_id, "BBBBBBBB-0000-0000-0000-000000000001");
        assert_eq!(v[0].cwd, PathBuf::from("/home/user/proj-a"));
        assert!(v[0].pid_exists);
        assert!(!v[1].pid_exists);
        assert!(v[2].pid_exists);
        assert_eq!(v[0].updated.as_deref(), Some("2026-01-01T00:00:00.000Z"));
        assert_eq!(v[2].updated.as_deref(), Some("2026-01-01T00:00:02.000Z"));
        assert_eq!(parse_sessions_list("claude s workspace=w surface=f pid_exists=yes")[0].updated, None);
    }

    #[test]
    fn parse_keeps_cwd_with_spaces() {
        let v = parse_sessions_list(FIXTURE);
        assert_eq!(v[1].cwd, PathBuf::from("/home/user/proj b/sub"));
    }

    #[test]
    fn parse_ignores_garbage_lines() {
        let out = "garbage\n\ncodex 1 workspace=a surface=b cwd=/x pid_exists=yes\nclaude\nclaude id-only\nclaude s1 surface=b cwd=/x\nclaude s2 workspace=w surface=s cwd=/ok pid_exists=yes\n";
        let v = parse_sessions_list(out);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].session_id, "s2");
    }

    #[test]
    fn parse_handles_empty_output() {
        assert!(parse_sessions_list("").is_empty());
        assert!(parse_sessions_list("\n\n").is_empty());
    }

    #[test]
    fn cli_times_out_on_hanging_command() {
        let _g = lock();
        let d = tempfile::tempdir().unwrap();
        let cli = CmuxCli::with_bin(script(&d, "exec sleep 5"), Duration::from_millis(300));
        let t = Instant::now();
        assert!(cli.sessions().is_err());
        assert!(t.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn cli_missing_binary_is_err() {
        let _g = lock();
        let cli = CmuxCli::with_bin(PathBuf::from("/nonexistent/cmux-bin"), TIMEOUT);
        assert!(cli.sessions().is_err());
        assert!(cli.read_screen("w", "x", SCREEN_LINES).is_err());
    }

    #[test]
    fn cli_nonzero_exit_is_err() {
        let _g = lock();
        let d = tempfile::tempdir().unwrap();
        let cli = CmuxCli::with_bin(script(&d, "echo boom >&2; exit 1"), TIMEOUT);
        let e = cli.sessions().unwrap_err().to_string();
        assert!(e.contains("boom"), "{e}");
    }

    #[test]
    fn cli_bounded_when_child_closes_stdout_but_keeps_running() {
        let _g = lock();
        let d = tempfile::tempdir().unwrap();
        let cli = CmuxCli::with_bin(script(&d, "exec >/dev/null\nsleep 5"), Duration::from_millis(300));
        let t = Instant::now();
        assert!(cli.sessions().is_err());
        assert!(t.elapsed() < Duration::from_millis(1500), "{:?}", t.elapsed());
    }

    #[test]
    fn cli_bounded_when_grandchild_holds_pipes() {
        let _g = lock();
        let d = tempfile::tempdir().unwrap();
        // Child exits at once, but a background sleep keeps stderr open.
        let cli = CmuxCli::with_bin(script(&d, "sleep 5 >/dev/null &\nexit 0"), Duration::from_millis(300));
        let t = Instant::now();
        assert!(cli.sessions().is_err());
        assert!(t.elapsed() < Duration::from_millis(1500), "{:?}", t.elapsed());
        // Same, holding stdout too.
        let cli = CmuxCli::with_bin(script(&d, "sleep 5 &\nexit 0"), Duration::from_millis(300));
        let t = Instant::now();
        assert!(cli.sessions().is_err());
        assert!(t.elapsed() < Duration::from_millis(1500), "{:?}", t.elapsed());
    }

    #[test]
    fn cli_sessions_parses_stdout() {
        let _g = lock();
        let d = tempfile::tempdir().unwrap();
        let fx = d.path().join("out.txt");
        std::fs::write(&fx, FIXTURE).unwrap();
        let cli = CmuxCli::with_bin(script(&d, &format!("cat {}", fx.display())), TIMEOUT);
        assert_eq!(cli.sessions().unwrap().len(), 3);
    }

    #[test]
    fn focus_runs_select_then_focus_in_order_and_stops_on_failure() {
        let _g = lock();
        let d = tempfile::tempdir().unwrap();
        let log = d.path().join("log.txt");
        let ok = script(&d, &format!("echo \"$@\" >> {}", log.display()));
        CmuxCli::with_bin(ok, TIMEOUT).focus("WS", "SURF").unwrap();
        assert_eq!(
            std::fs::read_to_string(&log).unwrap(),
            "select-workspace --workspace WS\nfocus-panel --panel SURF --workspace WS\n"
        );

        std::fs::remove_file(&log).unwrap();
        let fail = script(&d, &format!("echo \"$@\" >> {}; exit 1", log.display()));
        assert!(CmuxCli::with_bin(fail, TIMEOUT).focus("WS", "SURF").is_err());
        assert_eq!(std::fs::read_to_string(&log).unwrap(), "select-workspace --workspace WS\n");
    }

    #[test]
    fn read_screen_targets_workspace_and_surface_with_line_limit() {
        let _g = lock();
        let d = tempfile::tempdir().unwrap();
        let log = d.path().join("log.txt");
        let ok = script(&d, &format!("echo \"$@\" >> {}; echo screen-text", log.display()));
        let cli = CmuxCli::with_bin(ok, TIMEOUT);
        assert_eq!(cli.read_screen("WS", "SURF", SCREEN_LINES).unwrap(), "screen-text\n");
        cli.read_screen("WS", "SURF", 77).unwrap();
        cli.read_screen("WS", "SURF", 0).unwrap();
        assert_eq!(
            std::fs::read_to_string(&log).unwrap(),
            "read-screen --workspace WS --surface SURF --lines 40\n\
             read-screen --workspace WS --surface SURF --lines 77\n\
             read-screen --workspace WS --surface SURF --lines 1\n"
        );
    }

    #[test]
    fn fake_cmux_records_calls() {
        let f = FakeCmux::new(vec![]);
        f.sessions().unwrap();
        f.focus("w", "s").unwrap();
        f.read_screen("w", "s", 12).unwrap();
        f.tree().unwrap();
        assert!(f.notifications().unwrap().is_empty());
        assert_eq!(*f.read_lines.lock().unwrap(), vec![12]);
        assert_eq!(f.calls(), vec!["sessions", "focus w s", "read_screen w s", "tree", "notifications"]);
    }

    const TREE: &str = "\
window AAAAAAAA-0000-0000-0000-0000000000F0 [current] ◀ active
├── workspace AAAAAAAA-0000-0000-0000-000000000001 \"proj a\"
│   ├── pane AAAAAAAA-0000-0000-0000-0000000000A1 [focused]
│   │   └── surface BBBBBBBB-0000-0000-0000-000000000001 [terminal] \"✳ surface BBBBBBBB-0000-0000-0000-00000000DEAD\" [selected] tty=ttys004
│   └── pane AAAAAAAA-0000-0000-0000-0000000000A2
│       ├── surface bbbbbbbb-0000-0000-0000-000000000002 [terminal] \"lower\" tty=ttys005
│       └── surface BBBBBBBB-0000-0000-0000-000000000003 [browser] \"docs\" [selected]
└── workspace AAAAAAAA-0000-0000-0000-000000000002 \"empty\"
garbage line
surface
    surface ??? [terminal]
│   └── surface
surfaces BBBBBBBB-0000-0000-0000-00000000BEEF
";

    #[test]
    fn tree_parse_extracts_surface_ids() {
        let got = parse_tree_surfaces(TREE);
        let want: std::collections::HashSet<String> = [
            "BBBBBBBB-0000-0000-0000-000000000001",
            "BBBBBBBB-0000-0000-0000-000000000002",
            "BBBBBBBB-0000-0000-0000-000000000003",
        ]
        .into_iter()
        .map(String::from)
        .collect();
        assert_eq!(got, want);
        assert!(parse_tree_surfaces("").is_empty());
        assert!(parse_tree_surfaces("\n│\n└──\n").is_empty());
    }

    #[test]
    fn cli_surfaces_runs_tree_with_uuids_and_rejects_empty_tree() {
        let _g = lock();
        let d = tempfile::tempdir().unwrap();
        let log = d.path().join("log.txt");
        let fx = d.path().join("tree.txt");
        std::fs::write(&fx, TREE).unwrap();
        let ok = script(&d, &format!("echo \"$@\" >> {}; cat {}", log.display(), fx.display()));
        assert_eq!(CmuxCli::with_bin(ok, TIMEOUT).tree().unwrap().surfaces.len(), 3);
        assert_eq!(std::fs::read_to_string(&log).unwrap(), "tree --all --id-format uuids\n");
        let empty = script(&d, "echo 'window AAAAAAAA-0000-0000-0000-0000000000F0'");
        assert!(CmuxCli::with_bin(empty, TIMEOUT).tree().is_err(), "no surface at all = unknown");
        let fail = script(&d, "exit 1");
        assert!(CmuxCli::with_bin(fail, TIMEOUT).tree().is_err());
    }

    const TWO_WINDOWS: &str = "\
window AAAAAAAA-0000-0000-0000-0000000000f1 [current] ◀ active
├── workspace AAAAAAAA-0000-0000-0000-000000000001 \"a\"
│   └── pane AAAAAAAA-0000-0000-0000-0000000000A1 [focused]
│       ├── surface BBBBBBBB-0000-0000-0000-000000000001 [terminal] \"x\" [selected] tty=ttys001
│       └── surface BBBBBBBB-0000-0000-0000-000000000002 [browser] \"y\"
window AAAAAAAA-0000-0000-0000-0000000000F2
└── workspace AAAAAAAA-0000-0000-0000-000000000002 \"b\"
    └── pane AAAAAAAA-0000-0000-0000-0000000000A2
        └── surface bbbbbbbb-0000-0000-0000-000000000003 [terminal] \"window z\" tty=ttys002
window ???
";

    const NOTES: &str = "\
0:N0000000-0000-0000-0000-000000000001|AAAAAAAA-0000-0000-0000-000000000001|BBBBBBBB-0000-0000-0000-000000000001|unread|Claude Code|Done|Finished the task|2026-01-01T00:00:05Z|proj a
1:N0000000-0000-0000-0000-000000000002|AAAAAAAA-0000-0000-0000-000000000001|BBBBBBBB-0000-0000-0000-000000000002|read|Claude Code|Waiting|a | b | c|2026-01-01T00:00:04Z|ws | with pipe
2:N0000000-0000-0000-0000-000000000003|AAAAAAAA-0000-0000-0000-000000000002|bbbbbbbb-0000-0000-0000-000000000003|unread|Claude Code|Multi|line one
line two | still body
3: not a record
line three|2026-01-01T00:00:03.250Z|proj c
";

    #[test]
    fn parse_notifications_reads_state_title_and_ids() {
        let v = parse_notifications(NOTES);
        assert_eq!(v.len(), 3, "{v:?}");
        assert_eq!(v[0].id, "N0000000-0000-0000-0000-000000000001");
        assert_eq!(v[0].workspace_id, "AAAAAAAA-0000-0000-0000-000000000001");
        assert_eq!(v[0].surface_id, "BBBBBBBB-0000-0000-0000-000000000001");
        assert!(v[0].unread);
        assert_eq!(v[0].title, "Done");
        assert_eq!(v[0].timestamp, "2026-01-01T00:00:05Z");
        assert_eq!(v[0].workspace_title, "proj a");
        // a body with `|` and a workspace title with `|`: both ends stay right
        assert!(!v[1].unread);
        assert_eq!(v[1].title, "Waiting");
        assert_eq!(v[1].timestamp, "2026-01-01T00:00:04Z");
        assert_eq!(v[1].workspace_title, "ws | with pipe");
        // a body spanning lines (even one that looks like `3: …`) is one record
        assert!(v[2].unread);
        assert_eq!(v[2].surface_id, "bbbbbbbb-0000-0000-0000-000000000003");
        assert_eq!(v[2].title, "Multi");
        assert_eq!(v[2].timestamp, "2026-01-01T00:00:03.250Z");
        assert_eq!(v[2].workspace_title, "proj c");
    }

    #[test]
    fn parse_notifications_empty_and_malformed() {
        assert!(parse_notifications("").is_empty());
        assert!(parse_notifications("\n\n").is_empty());
        assert!(parse_notifications("No notifications\n").is_empty());
        let bad = "\
garbage
0:N1|ws|sf|maybe|Claude Code|t|b|2026-01-01T00:00:00Z|w
1:N2||sf|unread|Claude Code|t|b|2026-01-01T00:00:00Z|w
2:N3|ws||unread|Claude Code|t|b|2026-01-01T00:00:00Z|w
3:N4|ws|sf|unread|Claude Code|t|b|no time|w
4:N5|ws|sf|unread
x5:N6|ws|sf|unread|Claude Code|t|b|2026-01-01T00:00:00Z|w
6:N7|ws|sf|unread|Claude Code|ok||2026-01-01T00:00:00Z|
";
        let v = parse_notifications(bad);
        assert_eq!(v.len(), 1, "{v:?}");
        assert_eq!(v[0].id, "N7");
        assert_eq!(v[0].title, "ok");
        assert_eq!(v[0].workspace_title, "");
    }

    #[test]
    fn cli_notifications_runs_list_notifications_and_fails_cleanly() {
        let _g = lock();
        let d = tempfile::tempdir().unwrap();
        let log = d.path().join("log.txt");
        let fx = d.path().join("notes.txt");
        std::fs::write(&fx, NOTES).unwrap();
        let ok = script(&d, &format!("echo \"$@\" >> {}; cat {}", log.display(), fx.display()));
        assert_eq!(CmuxCli::with_bin(ok, TIMEOUT).notifications().unwrap().len(), 3);
        assert_eq!(std::fs::read_to_string(&log).unwrap(), "list-notifications\n");
        let fail = script(&d, "exit 1");
        assert!(CmuxCli::with_bin(fail, TIMEOUT).notifications().is_err());
        let hang = script(&d, "exec sleep 5");
        let t = Instant::now();
        assert!(CmuxCli::with_bin(hang, Duration::from_millis(300)).notifications().is_err());
        assert!(t.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn tree_parse_maps_surfaces_to_windows_in_order() {
        let t = parse_tree(TWO_WINDOWS);
        let w1 = "AAAAAAAA-0000-0000-0000-0000000000F1";
        let w2 = "AAAAAAAA-0000-0000-0000-0000000000F2";
        assert_eq!(t.window_order, vec![w1.to_string(), w2.to_string()]);
        assert_eq!(t.surfaces.len(), 3);
        assert_eq!(t.window_of("bbbbbbbb-0000-0000-0000-000000000001"), Some(w1));
        assert_eq!(t.window_of("BBBBBBBB-0000-0000-0000-000000000002"), Some(w1));
        assert_eq!(t.window_of("BBBBBBBB-0000-0000-0000-000000000003"), Some(w2));
        assert!(t.has_surface("bbbbbbbb-0000-0000-0000-000000000003"));
        assert!(!t.has_surface("BBBBBBBB-0000-0000-0000-00000000DEAD"));
        // a surface before any window line: open, window unknown
        let t = parse_tree("surface BBBBBBBB-0000-0000-0000-000000000009 [terminal]");
        assert!(t.has_surface("BBBBBBBB-0000-0000-0000-000000000009"));
        assert_eq!(t.window_of("BBBBBBBB-0000-0000-0000-000000000009"), None);
        assert!(t.window_order.is_empty());
    }
}
