//! App state, key mapping, grid navigation, cmux focus and the event loop.

use std::collections::HashMap;
use std::path::PathBuf;
use std::io::Stdout;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Rect;
use ratatui::Terminal;

use crate::branch::{spawn_branch_resolver, BranchCache};
use crate::config::Config;
use crate::focus::{BackgroundFocus, FocusResult};
use crate::model::{Session, Status};
use crate::pr::{default_providers, spawn_poller, PrCache};
use crate::preview::{spawn_preview, PreviewCache, PreviewSlot, PreviewTarget, PREVIEW_INTERVAL};
use crate::refresh::{spawn_refresh, Snapshot};
use crate::source::cmux::{Cmux, CmuxCli};
use crate::ui::grid::resolve_view;
use crate::ui::groups::{layout_groups, move_selection, sort_by_window, Row};
use crate::ui::mascot::{done_oneshot_len, status_transition_fx, TileFx};
use crate::ui::preview::{PreviewBody, PREVIEW_DONE, PREVIEW_FAILED, PREVIEW_LOADING, PREVIEW_NO_SESSION};
use crate::ui::style::FPS;
use crate::ui::tile::ViewKind;
use crate::ui::screen;

mod settings_ctl;
pub use settings_ctl::PrPoller;

/// How long a transient footer message (e.g. a focus error) stays visible.
pub const FLASH_DURATION: Duration = Duration::from_secs(3);
/// How long a startup notice (e.g. an unreadable config file) stays visible.
pub const NOTICE_DURATION: Duration = Duration::from_secs(6);
/// Input poll timeout = one animation frame at [`FPS`].
pub const FRAME: Duration = Duration::from_millis(1000 / FPS);
/// Footer text when the selected session has no cmux ids.
pub const NO_TARGET: &str = "이동할 수 없는 세션";
/// Footer text while a focus call is running in the background.
pub const FOCUSING: &str = "이동 중…";
/// Footer text when cmux could not move the focus (details are not shown).
pub const FOCUS_FAILED: &str = "이동하지 못했습니다 (cmux 오류)";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    Left,
    Down,
    Up,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Move(Dir),
    Focus,
    TogglePreview,
    OpenSettings,
    Quit,
    None,
}

/// Which screen is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Tiles,
    Settings,
}

/// Maps a key press to an action: `h j k l` / arrows, Enter, `p`/`P`, `i`, `q`/`Q`, Ctrl-C.
/// Release and repeat events map to [`Action::None`].
pub fn key_to_action(key: KeyEvent) -> Action {
    if key.kind != KeyEventKind::Press {
        return Action::None;
    }
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        return match key.code {
            KeyCode::Char('c') => Action::Quit,
            _ => Action::None,
        };
    }
    match key.code {
        KeyCode::Char('h') | KeyCode::Left => Action::Move(Dir::Left),
        KeyCode::Char('j') | KeyCode::Down => Action::Move(Dir::Down),
        KeyCode::Char('k') | KeyCode::Up => Action::Move(Dir::Up),
        KeyCode::Char('l') | KeyCode::Right => Action::Move(Dir::Right),
        KeyCode::Enter => Action::Focus,
        KeyCode::Char('p' | 'P') => Action::TogglePreview,
        KeyCode::Char('i') => Action::OpenSettings,
        KeyCode::Char('q' | 'Q') => Action::Quit,
        _ => Action::None,
    }
}

/// Last status change of a session, in animation ticks.
#[derive(Debug, Clone, Copy)]
struct Transition {
    from: Status,
    to: Status,
    at: u64,
}

pub struct App {
    pub sessions: Vec<Session>,
    pub selected: usize,
    /// Columns of the last drawn grid (navigation uses it).
    pub cols: usize,
    pub view: ViewKind,
    pub config: Config,
    pub mode: Mode,
    /// First visible grid row.
    pub scroll: usize,
    pub should_quit: bool,
    /// False until the first snapshot arrived.
    pub loaded: bool,
    source_message: Option<String>,
    /// Footer message, when it was shown and for how long.
    flash: Option<(String, Instant, Duration)>,
    transitions: HashMap<String, Transition>,
    /// A background focus call has not reported back yet.
    focus_pending: bool,
    /// Selected row of the settings window.
    pub settings_cursor: usize,
    /// Why PR lookups cannot work, computed when the settings window opens
    /// (e.g. "az 없음"); `None` when the needed CLI is installed.
    pub pr_tools_note: Option<String>,
    /// PR availability note from the last snapshot (footer, dim).
    pub pr_note: Option<String>,
    /// Shared with the PR poller; tells which CLIs the remotes need.
    pr_cache: Option<Arc<PrCache>>,
    /// Where setting changes are saved; `None` = in-memory only.
    pub config_path: Option<PathBuf>,
    /// The config file existed but could not be loaded: back it up before
    /// the first save overwrites it.
    backup_pending: bool,
    /// Config shared with the refresh thread; kept in sync on every change.
    shared_config: Option<Arc<Mutex<Config>>>,
    pr_poller: Option<PrPoller>,
    /// The preview panel is toggled on (`p`); runtime only, off at launch.
    pub preview: bool,
    /// Filled by the preview thread; the UI only reads it.
    preview_cache: Arc<PreviewCache>,
    /// What the preview thread should read (`None` = nothing).
    preview_slot: PreviewSlot,
    /// Tile grid area of the last drawn frame.
    pub grid_rect: Rect,
    /// The preview is on but the last frame had no room for the panel.
    pub preview_too_small: bool,
    /// cmux windows in tree order (tiles are grouped by window).
    pub window_order: Vec<String>,
    /// The window holding the monitor itself, if known.
    pub current_window: Option<String>,
}

