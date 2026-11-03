use std::path::PathBuf;
use std::time::SystemTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Running,
    NeedsInput,
    Idle,
    Done,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrState {
    Draft,
    ReviewPending,
    Approved,
    Conflict,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pr {
    pub number: u64,
    pub title: String,
    pub state: PrState,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Session {
    pub id: String,
    pub name: String,
    pub status: Status,
    pub pid: u32,
    pub cwd: PathBuf,
    pub workspace_id: String,
    pub surface_id: String,
    pub started_at: Option<SystemTime>,
    pub updated_at: Option<SystemTime>,
    pub ended_at: Option<SystemTime>,
    pub summary: Option<String>,
    pub branch: Option<String>,
    pub branch_guessed: bool,
    pub pr: Option<Pr>,
    /// PR info is on but the lookup failed (tool missing, not logged in, …).
    pub pr_unavailable: bool,
    /// The hook's `transcriptPath` (branch guessing reads it).
    pub transcript: Option<PathBuf>,
    /// cmux window (uppercase id) the session's tab is in, from `cmux tree`;
    /// `None` when unknown.
    pub window_id: Option<String>,
}
