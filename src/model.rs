use std::path::PathBuf;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

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
    /// Context-window usage in percent (0..=100), `None` when unknown.
    pub ctx_percent: Option<f32>,
    /// Model of the last main-chain assistant reply in the transcript
    /// (e.g. `claude-opus-4-5`), `None` when unknown.
    pub model: Option<String>,
    /// cmux holds an unread notification for this session's surface
    /// (`cmux list-notifications`); the card border blinks while it does.
    pub unread_notification: bool,
}

/// Claude model family; each one maps to a spirit element in the config.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModelFamily {
    Fable,
    Opus,
    Sonnet,
    Haiku,
}

impl ModelFamily {
    pub const ALL: [ModelFamily; 4] = [ModelFamily::Fable, ModelFamily::Opus, ModelFamily::Sonnet, ModelFamily::Haiku];

    /// Lowercase name, used as the config key.
    pub fn as_key(self) -> &'static str {
        match self {
            ModelFamily::Fable => "fable",
            ModelFamily::Opus => "opus",
            ModelFamily::Sonnet => "sonnet",
            ModelFamily::Haiku => "haiku",
        }
    }
}

/// Family of a model id: a case-insensitive search for the family word, so
/// dated ids, provider prefixes (`us.anthropic.…`) and suffixes (`[1m]`) all
/// work. Anything else (empty, `<synthetic>`, other vendors) is `None`.
pub fn model_family(model: &str) -> Option<ModelFamily> {
    let lower = model.to_ascii_lowercase();
    ModelFamily::ALL.into_iter().find(|f| lower.contains(f.as_key()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_family_detects_every_spelling() {
        use ModelFamily::*;
        let cases: &[(&str, Option<ModelFamily>)] = &[
            ("claude-opus-4-5", Some(Opus)),
            ("claude-opus-4-1-20250805", Some(Opus)),
            ("claude-fable-5-1", Some(Fable)),
            ("claude-sonnet-5-5", Some(Sonnet)),
            ("claude-haiku-5-5", Some(Haiku)),
            ("us.anthropic.claude-opus-4-5-v1:0", Some(Opus)),
            ("claude-sonnet-5-5[1m]", Some(Sonnet)),
            ("CLAUDE-HAIKU-5-5", Some(Haiku)),
            ("Opus", Some(Opus)),
            ("fable", Some(Fable)),
            ("claude-3-5-sonnet-20241022", Some(Sonnet)),
            ("", None),
            ("   ", None),
            ("<synthetic>", None),
            ("gpt-5", None),
            ("claude-mystery-9", None),
        ];
        for (model, want) in cases {
            assert_eq!(model_family(model), *want, "{model:?}");
        }
    }

    #[test]
    fn model_family_keys_are_lowercase_names() {
        let keys: Vec<&str> = ModelFamily::ALL.iter().map(|f| f.as_key()).collect();
        assert_eq!(keys, ["fable", "opus", "sonnet", "haiku"]);
        for f in ModelFamily::ALL {
            assert_eq!(serde_json::to_string(&f).unwrap(), format!("\"{}\"", f.as_key()));
            assert_eq!(model_family(f.as_key()), Some(f));
        }
    }
}