impl App {
    pub fn new(config: Config) -> Self {
        Self {
            sessions: Vec::new(),
            selected: 0,
            cols: 1,
            view: resolve_view(config.view, 0),
            config,
            mode: Mode::Tiles,
            scroll: 0,
            should_quit: false,
            loaded: false,
            source_message: None,
            flash: None,
            transitions: HashMap::new(),
            focus_pending: false,
            settings_cursor: 0,
            pr_tools_note: None,
            pr_note: None,
            pr_cache: None,
            config_path: None,
            backup_pending: false,
            shared_config: None,
            pr_poller: None,
            preview: false,
            preview_cache: Arc::new(PreviewCache::new()),
            preview_slot: Arc::new(Mutex::new(None)),
            grid_rect: Rect::default(),
            preview_too_small: false,
            window_order: Vec::new(),
            current_window: None,
        }
    }

    /// App wired to persistence: changes are saved to `path`, pushed into
    /// `shared_config` and start/stop PR polling through `pr_poller`.
    pub fn with_settings(
        config: Config,
        path: PathBuf,
        shared_config: Arc<Mutex<Config>>,
        pr_poller: PrPoller,
        pr_cache: Arc<PrCache>,
    ) -> Self {
        let mut app = Self::new(config);
        app.pr_cache = Some(pr_cache);
        app.config_path = Some(path);
        app.shared_config = Some(shared_config);
        app.pr_poller = Some(pr_poller);
        app
    }

    /// Starts the PR poller if PR info is enabled (call once at startup).
    pub fn start_pr_poller_if_enabled(&mut self) {
        self.sync_pr_poller();
    }

    /// Asks the PR poller to stop (never waits for it).
    pub fn shutdown(&mut self) {
        if let Some(p) = &mut self.pr_poller {
            p.stop();
        }
    }

    pub fn apply_snapshot(&mut self, snap: Snapshot, tick: u64) {
        self.source_message = snap.message;
        self.pr_note = snap.pr_note;
        self.window_order = snap.window_order;
        self.current_window = snap.current_window;
        self.loaded = true;
        self.set_sessions(snap.sessions, tick);
    }

    /// Replaces the list, keeping the selection on the same session id when it
    /// still exists (else the clamped index) and recording status changes.
    /// The list is put in window-group order (see [`crate::ui::groups`]).
    pub fn set_sessions(&mut self, mut sessions: Vec<Session>, tick: u64) {
        sort_by_window(&mut sessions, &self.window_order);
        let selected_id = self.selected_session().map(|s| s.id.clone());
        let mut transitions = HashMap::with_capacity(sessions.len());
        for s in &sessions {
            let t = match self.transitions.get(&s.id) {
                Some(t) if t.to == s.status => *t,
                Some(t) => Transition { from: t.to, to: s.status, at: tick },
                None => Transition { from: s.status, to: s.status, at: tick },
            };
            transitions.insert(s.id.clone(), t);
        }
        self.transitions = transitions;
        self.sessions = sessions;
        self.view = resolve_view(self.config.view, self.sessions.len());
        match selected_id.and_then(|id| self.sessions.iter().position(|s| s.id == id)) {
            Some(i) => self.selected = i,
            None => self.clamp_selection(),
        }
        self.sync_preview_target();
    }

    /// The preview thread's cache and target slot.
    pub fn preview_shared(&self) -> (Arc<PreviewCache>, PreviewSlot) {
        (self.preview_cache.clone(), self.preview_slot.clone())
    }

    /// `p`: shows or hides the preview panel.
    pub fn toggle_preview(&mut self) {
        self.preview = !self.preview;
        self.sync_preview_target();
    }

    /// What the preview thread should read now: the selected session while
    /// the preview is on, unless it is done or has no cmux ids.
    pub fn preview_target(&self) -> Option<PreviewTarget> {
        let s = self.selected_session().filter(|_| self.preview)?;
        if s.status == Status::Done || s.workspace_id.is_empty() || s.surface_id.is_empty() {
            return None;
        }
        Some(PreviewTarget { session_id: s.id.clone(), workspace_id: s.workspace_id.clone(), surface_id: s.surface_id.clone() })
    }

    /// Publishes [`App::preview_target`] to the preview thread (only on change).
    fn sync_preview_target(&mut self) {
        let want = self.preview_target();
        let mut slot = self.preview_slot.lock().unwrap_or_else(|e| e.into_inner());
        if *slot != want {
            *slot = want;
        }
    }

    /// What the panel should show for the selected session (cache read only).
    pub fn preview_body(&self) -> PreviewBody {
        let Some(s) = self.selected_session() else {
            return PreviewBody::Note(PREVIEW_NO_SESSION);
        };
        if s.status == Status::Done {
            return PreviewBody::Note(PREVIEW_DONE);
        }
        if s.workspace_id.is_empty() || s.surface_id.is_empty() {
            return PreviewBody::Note(NO_TARGET);
        }
        match self.preview_cache.get(&s.id).map(|e| e.screen) {
            Some(Ok(text)) => PreviewBody::Screen(text),
            Some(Err(())) => PreviewBody::Note(PREVIEW_FAILED),
            None => PreviewBody::Note(PREVIEW_LOADING),
        }
    }

