use anyhow::{anyhow, Context, Result};
use std::io::Read;
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

/// Runs `program` with `args` (argv, no shell) under ONE overall deadline that
/// covers spawn, output collection and exit. stdin is null and stderr is
/// discarded (`Output::stderr` is empty). On expiry the child is killed and
/// reaped and an `Err` is returned. The error text never contains arguments.
///
/// Only the direct child is killed: a grandchild that inherited the stdout pipe
/// may outlive the call, but the call itself still returns within the deadline.
pub fn run_bounded(
    program: &str,
    args: &[&str],
    cwd: Option<&Path>,
    timeout: Duration,
) -> Result<Output> {
    let deadline = Instant::now() + timeout;
    let mut cmd = Command::new(program);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    let mut child = cmd.spawn().with_context(|| format!("failed to start {program}"))?;
    let Some(mut stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(anyhow!("no stdout pipe for {program}"));
    };
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stdout.read_to_end(&mut bytes);
        let _ = tx.send(bytes);
    });
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break st,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(5)),
            other => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(match other {
                    Err(e) => anyhow!("waiting for {program} failed: {e}"),
                    _ => anyhow!("{program} timed out after {timeout:?}"),
                });
            }
        }
    };
    let left = deadline.saturating_duration_since(Instant::now());
    let stdout = rx
        .recv_timeout(left)
        .map_err(|_| anyhow!("{program} output not closed within {timeout:?}"))?;
    Ok(Output { status, stdout, stderr: Vec::new() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captures_stdout() {
        let out = run_bounded("sh", &["-c", "printf hello"], None, Duration::from_secs(2)).unwrap();
        assert!(out.status.success());
        assert_eq!(out.stdout, b"hello");
    }

    #[test]
    fn reports_nonzero_exit() {
        let out = run_bounded("sh", &["-c", "exit 3"], None, Duration::from_secs(2)).unwrap();
        assert_eq!(out.status.code(), Some(3));
    }

    #[test]
    fn honors_cwd() {
        let tmp = tempfile::tempdir().unwrap();
        let out = run_bounded("pwd", &[], Some(tmp.path()), Duration::from_secs(2)).unwrap();
        let got = String::from_utf8_lossy(&out.stdout).trim().to_string();
        assert_eq!(Path::new(&got).canonicalize().unwrap(), tmp.path().canonicalize().unwrap());
    }

    #[test]
    fn run_bounded_times_out_quickly() {
        let t = Instant::now();
        let r = run_bounded("sh", &["-c", "sleep 5"], None, Duration::from_millis(300));
        assert!(r.is_err());
        assert!(t.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn child_closing_stdout_but_alive_is_bounded() {
        let t = Instant::now();
        let r = run_bounded("sh", &["-c", "exec >&-; sleep 5"], None, Duration::from_millis(300));
        assert!(r.is_err());
        assert!(t.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn grandchild_holding_pipe_is_bounded() {
        let t = Instant::now();
        let r = run_bounded("sh", &["-c", "sleep 5 & exit 0"], None, Duration::from_millis(300));
        assert!(r.is_err());
        assert!(t.elapsed() < Duration::from_millis(1500), "{:?}", t.elapsed());
    }

    #[test]
    fn missing_binary_is_err() {
        assert!(run_bounded("definitely-not-a-binary-xyz", &[], None, Duration::from_secs(1)).is_err());
    }
}
