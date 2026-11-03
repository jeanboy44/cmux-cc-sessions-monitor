//! Settings-window behaviour of [`App`]: keys, applying a change (shared
//! config, save to disk, PR poller) and the poller lifecycle.

use std::ffi::OsString;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use super::{App, Mode};
use crate::ui::settings::{toggle, SettingsItem, ALL_ITEMS, PR_UNAVAILABLE};
use crate::ui::grid::resolve_view;

/// Starts and stops the PR poller. `launch` spawns a poller that exits once
/// the flag it receives is set; stopping never joins.
pub struct PrPoller {
    launch: Box<dyn FnMut(Arc<AtomicBool>)>,
    stop: Option<Arc<AtomicBool>>,
}

impl PrPoller {
    pub fn new(launch: impl FnMut(Arc<AtomicBool>) + 'static) -> Self {
        Self { launch: Box::new(launch), stop: None }
    }

    /// No-op while already running.
    pub fn start(&mut self) {
        if self.stop.is_none() {
            let flag = Arc::new(AtomicBool::new(false));
            (self.launch)(flag.clone());
            self.stop = Some(flag);
        }
    }

    /// Sets the stop flag and forgets the poller; it winds down by itself.
    pub fn stop(&mut self) {
        if let Some(flag) = self.stop.take() {
            flag.store(true, Ordering::Relaxed);
        }
    }
}

/// True when none of `tools` is an existing file in a `PATH` directory.
/// Only looks at the file system (no process is spawned), so it cannot block.
fn tools_missing_in(path_var: Option<&OsString>, tools: &[&str]) -> bool {
    let Some(path_var) = path_var else { return true };
    !std::env::split_paths(path_var).any(|dir| tools.iter().any(|tool| dir.join(tool).is_file()))
}

/// True when neither `az` nor `gh` is on `PATH`.
pub fn pr_tools_missing_in(path_var: Option<OsString>) -> bool {
    tools_missing_in(path_var.as_ref(), &["az", "gh"])
}

/// Note for the settings window's PR item. When the PR poller has seen the
/// sessions' remotes (`needed` = their CLIs), names the missing ones
/// ("az 없음"); otherwise falls back to "neither az nor gh found".
pub fn pr_tools_note(path_var: Option<OsString>, needed: &[&str]) -> Option<String> {
    if needed.is_empty() {
        return pr_tools_missing_in(path_var).then(|| PR_UNAVAILABLE.to_string());
    }
    let missing: Vec<&str> =
        needed.iter().copied().filter(|t| tools_missing_in(path_var.as_ref(), &[t])).collect();
    (!missing.is_empty()).then(|| format!("{} 없음", missing.join("/")))
}

/// Footer text when the unreadable config could not be backed up (nothing saved).
pub const BACKUP_FAILED: &str = "설정 백업 실패: 파일을 덮어쓰지 않았습니다";

/// Footer text for a failed save: the short reason only, never a path.
fn save_error_text(err: &anyhow::Error) -> String {
    let reason = err.to_string();
    let reason = reason.lines().next().unwrap_or_default();
    format!("설정 저장 실패: {reason}")
}

impl App {
    pub(super) fn open_settings(&mut self) {
        self.mode = Mode::Settings;
        let needed = self.pr_cache.as_ref().map(|c| c.tools_seen()).unwrap_or_default();
        self.pr_tools_note = pr_tools_note(std::env::var_os("PATH"), &needed);
    }