    pub fn clamp_selection(&mut self) {
        self.selected = self.selected.min(self.sessions.len().saturating_sub(1));
    }

    pub fn selected_session(&self) -> Option<&Session> {
        self.sessions.get(self.selected)
    }

    /// Layout rows of the tile grid (window groups, `cols` columns); the
    /// renderer and the navigation both use it.
    pub fn grid_rows(&self) -> Vec<Row> {
        layout_groups(&self.sessions, &self.window_order, self.current_window.as_deref(), self.cols)
    }

    /// Grid move over [`App::grid_rows`], stopping at the edges. j/k cross
    /// window groups; a shorter target row clamps the column.
    pub fn apply_move(&mut self, dir: Dir) {
        if self.sessions.is_empty() {
            self.selected = 0;
            return;
        }
        self.clamp_selection();
        self.selected = move_selection(&self.grid_rows(), self.selected, dir);
    }

    /// Focuses the selected session's workspace and surface in cmux.
    /// No selection is a no-op; a session without ids is an error.
    pub fn focus_selected(&self, cmux: &dyn Cmux) -> Result<()> {
        let Some(s) = self.selected_session() else {
            return Ok(());
        };
        if s.workspace_id.is_empty() || s.surface_id.is_empty() {
            return Err(anyhow!(NO_TARGET));
        }
        cmux.focus(&s.workspace_id, &s.surface_id)
    }

    /// Shows `msg` in the footer for [`FLASH_DURATION`].
    pub fn notify(&mut self, msg: impl Into<String>, now: Instant) {
        self.notify_for(msg, now, FLASH_DURATION);
    }

    /// Shows `msg` in the footer for `duration`.
    pub fn notify_for(&mut self, msg: impl Into<String>, now: Instant, duration: Duration) {
        self.flash = Some((msg.into(), now, duration));
    }

    /// The config file exists but could not be loaded: tell the user (for
    /// [`NOTICE_DURATION`]) and back the file up before the first save.
    pub fn report_config_load_error(&mut self, msg: String, now: Instant) {
        self.backup_pending = true;
        self.notify_for(msg, now, NOTICE_DURATION);
    }

    /// Transient message if still fresh, else the source-failure note. The
    /// [`FOCUSING`] note stays for as long as the focus call is pending.
    pub fn footer_message(&self, now: Instant) -> Option<&str> {
        match &self.flash {
            Some((m, _, _)) if self.focus_pending && m == FOCUSING => Some(m),
            Some((m, at, dur)) if now.saturating_duration_since(*at) < *dur => Some(m),
            _ => self.source_message.as_deref(),
        }
    }

