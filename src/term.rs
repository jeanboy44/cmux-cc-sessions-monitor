//! Terminal setup/teardown and the quiet panic hook.
//!
//! Panic policy: a panic on the main (UI) thread restores the terminal first
//! and then prints the message to stderr, so it is readable after the alternate
//! screen is gone. Panics on any other thread (refresh, PR poller, cmux reader
//! threads) are appended to `~/.cache/cmux-magigent/panic.log` and never
//! written to the screen; those threads catch their panics and keep going.

use std::io::{stdout, Stdout, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Result;
use crossterm::cursor::{Hide, Show};
use crossterm::execute;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

static ACTIVE: AtomicBool = AtomicBool::new(false);

/// Owns the terminal in raw mode + alternate screen; restores it on drop.
pub struct TerminalGuard {
    pub terminal: Terminal<CrosstermBackend<Stdout>>,
}

impl TerminalGuard {
    pub fn new() -> Result<Self> {
        enable_raw_mode()?;
        // Only now is there something to restore.
        ACTIVE.store(true, Ordering::SeqCst);
        let mut out = stdout();
        if let Err(e) = execute!(out, EnterAlternateScreen, Hide) {
            restore();
            return Err(e.into());
        }
        match Terminal::new(CrosstermBackend::new(out)) {
            Ok(terminal) => Ok(Self { terminal }),
            Err(e) => {
                restore();
                Err(e.into())
            }
        }
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        restore();
    }
}

/// Leaves the alternate screen, disables raw mode and shows the cursor.
/// Idempotent: only the first call after [`TerminalGuard::new`] does anything.
pub fn restore() {
    if ACTIVE.swap(false, Ordering::SeqCst) {
        let _ = disable_raw_mode();
        let mut out = stdout();
        let _ = execute!(out, LeaveAlternateScreen, Show);
        let _ = out.flush();
    }
}

pub fn panic_log_path() -> PathBuf {
    dirs::home_dir().unwrap_or_default().join(".cache").join("cmux-magigent").join("panic.log")
}

fn log_panic(msg: &str) {
    let path = panic_log_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(file, "{msg}");
    }
}

/// Installs the panic hook described in the module docs.
pub fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let thread = std::thread::current();
        let name = thread.name().unwrap_or("<unnamed>");
        if name == "main" {
            restore();
            eprintln!("cmux-magigent panicked: {info}");
        } else {
            log_panic(&format!("[thread {name}] {info}"));
        }
    }));
}