    /// Settings-mode key handling. Esc / `i` / `q` close the window (they
    /// never quit); Ctrl-C quits.
    pub(super) fn handle_settings_key(&mut self, key: KeyEvent, now: Instant) {
        if key.kind != KeyEventKind::Press {
            return;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            if key.code == KeyCode::Char('c') {
                self.should_quit = true;
            }
            return;
        }
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                self.settings_cursor = (self.settings_cursor + 1).min(ALL_ITEMS.len() - 1);
            }
            KeyCode::Char('k') | KeyCode::Up => self.settings_cursor = self.settings_cursor.saturating_sub(1),
            KeyCode::Enter | KeyCode::Char(' ') => {
                let item = ALL_ITEMS[self.settings_cursor.min(ALL_ITEMS.len() - 1)];
                self.apply_setting(item, now);
            }
            KeyCode::Esc | KeyCode::Char('i' | 'q' | 'Q') => self.mode = Mode::Tiles,
            _ => {}
        }
    }

    /// Toggles `item` and applies it: in-memory config, the config shared
    /// with the refresh thread, the PR poller and the file. A failed save
    /// only shows a footer message; the new value stays active.
    pub(crate) fn apply_setting(&mut self, item: SettingsItem, now: Instant) {
        let pr_before = self.config.extras.pr;
        toggle(&mut self.config, item);
        self.view = resolve_view(self.config.view, self.sessions.len());
        if let Some(shared) = &self.shared_config {
            *shared.lock().unwrap_or_else(|e| e.into_inner()) = self.config.clone();
        }
        if self.config.extras.pr != pr_before {
            self.sync_pr_poller();
        }
        if let Some(path) = self.config_path.clone() {
            self.save_config(&path, now);
        }
    }

    /// Starts or stops the poller to match `config.extras.pr`.
    pub(super) fn sync_pr_poller(&mut self) {
        let Some(poller) = &mut self.pr_poller else { return };
        if self.config.extras.pr {
            poller.start();
        } else {
            poller.stop();
        }
    }

    /// Saves the config. After a failed load the original file is first
    /// copied to a free `.bak` name (once per run); if that copy fails the
    /// file is left alone.
    fn save_config(&mut self, path: &Path, now: Instant) {
        let mut backed_up = None;
        if self.backup_pending {
            if path.exists() {
                match crate::config::backup(path) {
                    Ok(bak) => backed_up = bak.file_name().map(|n| n.to_string_lossy().into_owned()),
                    Err(_) => {
                        self.notify(BACKUP_FAILED, now);
                        return;
                    }
                }
            }
            self.backup_pending = false;
        }
        match self.config.save(path) {
            Err(e) => self.notify(save_error_text(&e), now),
            Ok(()) => {
                if let Some(name) = backed_up {
                    self.notify_for(format!("기존 설정을 {name}에 백업했습니다"), now, super::NOTICE_DURATION);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{key_to_action, Action};
    use crate::config::{Config, ViewMode};
    use std::sync::atomic::AtomicUsize;
    use std::sync::Mutex;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn press(app: &mut App, code: KeyCode) {
        app.handle_settings_key(key(code), Instant::now());
    }

    /// App in settings mode saving to a file in `dir`.
    fn app_in(dir: &tempfile::TempDir) -> App {
        let mut app = App::new(Config::default());
        app.config_path = Some(dir.path().join("cfg").join("config.toml"));
        app.mode = Mode::Settings;
        app
    }

    #[test]
    fn settings_keys_move_and_toggle() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_in(&dir);
        assert_eq!(key_to_action(key(KeyCode::Char('j'))), Action::Move(crate::app::Dir::Down));
        press(&mut app, KeyCode::Char('k'));
        assert_eq!(app.settings_cursor, 0, "clamped at the top");
        press(&mut app, KeyCode::Char('j'));
        press(&mut app, KeyCode::Down);
        assert_eq!(app.settings_cursor, 2);
        press(&mut app, KeyCode::Up);
        assert_eq!(app.settings_cursor, 1);
        for _ in 0..20 {
            press(&mut app, KeyCode::Char('j'));
        }
        assert_eq!(app.settings_cursor, ALL_ITEMS.len() - 1, "clamped at the bottom");
        // Enter on GuessWorktree (default on) flips it, Space flips back
        press(&mut app, KeyCode::Enter);
        assert!(!app.config.guess_worktree);
        press(&mut app, KeyCode::Char(' '));
        assert!(app.config.guess_worktree);
        // tile navigation does not move tiles
        app.set_sessions(vec![crate::app::fake_session("a", crate::model::Status::Idle), crate::app::fake_session("b", crate::model::Status::Idle)], 0);
        app.cols = 2;
        press(&mut app, KeyCode::Char('l'));
        assert_eq!(app.selected, 0);
    }

    #[test]
    fn settings_q_closes_instead_of_quitting() {
        let dir = tempfile::tempdir().unwrap();
        for code in [KeyCode::Char('q'), KeyCode::Char('Q'), KeyCode::Char('i'), KeyCode::Esc] {
            let mut app = app_in(&dir);
            press(&mut app, code);
            assert_eq!(app.mode, Mode::Tiles, "{code:?}");
            assert!(!app.should_quit, "{code:?}");
        }
        let mut app = app_in(&dir);
        app.handle_settings_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL), Instant::now());
        assert!(app.should_quit, "Ctrl-C still quits");
    }

    #[test]
    fn settings_changes_persist_to_file() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_in(&dir);
        app.apply_setting(SettingsItem::View, Instant::now());
        app.apply_setting(SettingsItem::ExtraSummary, Instant::now());
        let loaded = Config::load(app.config_path.as_ref().unwrap());
        assert_eq!(loaded.view, ViewMode::A);
        assert!(!loaded.extras.summary);
        assert_eq!(loaded, app.config);
        assert_eq!(app.footer_message(Instant::now()), None);
    }

    #[test]
    fn first_save_after_failed_load_backs_up_the_original_once() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_in(&dir);
        let path = app.config_path.clone().unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "view = \"A\" # hand-edited\n").unwrap();
        let t0 = Instant::now();
        app.report_config_load_error(crate::config::LOAD_FAILED.into(), t0);
        assert_eq!(app.footer_message(t0), Some(crate::config::LOAD_FAILED));
        app.apply_setting(SettingsItem::ExtraSummary, t0);
        let bak = path.with_file_name("config.toml.bak");
        assert_eq!(std::fs::read_to_string(&bak).unwrap(), "view = \"A\" # hand-edited\n");
        let msg = app.footer_message(t0).unwrap().to_string();
        assert!(msg.contains("config.toml.bak"), "{msg}");
        assert!(!msg.contains(dir.path().to_str().unwrap()), "no full paths: {msg}");
        assert!(!Config::load(&path).extras.summary, "the change was saved");
        // later saves do not back up again
        app.apply_setting(SettingsItem::ExtraSummary, t0);
        assert!(!path.with_file_name("config.toml.bak.1").exists());
        assert_eq!(std::fs::read_to_string(&bak).unwrap(), "view = \"A\" # hand-edited\n");
    }

    #[test]
    fn existing_backup_is_not_clobbered() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_in(&dir);
        let path = app.config_path.clone().unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "broken again").unwrap();
        std::fs::write(path.with_file_name("config.toml.bak"), "older backup").unwrap();
        app.report_config_load_error(crate::config::LOAD_FAILED.into(), Instant::now());
        app.apply_setting(SettingsItem::ExtraSummary, Instant::now());
        assert_eq!(std::fs::read_to_string(path.with_file_name("config.toml.bak")).unwrap(), "older backup");
        assert_eq!(std::fs::read_to_string(path.with_file_name("config.toml.bak.1")).unwrap(), "broken again");
    }

    #[test]
    fn no_backup_without_load_error() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_in(&dir);
        let path = app.config_path.clone().unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "view = \"b\"\n").unwrap();
        app.apply_setting(SettingsItem::ExtraSummary, Instant::now());
        assert!(!path.with_file_name("config.toml.bak").exists());
    }

    #[test]
    fn load_error_notice_lasts_about_six_seconds() {
        let mut app = App::new(Config::default());
        let t0 = Instant::now();
        app.report_config_load_error(crate::config::LOAD_FAILED.into(), t0);
        assert_eq!(app.footer_message(t0 + std::time::Duration::from_secs(5)), Some(crate::config::LOAD_FAILED));
        assert_eq!(app.footer_message(t0 + crate::app::NOTICE_DURATION), None);
    }

    #[test]
    fn settings_save_failure_does_not_panic() {
        let dir = tempfile::tempdir().unwrap();
        // a regular file where the parent directory should be
        let blocker = dir.path().join("blocker");
        std::fs::write(&blocker, "x").unwrap();
        let mut app = App::new(Config::default());
        app.config_path = Some(blocker.join("config.toml"));
        let now = Instant::now();
        app.apply_setting(SettingsItem::ExtraSummary, now);
        assert!(!app.config.extras.summary, "in-memory value still changed");
        let msg = app.footer_message(now).expect("error shown in the footer");
        assert!(msg.starts_with("설정 저장 실패: "), "{msg}");
        assert!(!msg.contains(dir.path().to_str().unwrap()), "no full paths: {msg}");
        assert!(!app.should_quit);
    }

    #[test]
    fn settings_toggle_updates_shared_config_for_refresh_thread() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_in(&dir);
        let shared = Arc::new(Mutex::new(app.config.clone()));
        app.shared_config = Some(shared.clone());
        app.apply_setting(SettingsItem::ExtraBranch, Instant::now());
        assert!(!shared.lock().unwrap().extras.branch);
        assert_eq!(*shared.lock().unwrap(), app.config);
    }

    #[test]
    fn view_change_applies_to_resolved_view() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_in(&dir);
        app.apply_setting(SettingsItem::View, Instant::now()); // Auto -> A
        assert_eq!(app.view, crate::ui::tile::ViewKind::A);
        app.apply_setting(SettingsItem::View, Instant::now()); // A -> B
        assert_eq!(app.view, crate::ui::tile::ViewKind::B);
    }

    #[test]
    fn settings_pr_toggle_starts_and_stops_poller() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_in(&dir);
        let starts = Arc::new(AtomicUsize::new(0));
        let flags: Arc<Mutex<Vec<Arc<AtomicBool>>>> = Default::default();
        let (s2, f2) = (starts.clone(), flags.clone());
        app.pr_poller = Some(PrPoller::new(move |flag| {
            s2.fetch_add(1, Ordering::SeqCst);
            f2.lock().unwrap().push(flag);
        }));
        app.apply_setting(SettingsItem::ExtraBranch, Instant::now());
        assert_eq!(starts.load(Ordering::SeqCst), 0, "other items never touch the poller");
        app.apply_setting(SettingsItem::ExtraPr, Instant::now());
        assert!(app.config.extras.pr);
        assert_eq!(starts.load(Ordering::SeqCst), 1);
        assert!(!flags.lock().unwrap()[0].load(Ordering::SeqCst));
        app.start_pr_poller_if_enabled();
        assert_eq!(starts.load(Ordering::SeqCst), 1, "already running: no second poller");
        app.apply_setting(SettingsItem::ExtraPr, Instant::now());
        assert!(!app.config.extras.pr);
        assert!(flags.lock().unwrap()[0].load(Ordering::SeqCst), "stop flag set");
        app.apply_setting(SettingsItem::ExtraPr, Instant::now());
        assert_eq!(starts.load(Ordering::SeqCst), 2, "restart gets a fresh flag");
        assert!(!flags.lock().unwrap()[1].load(Ordering::SeqCst));
        app.shutdown();
        assert!(flags.lock().unwrap()[1].load(Ordering::SeqCst));
    }

    #[test]
    fn pr_tools_note_names_the_tool_the_remotes_need() {
        let dir = tempfile::tempdir().unwrap();
        let path = std::env::join_paths([dir.path()]).unwrap();
        std::fs::write(dir.path().join("gh"), "").unwrap();
        // remotes known: only the needed tool counts
        assert_eq!(pr_tools_note(Some(path.clone()), &["az"]).as_deref(), Some("az 없음"));
        assert_eq!(pr_tools_note(Some(path.clone()), &["gh"]), None);
        assert_eq!(pr_tools_note(None, &["az", "gh"]).as_deref(), Some("az/gh 없음"));
        // nothing known yet: generic check (either tool is enough)
        assert_eq!(pr_tools_note(Some(path), &[]), None);
        assert_eq!(pr_tools_note(None, &[]).as_deref(), Some(crate::ui::settings::PR_UNAVAILABLE));
    }

    #[test]
    fn pr_tools_detection_scans_path_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let path = std::env::join_paths([dir.path()]).unwrap();
        assert!(pr_tools_missing_in(Some(path.clone())));
        assert!(pr_tools_missing_in(None));
        std::fs::write(dir.path().join("gh"), "").unwrap();
        assert!(!pr_tools_missing_in(Some(path)));
    }
}