    /// The outcome of a background focus arrived (`now` = arrival time).
    /// A failure shows a short fixed message; cmux's own error text (argv,
    /// exit status) is not meant for the footer.
    pub fn focus_done(&mut self, result: FocusResult, now: Instant) {
        self.focus_pending = false;
        match result {
            Err(_) => self.notify(FOCUS_FAILED, now),
            Ok(()) => {
                if self.flash.as_ref().is_some_and(|(m, _, _)| m == FOCUSING) {
                    self.flash = None;
                }
            }
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent, cmux: &BackgroundFocus, now: Instant) {
        if self.mode == Mode::Settings {
            self.handle_settings_key(key, now);
            return;
        }
        let action = key_to_action(key);
        match action {
            Action::Move(d) => {
                self.apply_move(d);
                self.sync_preview_target();
            }
            Action::TogglePreview => self.toggle_preview(),
            // While a focus is in flight further Enter presses are ignored.
            Action::Focus if self.focus_pending => {}
            Action::Focus => match self.focus_selected(cmux) {
                Err(e) => {
                    let text = e.to_string();
                    self.notify(text.lines().next().unwrap_or_default(), now);
                }
                Ok(()) if self.selected_session().is_some() => {
                    // Dispatched to the worker; the outcome comes via `focus_done`.
                    self.focus_pending = true;
                    self.notify(FOCUSING, now);
                }
                Ok(()) => {}
            },
            Action::OpenSettings => self.open_settings(),
            Action::Quit => self.should_quit = true,
            Action::None => {}
        }
    }

    /// Whether status-change tracking still holds state for session `id`.
    #[cfg(test)]
    pub(crate) fn has_transition(&self, id: &str) -> bool {
        self.transitions.contains_key(id)
    }

    fn transition(&self, s: &Session, tick: u64) -> Option<(Transition, u64)> {
        let t = self.transitions.get(&s.id).filter(|t| t.to == s.status)?;
        Some((*t, tick.saturating_sub(t.at)))
    }

    /// Shake/flash for the tile of `s` at `tick`.
    pub fn tile_fx(&self, s: &Session, tick: u64) -> TileFx {
        self.transition(s, tick)
            .map(|(t, since)| status_transition_fx(t.from, t.to, since))
            .unwrap_or_default()
    }

    /// Ticks since `s` turned `Done` while the one-shot celebration is playing.
    pub fn done_oneshot_tick(&self, s: &Session, tick: u64) -> Option<u64> {
        let (t, since) = self.transition(s, tick)?;
        (t.to == Status::Done && t.from != Status::Done && since < done_oneshot_len()).then_some(since)
    }
}

/// The newest message waiting in `rx`, discarding older ones (non-blocking).
pub fn latest<T>(rx: &mpsc::Receiver<T>) -> Option<T> {
    let mut last = None;
    while let Ok(v) = rx.try_recv() {
        last = Some(v);
    }
    last
}

/// 12 fps animation tick since `start`.
pub fn tick_at(start: Instant, now: Instant) -> u64 {
    (now.saturating_duration_since(start).as_millis() * u128::from(FPS) / 1000) as u64
}

/// Runs the TUI until the user quits. Starts the refresh and branch threads
/// (and the PR poller when PR info is enabled); on exit only sets their stop
/// flags and drops the handles so quitting is instant.
/// `load_error` is the reason the config file could not be loaded, if any.
pub fn run(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    config: Config,
    load_error: Option<String>,
) -> Result<()> {
    let stop = Arc::new(AtomicBool::new(false));
    // Shared with the refresh thread; the settings window updates it too.
    let shared_config = Arc::new(Mutex::new(config.clone()));
    // Always handed to the refresh thread so PR info can be switched on at
    // runtime; the poller itself only runs while `extras.pr` is on.
    let cache = Arc::new(PrCache::new());
    let targets = Arc::new(Mutex::new(Vec::new()));
    let interval = Duration::from_secs(config.pr_poll_seconds.max(1));
    let poller = PrPoller::new({
        let (cache, targets) = (cache.clone(), targets.clone());
        move |flag| drop(spawn_poller(cache.clone(), targets.clone(), interval, default_providers(), flag))
    });
    // Branch resolution (git, pgrep/lsof, transcripts) never runs on the
    // refresh thread, so status updates never wait on it.
    let branch_cache = Arc::new(BranchCache::new());
    let branch_targets = Arc::new(Mutex::new(Vec::new()));
    drop(spawn_branch_resolver(
        branch_cache.clone(),
        branch_targets.clone(),
        shared_config.clone(),
        Arc::new(crate::worktree::resolve_branch),
        stop.clone(),
    ));
    let (tx, rx) = mpsc::channel();
    drop(spawn_refresh(
        tx,
        shared_config.clone(),
        Some((cache.clone(), targets)),
        Some((branch_cache, branch_targets)),
        stop.clone(),
    ));

    let cmux = BackgroundFocus::new(Arc::new(CmuxCli::new()));
    let mut app = App::with_settings(config, crate::config::default_path(), shared_config, poller, cache);
    app.start_pr_poller_if_enabled();
    // Screen reads for the preview never run on the UI or refresh thread.
    let (preview_cache, preview_slot) = app.preview_shared();
    drop(spawn_preview(preview_cache, preview_slot, Arc::new(CmuxCli::new()), PREVIEW_INTERVAL, stop.clone()));
    if let Some(msg) = load_error {
        app.report_config_load_error(msg, Instant::now());
    }
    let start = Instant::now();
    let result = (|| -> Result<()> {
        loop {
            let now = Instant::now();
            let tick = tick_at(start, now);
            // Only the newest snapshot matters; transitions compare it with
            // the previously applied one.
            if let Some(snap) = latest(&rx) {
                app.apply_snapshot(snap, tick);
            }
            while let Some(result) = cmux.try_result() {
                app.focus_done(result, Instant::now());
            }
            terminal.draw(|f| screen::draw(f, &mut app, tick, now))?;
            if event::poll(FRAME)? {
                if let Event::Key(key) = event::read()? {
                    app.handle_key(key, &cmux, Instant::now());
                }
            }
            if app.should_quit {
                return Ok(());
            }
        }
    })();
    stop.store(true, Ordering::Relaxed);
    app.shutdown();
    result
}

#[cfg(test)]
pub(crate) fn fake_session(id: &str, status: Status) -> Session {
    use std::path::PathBuf;
    Session {
        id: id.into(),
        name: format!("proj-{id}"),
        status,
        pid: 100,
        cwd: PathBuf::from(format!("/home/user/proj-{id}")),
        workspace_id: format!("ws-{id}"),
        surface_id: format!("sf-{id}"),
        started_at: None,
        updated_at: None,
        ended_at: None,
        summary: Some(format!("working on {id}")),
        branch: Some("main".into()),
        branch_guessed: false,
        pr: None,
        pr_unavailable: false,
        transcript: None,
        window_id: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::cmux::FakeCmux;
    use crossterm::event::KeyEventState;

    fn app_with(n: usize, cols: usize) -> App {
        let mut app = App::new(Config::default());
        let sessions = (0..n).map(|i| fake_session(&format!("s{i}"), Status::Idle)).collect();
        app.set_sessions(sessions, 0);
        app.cols = cols;
        app
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    /// A cmux whose focus takes `delay` and then returns `result`.
    struct SlowCmux {
        delay: Duration,
        result: std::result::Result<(), String>,
        calls: std::sync::atomic::AtomicUsize,
    }

    impl Cmux for SlowCmux {
        fn sessions(&self) -> Result<Vec<crate::source::cmux::CmuxSessionRef>> {
            Ok(vec![])
        }
        fn tree(&self) -> Result<crate::source::cmux::TreeLayout> {
            Ok(Default::default())
        }
        fn focus(&self, _: &str, _: &str) -> Result<()> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            std::thread::sleep(self.delay);
            self.result.clone().map_err(|e| anyhow!(e))
        }
        fn read_screen(&self, _: &str, _: &str) -> Result<String> {
            Ok(String::new())
        }
    }

    fn slow(delay_ms: u64, result: std::result::Result<(), String>) -> Arc<SlowCmux> {
        Arc::new(SlowCmux { delay: Duration::from_millis(delay_ms), result, calls: Default::default() })
    }

    fn bg(fake: FakeCmux) -> (BackgroundFocus, Arc<FakeCmux>) {
        let fake = Arc::new(fake);
        (BackgroundFocus::new(fake.clone()), fake)
    }

    /// Waits for the background focus result and hands it to the app.
    fn finish_focus(app: &mut App, focus: &BackgroundFocus) -> Instant {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if let Some(r) = focus.try_result() {
                let now = Instant::now();
                app.focus_done(r, now);
                return now;
            }
            assert!(Instant::now() < deadline, "focus result never arrived");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn ids(list: &[&str]) -> Vec<Session> {
        list.iter().map(|id| fake_session(id, Status::Idle)).collect()
    }

    #[test]
    fn move_right_then_left_returns() {
        let mut app = app_with(6, 3);
        app.apply_move(Dir::Right);
        assert_eq!(app.selected, 1);
        app.apply_move(Dir::Left);
        assert_eq!(app.selected, 0);
    }

    #[test]
    fn move_stops_at_edges() {
        let mut app = app_with(6, 3);
        app.apply_move(Dir::Left);
        app.apply_move(Dir::Up);
        assert_eq!(app.selected, 0);
        app.selected = 2;
        app.apply_move(Dir::Right); // end of row: no wrap
        assert_eq!(app.selected, 2);
        app.selected = 5;
        app.apply_move(Dir::Down);
        app.apply_move(Dir::Right);
        assert_eq!(app.selected, 5);
        app.selected = 3;
        app.apply_move(Dir::Left); // start of row: no wrap
        assert_eq!(app.selected, 3);
    }

    #[test]
    fn move_down_in_grid_uses_cols() {
        let mut app = app_with(7, 3);
        app.apply_move(Dir::Down);
        assert_eq!(app.selected, 3);
        app.apply_move(Dir::Down);
        assert_eq!(app.selected, 6);
        app.apply_move(Dir::Up);
        assert_eq!(app.selected, 3);
        // short last row: j from (row 1, col 2) clamps to the last tile
        app.selected = 5;
        app.apply_move(Dir::Down);
        assert_eq!(app.selected, 6);
        // last row: right stops at the last tile
        app.apply_move(Dir::Right);
        assert_eq!(app.selected, 6);
    }

    #[test]
    fn move_with_zero_sessions_does_not_panic() {
        let mut app = app_with(0, 3);
        for d in [Dir::Left, Dir::Down, Dir::Up, Dir::Right] {
            app.apply_move(d);
            assert_eq!(app.selected, 0);
        }
        app.cols = 0;
        app.apply_move(Dir::Down);
        let mut app = app_with(4, 0);
        app.apply_move(Dir::Down);
        assert_eq!(app.selected, 1);
    }

    #[test]
    fn selected_clamped_when_sessions_shrink() {
        let mut app = app_with(5, 3);
        app.selected = 4;
        app.set_sessions(ids(&["x", "y"]), 1);
        assert_eq!(app.selected, 1);
        app.set_sessions(Vec::new(), 2);
        assert_eq!(app.selected, 0);
        app.apply_move(Dir::Right);
        assert_eq!(app.selected, 0);
    }

    #[test]
    fn selection_follows_session_id_across_refresh() {
        let mut app = App::new(Config::default());
        app.set_sessions(ids(&["a", "b", "c"]), 0);
        app.selected = 1;
        app.set_sessions(ids(&["new", "a", "b", "c"]), 1);
        assert_eq!(app.selected_session().unwrap().id, "b");
        app.set_sessions(ids(&["new", "a", "c"]), 2);
        assert_eq!(app.selected, 2, "gone: falls back to the clamped index");
    }

    #[test]
    fn key_to_action_vim_and_arrows() {
        let cases = [
            (KeyCode::Char('h'), Action::Move(Dir::Left)),
            (KeyCode::Left, Action::Move(Dir::Left)),
            (KeyCode::Char('j'), Action::Move(Dir::Down)),
            (KeyCode::Down, Action::Move(Dir::Down)),
            (KeyCode::Char('k'), Action::Move(Dir::Up)),
            (KeyCode::Up, Action::Move(Dir::Up)),
            (KeyCode::Char('l'), Action::Move(Dir::Right)),
            (KeyCode::Right, Action::Move(Dir::Right)),
            (KeyCode::Enter, Action::Focus),
            (KeyCode::Char('i'), Action::OpenSettings),
            (KeyCode::Char('q'), Action::Quit),
            (KeyCode::Char('x'), Action::None),
            (KeyCode::Esc, Action::None),
        ];
        for (code, want) in cases {
            assert_eq!(key_to_action(key(code)), want, "{code:?}");
        }
        assert_eq!(key_to_action(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)), Action::Quit);
        assert_eq!(key_to_action(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::CONTROL)), Action::None);
    }

    #[test]
    fn key_p_toggles_preview_and_ctrl_p_is_ignored() {
        assert_eq!(key_to_action(key(KeyCode::Char('p'))), Action::TogglePreview);
        assert_eq!(key_to_action(KeyEvent::new(KeyCode::Char('P'), KeyModifiers::SHIFT)), Action::TogglePreview);
        assert_eq!(key_to_action(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL)), Action::None);
    }

