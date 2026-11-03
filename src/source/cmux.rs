use anyhow::{anyhow, bail, Context, Result};
use std::collections::HashSet;
use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(2);
/// Lines read by `read-screen` (the permission prompt sits at the bottom).
const SCREEN_LINES: &str = "40";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CmuxSessionRef {
    pub session_id: String,
    pub workspace_id: String,
    pub surface_id: String,
    pub cwd: PathBuf,
    pub pid_exists: bool,
    /// Raw `updated=` value (ISO-8601), used to pick among duplicate refs.
    pub updated: Option<String>,
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
    })
}

/// Surface ids (uppercased) found in `cmux tree --all --id-format uuids`
/// output, whose surface lines look like
/// `│   └── surface <UUID> [terminal] "title" [selected] tty=ttys004`.
/// Lines that are not surface lines, or whose id looks wrong, are ignored.
pub fn parse_tree_surfaces(output: &str) -> HashSet<String> {
    output.lines().filter_map(tree_surface_id).collect()
}

fn tree_surface_id(line: &str) -> Option<String> {
    // Drop the tree drawing (`│ ├── └──`) and indentation in front.
    let rest = line.trim_start_matches(|c: char| c.is_whitespace() || matches!(c, '│' | '├' | '└' | '─' | '|' | '`'));
    let mut toks = rest.split_whitespace();
    if toks.next()? != "surface" {
        return None;
    }
    let id = toks.next()?;
    let ok = id.len() >= 8 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    ok.then(|| id.to_ascii_uppercase())
}

pub trait Cmux {
    fn sessions(&self) -> Result<Vec<CmuxSessionRef>>;
    /// Ids (uppercase) of every surface (tab) that currently exists in cmux.
    /// `cmux sessions list` is a saved-record list and keeps closed tabs, so
    /// this is the only source for "is this tab still open".
    fn surfaces(&self) -> Result<HashSet<String>>;
    fn focus(&self, workspace_id: &str, surface_id: &str) -> Result<()>;
    /// Last lines of a surface's screen. `workspace_id` is required: cmux
    /// otherwise defaults to the caller's own workspace.
    fn read_screen(&self, workspace_id: &str, surface_id: &str) -> Result<String>;
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
    fn surfaces(&self) -> Result<HashSet<String>> {
        let set = parse_tree_surfaces(&self.run(&["tree", "--all", "--id-format", "uuids"])?);
        if set.is_empty() {
            bail!("cmux tree listed no surfaces");
        }
        Ok(set)
    }

    fn focus(&self, workspace_id: &str, surface_id: &str) -> Result<()> {
        self.run(&["select-workspace", "--workspace", workspace_id])?;
        self.run(&["focus-panel", "--panel", surface_id, "--workspace", workspace_id])?;
        Ok(())
    }

    fn read_screen(&self, workspace_id: &str, surface_id: &str) -> Result<String> {
        self.run(&["read-screen", "--workspace", workspace_id, "--surface", surface_id, "--lines", SCREEN_LINES])
    }
}

#[cfg(test)]
pub struct FakeCmux {
    pub sessions_result: Result<Vec<CmuxSessionRef>, String>,
    /// Defaults to the surfaces of the sessions passed to [`FakeCmux::new`].
    pub surfaces_result: Result<HashSet<String>, String>,
    pub read_screen_result: Result<String, String>,
    pub focus_result: Result<(), String>,
    /// Recorded calls, e.g. `sessions`, `focus ws surface`, `read_screen ws surface`.
    pub calls: std::sync::Mutex<Vec<String>>,
}

#[cfg(test)]
impl FakeCmux {
    pub fn new(sessions: Vec<CmuxSessionRef>) -> Self {
        let surfaces = sessions.iter().map(|r| r.surface_id.to_ascii_uppercase()).collect();
        Self {
            sessions_result: Ok(sessions),
            surfaces_result: Ok(surfaces),
            read_screen_result: Ok(String::new()),
            focus_result: Ok(()),
            calls: Default::default(),
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

    fn surfaces(&self) -> Result<HashSet<String>> {
        self.calls.lock().unwrap().push("surfaces".into());
        self.surfaces_result.clone().map_err(|e| anyhow!(e))
    }

    fn focus(&self, workspace_id: &str, surface_id: &str) -> Result<()> {
        self.calls.lock().unwrap().push(format!("focus {workspace_id} {surface_id}"));
        self.focus_result.clone().map_err(|e| anyhow!(e))
    }

    fn read_screen(&self, workspace_id: &str, surface_id: &str) -> Result<String> {
        self.calls.lock().unwrap().push(format!("read_screen {workspace_id} {surface_id}"));
        self.read_screen_result.clone().map_err(|e| anyhow!(e))
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
        assert!(cli.read_screen("w", "x").is_err());
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
        let out = CmuxCli::with_bin(ok, TIMEOUT).read_screen("WS", "SURF").unwrap();
        assert_eq!(out, "screen-text\n");
        assert_eq!(
            std::fs::read_to_string(&log).unwrap(),
            "read-screen --workspace WS --surface SURF --lines 40\n"
        );
    }

    #[test]
    fn fake_cmux_records_calls() {
        let f = FakeCmux::new(vec![]);
        f.sessions().unwrap();
        f.focus("w", "s").unwrap();
        f.read_screen("w", "s").unwrap();
        f.surfaces().unwrap();
        assert_eq!(f.calls(), vec!["sessions", "focus w s", "read_screen w s", "surfaces"]);
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
        let want: HashSet<String> = [
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
        assert_eq!(CmuxCli::with_bin(ok, TIMEOUT).surfaces().unwrap().len(), 3);
        assert_eq!(std::fs::read_to_string(&log).unwrap(), "tree --all --id-format uuids\n");
        let empty = script(&d, "echo 'window AAAAAAAA-0000-0000-0000-0000000000F0'");
        assert!(CmuxCli::with_bin(empty, TIMEOUT).surfaces().is_err(), "no surface at all = unknown");
        let fail = script(&d, "exit 1");
        assert!(CmuxCli::with_bin(fail, TIMEOUT).surfaces().is_err());
    }
}
