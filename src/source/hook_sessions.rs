use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Deserializer};

use super::lenient;

#[derive(Debug, Clone, PartialEq)]
pub struct HookRecord {
    pub session_id: String,
    pub pid: Option<u32>,
    pub workspace_id: String,
    pub surface_id: String,
    pub cwd: PathBuf,
    pub agent_lifecycle: Option<String>,
    pub hook_event_name: Option<String>,
    pub last_body: Option<String>,
    pub transcript_path: Option<PathBuf>,
    pub updated_at: Option<String>,
}

/// `updatedAt` may be a number or a string; numbers are stringified.
fn string_or_number<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    Ok(match serde_json::Value::deserialize(d)? {
        serde_json::Value::String(s) => Some(s),
        serde_json::Value::Number(n) => Some(n.to_string()),
        _ => None,
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Raw {
    #[serde(default, deserialize_with = "lenient")]
    session_id: Option<String>,
    #[serde(default, deserialize_with = "lenient")]
    pid: Option<u32>,
    #[serde(default, deserialize_with = "lenient")]
    workspace_id: Option<String>,
    #[serde(default, deserialize_with = "lenient")]
    surface_id: Option<String>,
    #[serde(default, deserialize_with = "lenient")]
    cwd: Option<PathBuf>,
    #[serde(default, deserialize_with = "lenient")]
    agent_lifecycle: Option<String>,
    #[serde(default, deserialize_with = "lenient")]
    hook_event_name: Option<String>,
    #[serde(default, deserialize_with = "lenient")]
    last_body: Option<String>,
    #[serde(default, deserialize_with = "lenient")]
    transcript_path: Option<PathBuf>,
    #[serde(default, deserialize_with = "string_or_number")]
    updated_at: Option<String>,
}

impl Raw {
    fn into_record(self) -> Option<HookRecord> {
        Some(HookRecord {
            session_id: self.session_id?,
            pid: self.pid,
            workspace_id: self.workspace_id.unwrap_or_default(),
            surface_id: self.surface_id.unwrap_or_default(),
            cwd: self.cwd.unwrap_or_default(),
            agent_lifecycle: self.agent_lifecycle,
            hook_event_name: self.hook_event_name,
            last_body: self.last_body,
            transcript_path: self.transcript_path,
            updated_at: self.updated_at,
        })
    }
}

#[derive(Deserialize)]
struct File {
    #[serde(default, deserialize_with = "lenient")]
    sessions: Option<HashMap<String, serde_json::Value>>,
}

/// Reads the values of the top-level `sessions` map; broken input yields an empty Vec.
pub fn read_hook_sessions(path: &Path) -> Vec<HookRecord> {
    let Ok(text) = fs::read_to_string(path) else {
        return Vec::new();
    };
    let Ok(file) = serde_json::from_str::<File>(&text) else {
        return Vec::new();
    };
    file.sessions
        .unwrap_or_default()
        .into_values()
        .filter_map(|v| serde_json::from_value::<Raw>(v).ok())
        .filter_map(Raw::into_record)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    const FIXTURE: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/hook_sessions.json"));

    fn write(s: &str) -> (tempfile::TempDir, PathBuf) {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("hooks.json");
        fs::write(&p, s).unwrap();
        (d, p)
    }

    #[test]
    fn hook_sessions_parses_fixture() {
        let (_d, p) = write(FIXTURE);
        let mut r = read_hook_sessions(&p);
        r.sort_by(|a, b| a.session_id.cmp(&b.session_id));
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].agent_lifecycle.as_deref(), Some("running"));
        assert_eq!(r[0].pid, Some(4242));
        assert_eq!(r[0].hook_event_name.as_deref(), Some("PreToolUse"));
        assert_eq!(r[0].last_body.as_deref(), Some("Running tests"));
        assert_eq!(r[0].cwd, PathBuf::from("/home/user/proj"));
        assert_eq!(r[0].updated_at.as_deref(), Some("2026-11-03T10:00:00Z"));
        assert!(r[0].transcript_path.is_some());
        assert_eq!(r[1].agent_lifecycle.as_deref(), Some("idle"));
        assert_eq!(r[1].updated_at.as_deref(), Some("1700000009"));
        assert_eq!(r[1].pid, None);
    }

    #[test]
    fn hook_sessions_missing_file_is_empty() {
        assert!(read_hook_sessions(Path::new("/nonexistent/hooks.json")).is_empty());
    }

    #[test]
    fn hook_sessions_missing_optional_fields_ok() {
        let (_d, p) = write(r#"{"sessions":{"k":{"sessionId":"s1"}}}"#);
        let r = read_hook_sessions(&p);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].session_id, "s1");
        assert_eq!(r[0].workspace_id, "");
        assert_eq!(r[0].surface_id, "");
        assert_eq!(r[0].cwd, PathBuf::new());
        assert_eq!(r[0].agent_lifecycle, None);
        assert_eq!(r[0].transcript_path, None);
    }

    #[test]
    fn hook_sessions_broken_or_odd_files_are_empty() {
        for s in ["{", "", "[]", r#"{"sessions":[1]}"#, r#"{"other":1}"#, r#"{"sessions":"x"}"#] {
            let (_d, p) = write(s);
            assert!(read_hook_sessions(&p).is_empty(), "input {s:?}");
        }
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("bin.json");
        fs::write(&p, [0xff, 0x00, 0xfe]).unwrap();
        assert!(read_hook_sessions(&p).is_empty());
    }

    #[test]
    fn hook_sessions_bad_entry_skipped_others_kept() {
        let (_d, p) = write(
            r#"{"sessions":{"a":"junk","b":{"cwd":"/x"},"c":{"sessionId":7},"d":{"sessionId":"ok","pid":"x","agentLifecycle":3,"lastBody":null,"updatedAt":true}}}"#,
        );
        let r = read_hook_sessions(&p);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].session_id, "ok");
        assert_eq!(r[0].pid, None);
        assert_eq!(r[0].agent_lifecycle, None);
        assert_eq!(r[0].updated_at, None);
    }
}