    #[test]
    fn toggle_preview_flips_state_and_publishes_target() {
        let mut app = app_with(3, 3);
        let (cmux, _) = bg(FakeCmux::new(vec![]));
        let (_, slot) = app.preview_shared();
        assert!(!app.preview, "off at launch");
        assert_eq!(*slot.lock().unwrap(), None);
        app.handle_key(key(KeyCode::Char('p')), &cmux, Instant::now());
        assert!(app.preview);
        assert_eq!(slot.lock().unwrap().as_ref().map(|t| t.surface_id.clone()), Some("sf-s0".into()));
        // moving the selection retargets at once
        app.handle_key(key(KeyCode::Char('l')), &cmux, Instant::now());
        assert_eq!(slot.lock().unwrap().as_ref().map(|t| t.session_id.clone()), Some("s1".into()));
        // a Done or id-less selection reads nothing and shows a note
        app.sessions[1].status = Status::Done;
        app.set_sessions(app.sessions.clone(), 1);
        assert_eq!(*slot.lock().unwrap(), None);
        assert_eq!(app.preview_body(), PreviewBody::Note(PREVIEW_DONE));
        app.sessions[1].status = Status::Idle;
        app.sessions[1].surface_id.clear();
        app.set_sessions(app.sessions.clone(), 2);
        assert_eq!(*slot.lock().unwrap(), None);
        assert_eq!(app.preview_body(), PreviewBody::Note(NO_TARGET));
        app.handle_key(key(KeyCode::Char('h')), &cmux, Instant::now());
        assert_eq!(app.preview_body(), PreviewBody::Note(PREVIEW_LOADING), "nothing read yet");
        // off again: the thread gets no target
        app.handle_key(key(KeyCode::Char('P')), &cmux, Instant::now());
        assert!(!app.preview);
        assert_eq!(*slot.lock().unwrap(), None);
        // p in the settings window does nothing special
        app.handle_key(key(KeyCode::Char('i')), &cmux, Instant::now());
        app.handle_key(key(KeyCode::Char('p')), &cmux, Instant::now());
        assert!(!app.preview);
        assert_eq!(app.mode, Mode::Settings);
    }

