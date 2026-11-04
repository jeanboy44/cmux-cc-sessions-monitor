use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use super::lenient;

#[derive(Debug, Clone, PartialEq)]
pub struct ClaudeSessionRecord {
    pub pid: u32,
    pub session_id: String,
    pub cwd: PathBuf,
    pub name: Option<String>,
    pub status: Option<String>,
    pub started_at_ms: Option<u64>,
    pub updated_at_ms: Option<u64>,
    /// When `status` last changed (`statusUpdatedAt`, epoch ms).
    pub status_updated_at_ms: Option<u64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Raw {
    #[serde(default, deserialize_with = "lenient")]
    pid: Option<u32>,
    #[serde(default, deserialize_with = "lenient")]
    session_id: Option<String>,
    #[serde(default, deserialize_with = "lenient")]
    cwd: Option<PathBuf>,
    #[serde(default, deserialize_with = "lenient")]
    name: Option<String>,
    #[serde(default, deserialize_with = "lenient")]
    status: Option<String>,
    #[serde(default, deserialize_with = "lenient")]
    started_at: Option<u64>,
    #[serde(default, deserialize_with = "lenient")]
    updated_at: Option<u64>,
    #[serde(default, deserialize_with = "lenient")]
    status_updated_at: Option<u64>,
}

fn parse(text: &str) -> Option<ClaudeSessionRecord> {
    let raw: Raw = serde_json::from_str(text).ok()?;
    Some(ClaudeSessionRecord {
        pid: raw.pid?,
        session_id: raw.session_id?,
        cwd: raw.cwd.unwrap_or_default(),
        name: raw.name,
        status: raw.status,
        started_at_ms: raw.started_at,
        updated_at_ms: raw.updated_at,
        status_updated_at_ms: raw.status_updated_at,
    })
}

/// Reads every parseable `*.json` file in `dir`; anything else is skipped.
pub fn read_claude_sessions(dir: &Path) -> Vec<ClaudeSessionRecord> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .filter_map(|p| fs::read_to_string(p).ok())
        .filter_map(|t| parse(&t))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    const FIXTURE: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/claude_session_busy.json"));

    #[test]
    fn claude_sessions_parses_fixture() {
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join("4242.json"), FIXTURE).unwrap();
        let r = read_claude_sessions(d.path());
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].pid, 4242);
        assert_eq!(r[0].status.as_deref(), Some("busy"));
        assert_eq!(r[0].cwd, PathBuf::from("/home/user/proj"));
        assert_eq!(r[0].started_at_ms, Some(1700000000000));
        assert_eq!(r[0].updated_at_ms, Some(1700000005000));
        assert_eq!(r[0].status_updated_at_ms, Some(1700000005000));
        assert_eq!(r[0].name.as_deref(), Some("demo"));
    }

    #[test]
    fn claude_sessions_skips_broken_file() {
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join("4242.json"), FIXTURE).unwrap();
        fs::write(d.path().join("1.json"), "{").unwrap();
        fs::write(d.path().join("2.json"), "").unwrap();
        fs::write(d.path().join("3.json"), [0xff, 0xfe, 0x00, 0x80]).unwrap();
        let r = read_claude_sessions(d.path());
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].pid, 4242);
    }

    #[test]
    fn claude_sessions_ignores_unknown_fields() {
        let d = tempfile::tempdir().unwrap();
        fs::write(
            d.path().join("7.json"),
            r#"{"pid":7,"sessionId":"s","cwd":"/home/user/p","brandNew":[1,2],"status":"idle"}"#,
        )
        .unwrap();
        let r = read_claude_sessions(d.path());
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].status.as_deref(), Some("idle"));
        assert_eq!(r[0].name, None);
    }

    #[test]
    fn claude_sessions_wrong_typed_optional_degrades_to_none() {
        let d = tempfile::tempdir().unwrap();
        fs::write(
            d.path().join("8.json"),
            r#"{"pid":8,"sessionId":"s","cwd":"/home/user/p","status":5,"name":["x"],"startedAt":"soon","updatedAt":-1}"#,
        )
        .unwrap();
        let r = read_claude_sessions(d.path());
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].status, None);
        assert_eq!(r[0].name, None);
        assert_eq!(r[0].started_at_ms, None);
        assert_eq!(r[0].updated_at_ms, None);
        assert_eq!(r[0].status_updated_at_ms, None);
    }

    #[test]
    fn claude_sessions_status_updated_at_is_its_own_field() {
        let d = tempfile::tempdir().unwrap();
        fs::write(
            d.path().join("9.json"),
            r#"{"pid":9,"sessionId":"s","cwd":"/home/user/p","status":"busy","updatedAt":1700000009000,"statusUpdatedAt":1700000001000}"#,
        )
        .unwrap();
        let r = read_claude_sessions(d.path());
        assert_eq!(r[0].updated_at_ms, Some(1700000009000));
        assert_eq!(r[0].status_updated_at_ms, Some(1700000001000));
    }

    #[test]
    fn claude_sessions_skips_missing_or_wrong_typed_identity() {
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join("a.json"), r#"{"sessionId":"s","cwd":"/x"}"#).unwrap();
        fs::write(d.path().join("b.json"), r#"{"pid":"nine","sessionId":"s","cwd":"/x"}"#).unwrap();
        fs::write(d.path().join("c.json"), r#"{"pid":3,"cwd":"/x"}"#).unwrap();
        fs::write(d.path().join("d.json"), "[1,2]").unwrap();
        assert!(read_claude_sessions(d.path()).is_empty());
    }

    #[test]
    fn claude_sessions_ignores_non_json_files() {
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join("4242.abc123.key"), FIXTURE).unwrap();
        fs::write(d.path().join("notes.txt"), FIXTURE).unwrap();
        assert!(read_claude_sessions(d.path()).is_empty());
    }

    #[test]
    fn claude_sessions_missing_dir_is_empty() {
        assert!(read_claude_sessions(Path::new("/nonexistent/dir/for/test")).is_empty());
    }
}