    #[test]
    fn preview_body_reads_the_cache() {
        use crate::preview::PreviewEntry;
        let mut app = app_with(2, 3);
        app.preview = true;
        let (cache, _) = app.preview_shared();
        cache.put("s0", PreviewEntry { screen: Ok("hello".into()), fetched_at: Instant::now() });
        assert_eq!(app.preview_body(), PreviewBody::Screen("hello".into()));
        cache.put("s0", PreviewEntry { screen: Err(()), fetched_at: Instant::now() });
        assert_eq!(app.preview_body(), PreviewBody::Note(PREVIEW_FAILED));
        assert_eq!(App::new(Config::default()).preview_body(), PreviewBody::Note(PREVIEW_NO_SESSION));
    }

    fn in_window(id: &str, w: &str) -> Session {
        let mut s = fake_session(id, Status::Idle);
        s.window_id = Some(w.into());
        s
    }

    #[test]
    fn selection_follows_session_id_when_it_changes_window() {
        let mut app = App::new(Config::default());
        let snap = |sessions: Vec<Session>| Snapshot {
            sessions,
            window_order: vec!["W1".into(), "W2".into()],
            ..Default::default()
        };
        app.apply_snapshot(snap(vec![in_window("a", "W1"), in_window("b", "W2"), in_window("c", "W1")]), 0);
        assert_eq!(app.sessions.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["a", "c", "b"]);
        app.selected = 2; // b
        // b moves to window 1 and a new window appears
        app.apply_snapshot(snap(vec![in_window("a", "W1"), in_window("b", "W1"), in_window("c", "W2")]), 1);
        assert_eq!(app.sessions.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["a", "b", "c"]);
        assert_eq!(app.selected_session().unwrap().id, "b");
        // windows info gone (tree failed): one list, still on b
        app.apply_snapshot(Snapshot { sessions: ids(&["a", "b", "c"]), ..Default::default() }, 2);
        assert_eq!(app.selected_session().unwrap().id, "b");
        // j/k cross the group boundary with one column
        app.apply_snapshot(snap(vec![in_window("a", "W1"), in_window("b", "W2")]), 3);
        app.cols = 4;
        app.selected = 0;
        app.apply_move(Dir::Down);
        assert_eq!(app.selected_session().unwrap().id, "b");
        app.apply_move(Dir::Left);
        assert_eq!(app.selected_session().unwrap().id, "b", "h does not cross groups");
        app.apply_move(Dir::Up);
        assert_eq!(app.selected_session().unwrap().id, "a");
    }

    #[test]
    fn key_release_events_ignored() {
        for kind in [KeyEventKind::Release, KeyEventKind::Repeat] {
            let ev = KeyEvent::new_with_kind_and_state(KeyCode::Char('q'), KeyModifiers::NONE, kind, KeyEventState::NONE);
            assert_eq!(key_to_action(ev), Action::None);
        }
        let mut app = app_with(3, 3);
        let ev = KeyEvent::new_with_kind_and_state(KeyCode::Char('l'), KeyModifiers::NONE, KeyEventKind::Release, KeyEventState::NONE);
        app.handle_key(ev, &bg(FakeCmux::new(vec![])).0, Instant::now());
        assert_eq!(app.selected, 0);
    }

    #[test]
    fn focus_selected_calls_cmux_with_ids() {
        let mut app = app_with(3, 3);
        app.selected = 2;
        let cmux = FakeCmux::new(vec![]);
        app.focus_selected(&cmux).unwrap();
        assert_eq!(cmux.calls(), vec!["focus ws-s2 sf-s2".to_string()]);
        // Done sessions can still be focused (through the background worker)
        app.sessions[2].status = Status::Done;
        let (focus, fake) = bg(FakeCmux::new(vec![]));
        app.handle_key(key(KeyCode::Enter), &focus, Instant::now());
        finish_focus(&mut app, &focus);
        assert_eq!(fake.calls(), vec!["focus ws-s2 sf-s2".to_string()]);
        assert_eq!(app.footer_message(Instant::now()), None, "success clears the pending note");
        // no sessions: no call, no error
        let empty = app_with(0, 3);
        empty.focus_selected(&cmux).unwrap();
        assert_eq!(cmux.calls().len(), 1);
    }

    #[test]
    fn focus_error_is_returned_not_panicked() {
        let mut app = app_with(1, 3);
        let mut cmux = FakeCmux::new(vec![]);
        cmux.focus_result = Err("no such surface".into());
        let err = app.focus_selected(&cmux).unwrap_err();
        assert!(err.to_string().contains("no such surface"));

        let (focus, _) = bg(cmux);
        app.handle_key(key(KeyCode::Enter), &focus, Instant::now());
        let done = finish_focus(&mut app, &focus);
        assert!(!app.should_quit);
        assert_eq!(app.footer_message(done), Some(FOCUS_FAILED), "short message, no raw cmux text");
        assert_eq!(app.footer_message(done + FLASH_DURATION), None);
    }

    #[test]
    fn focus_session_without_ids_reports_message() {
        let mut app = app_with(1, 3);
        app.sessions[0].surface_id.clear();
        let cmux = FakeCmux::new(vec![]);
        assert_eq!(app.focus_selected(&cmux).unwrap_err().to_string(), NO_TARGET);
        let (focus, fake) = bg(cmux);
        let t0 = Instant::now();
        app.handle_key(key(KeyCode::Enter), &focus, t0);
        assert_eq!(app.footer_message(t0), Some(NO_TARGET));
        std::thread::sleep(Duration::from_millis(50));
        assert!(fake.calls().is_empty());
        assert!(focus.try_result().is_none());
    }

    #[test]
    fn focus_runs_off_the_ui_thread() {
        let mut app = app_with(2, 3);
        let cmux = slow(500, Err("cmux timed out".into()));
        let focus = BackgroundFocus::new(cmux.clone());
        let t0 = Instant::now();
        app.handle_key(key(KeyCode::Enter), &focus, t0);
        assert!(t0.elapsed() < Duration::from_millis(100), "event-loop step blocked for {:?}", t0.elapsed());
        assert_eq!(app.footer_message(t0), Some(FOCUSING));
        let done = finish_focus(&mut app, &focus);
        assert!(done - t0 >= Duration::from_millis(500));
        // the error is stamped at completion, so it stays a full FLASH_DURATION after it
        let late = done + FLASH_DURATION - Duration::from_millis(100);
        assert!(late > t0 + FLASH_DURATION);
        assert_eq!(app.footer_message(late), Some(FOCUS_FAILED));
        assert_eq!(app.footer_message(done + FLASH_DURATION), None);
    }

    #[test]
    fn focusing_note_stays_while_focus_is_pending() {
        let mut app = app_with(2, 3);
        let cmux = slow(300, Ok(()));
        let focus = BackgroundFocus::new(cmux.clone());
        let t0 = Instant::now();
        app.handle_key(key(KeyCode::Enter), &focus, t0);
        // long after FLASH_DURATION, still pending: the note stays
        assert_eq!(app.footer_message(t0 + FLASH_DURATION * 5), Some(FOCUSING));
        let done = finish_focus(&mut app, &focus);
        assert_eq!(app.footer_message(done), None, "cleared by the result");
    }

    #[test]
    fn focus_enter_ignored_while_in_flight() {
        let mut app = app_with(2, 3);
        let cmux = slow(200, Ok(()));
        let focus = BackgroundFocus::new(cmux.clone());
        for _ in 0..3 {
            app.handle_key(key(KeyCode::Enter), &focus, Instant::now());
        }
        finish_focus(&mut app, &focus);
        std::thread::sleep(Duration::from_millis(250));
        assert_eq!(cmux.calls.load(Ordering::SeqCst), 1, "no pile-up");
        assert!(focus.try_result().is_none());
        // once done, Enter works again
        app.handle_key(key(KeyCode::Enter), &focus, Instant::now());
        finish_focus(&mut app, &focus);
        assert_eq!(cmux.calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn latest_keeps_only_newest_snapshot() {
        let (tx, rx) = mpsc::channel();
        assert_eq!(latest::<u32>(&rx), None);
        for i in 1..=3 {
            tx.send(i).unwrap();
        }
        assert_eq!(latest(&rx), Some(3));
        assert_eq!(latest(&rx), None);
    }

    #[test]
    fn uppercase_q_quits() {
        assert_eq!(key_to_action(KeyEvent::new(KeyCode::Char('Q'), KeyModifiers::SHIFT)), Action::Quit);
        assert_eq!(key_to_action(KeyEvent::new(KeyCode::Char('H'), KeyModifiers::SHIFT)), Action::None);
    }

    #[test]
    fn source_message_shown_after_flash_expires() {
        let mut app = App::new(Config::default());
        app.apply_snapshot(Snapshot { message: Some("cmux unavailable".into()), ..Default::default() }, 0);
        let t0 = Instant::now();
        assert_eq!(app.footer_message(t0), Some("cmux unavailable"));
        app.notify("boom", t0);
        assert_eq!(app.footer_message(t0), Some("boom"));
        assert_eq!(app.footer_message(t0 + FLASH_DURATION), Some("cmux unavailable"));
    }

    #[test]
    fn settings_mode_toggles_and_blocks_navigation() {
        let mut app = app_with(3, 3);
        let (cmux, fake) = bg(FakeCmux::new(vec![]));
        let now = Instant::now();
        app.handle_key(key(KeyCode::Char('i')), &cmux, now);
        assert_eq!(app.mode, Mode::Settings);
        app.handle_key(key(KeyCode::Char('l')), &cmux, now);
        app.handle_key(key(KeyCode::Enter), &cmux, now);
        assert_eq!(app.selected, 0);
        std::thread::sleep(Duration::from_millis(50));
        assert!(fake.calls().is_empty());
        app.handle_key(key(KeyCode::Esc), &cmux, now);
        assert_eq!(app.mode, Mode::Tiles);
        app.handle_key(key(KeyCode::Char('i')), &cmux, now);
        app.handle_key(key(KeyCode::Char('i')), &cmux, now);
        assert_eq!(app.mode, Mode::Tiles);
        app.handle_key(key(KeyCode::Char('q')), &cmux, now);
        assert!(app.should_quit);
    }

    #[test]
    fn view_follows_session_count_in_auto() {
        let mut app = app_with(6, 3);
        assert_eq!(app.view, ViewKind::B);
        app.set_sessions(ids(&["a", "b", "c", "d", "e", "f", "g"]), 0);
        assert_eq!(app.view, ViewKind::A);
    }

    #[test]
    fn transition_tracking_sets_fx_on_status_change() {
        let mut app = App::new(Config::default());
        app.set_sessions(vec![fake_session("a", Status::Idle)], 0);
        let s = app.sessions[0].clone();
        assert_eq!(app.tile_fx(&s, 0), TileFx::default(), "first sighting has no fx");

        app.set_sessions(vec![fake_session("a", Status::NeedsInput)], 10);
        let s = app.sessions[0].clone();
        assert_eq!(app.tile_fx(&s, 10), status_transition_fx(Status::Idle, Status::NeedsInput, 0));
        assert_ne!(app.tile_fx(&s, 10), TileFx::default());
        assert_eq!(app.tile_fx(&s, 11), status_transition_fx(Status::Idle, Status::NeedsInput, 1));
        assert_eq!(app.tile_fx(&s, 30), TileFx::default());

        // unchanged status in a later refresh keeps the original change time
        app.set_sessions(vec![fake_session("a", Status::NeedsInput)], 11);
        assert_eq!(app.tile_fx(&s, 11), status_transition_fx(Status::Idle, Status::NeedsInput, 1));
    }

    #[test]
    fn done_oneshot_plays_once_then_holds() {
        let mut app = App::new(Config::default());
        app.set_sessions(vec![fake_session("a", Status::Running)], 0);
        app.set_sessions(vec![fake_session("a", Status::Done)], 5);
        let s = app.sessions[0].clone();
        let len = done_oneshot_len();
        assert_eq!(app.done_oneshot_tick(&s, 5), Some(0));
        assert_eq!(app.done_oneshot_tick(&s, 5 + len - 1), Some(len - 1));
        assert_eq!(app.done_oneshot_tick(&s, 5 + len), None);
        assert_eq!(app.done_oneshot_tick(&s, 5 + 10 * len), None);
        // a session first seen as Done does not celebrate
        let mut app = App::new(Config::default());
        app.set_sessions(vec![fake_session("b", Status::Done)], 0);
        assert_eq!(app.done_oneshot_tick(&app.sessions[0].clone(), 0), None);
    }

    #[test]
    fn tick_is_twelve_per_second() {
        let t0 = Instant::now();
        assert_eq!(tick_at(t0, t0), 0);
        assert_eq!(tick_at(t0, t0 + Duration::from_secs(1)), 12);
        assert_eq!(tick_at(t0, t0 + Duration::from_millis(2500)), 30);
    }
}
