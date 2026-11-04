//! Per-session PR meta written by the user's own hook:
//! `<home>/.claude/session-meta/<session id>.json` =
//! `{"prs":[{"number":N,"provider":"…","branch":"…",…}, …]}`, oldest first.
//!
//! This is the source of a card's PR number, so two sessions sharing a branch
//! do not inherit each other's PR. Only the number, provider and branch are
//! read (never the URL); unknown fields are ignored.

use std::path::{Path, PathBuf};

use serde_json::Value;

use super::context::safe_id;

/// One PR entry of a session's meta.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetaPr {
    pub number: u64,
    pub provider: Option<String>,
    pub branch: Option<String>,
}

pub fn default_session_meta_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_default().join(".claude").join("session-meta")
}

/// The session's PRs, most recent last. A missing or corrupt file is `None`;
/// entries without a usable number are skipped.
pub fn read_prs(dir: &Path, session_id: &str) -> Option<Vec<MetaPr>> {
    if !safe_id(session_id) {
        return None;
    }
    let text = std::fs::read_to_string(dir.join(format!("{session_id}.json"))).ok()?;
    let root: Value = serde_json::from_str(&text).ok()?;
    let root = root.as_object()?;
    let prs = root.get("prs").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
    Some(
        prs.iter()
            .filter_map(|e| {
                let text = |k: &str| e.get(k).and_then(Value::as_str).map(String::from);
                Some(MetaPr { number: parse_number(e.get("number")?)?, provider: text("provider"), branch: text("branch") })
            })
            .collect(),
    )
}

/// A PR number written as a JSON integer or a numeric string.
fn parse_number(v: &Value) -> Option<u64> {
    match v {
        Value::Number(n) => n.as_u64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

/// The entry to show: the latest one on the session's current `branch` when
/// there is one, else the last (most recent) entry.
pub fn pick<'a>(prs: &'a [MetaPr], branch: Option<&str>) -> Option<&'a MetaPr> {
    branch
        .and_then(|b| prs.iter().rev().find(|p| p.branch.as_deref() == Some(b)))
        .or_else(|| prs.last())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, id: &str, body: &str) {
        std::fs::write(dir.join(format!("{id}.json")), body).unwrap();
    }

    fn nums(v: Option<Vec<MetaPr>>) -> Option<Vec<u64>> {
        v.map(|v| v.iter().map(|p| p.number).collect())
    }

    #[test]
    fn read_prs_table() {
        let d = tempfile::tempdir().unwrap();
        let cases: &[(&str, &str, Option<Vec<u64>>)] = &[
            ("int", r#"{"prs":[{"number":7,"url":"https://example.invalid/pr/7"}]}"#, Some(vec![7])),
            ("string", r#"{"prs":[{"number":"8"},{"number":" 9 "}]}"#, Some(vec![8, 9])),
            ("order kept", r#"{"prs":[{"number":3},{"number":1},{"number":2}]}"#, Some(vec![3, 1, 2])),
            ("empty", r#"{"prs":[]}"#, Some(vec![])),
            ("no prs key", r#"{"other":1}"#, Some(vec![])),
            ("bad entries skipped", r#"{"prs":[{"number":"x"},{"number":-1},{"number":1.5},{},5,{"number":6}]}"#, Some(vec![6])),
            ("extra fields", r#"{"x":[1],"prs":[{"number":4,"extra":{"a":1},"created_at":"t"}]}"#, Some(vec![4])),
            ("bad json", "{not json", None),
            ("not an object", "[1,2]", None),
            ("prs not an array", r#"{"prs":"no"}"#, Some(vec![])),
        ];
        for (i, (name, body, want)) in cases.iter().enumerate() {
            let id = format!("s{i}");
            write(d.path(), &id, body);
            assert_eq!(nums(read_prs(d.path(), &id)), *want, "{name}");
        }
        assert_eq!(read_prs(d.path(), "missing"), None, "missing file");
        assert_eq!(read_prs(d.path(), "../s0"), None, "unsafe id");
        assert_eq!(read_prs(&d.path().join("nodir"), "s0"), None, "missing dir");
    }

    #[test]
    fn read_prs_keeps_provider_and_branch() {
        let d = tempfile::tempdir().unwrap();
        write(d.path(), "a", r#"{"prs":[{"number":1,"provider":"ado","branch":"feat/x"},{"number":2}]}"#);
        let v = read_prs(d.path(), "a").unwrap();
        assert_eq!(v[0], MetaPr { number: 1, provider: Some("ado".into()), branch: Some("feat/x".into()) });
        assert_eq!(v[1], MetaPr { number: 2, provider: None, branch: None });
    }

    #[test]
    fn pick_prefers_branch_match_else_last() {
        let pr = |n, b: Option<&str>| MetaPr { number: n, provider: None, branch: b.map(String::from) };
        let v = vec![pr(1, Some("a")), pr(2, Some("b")), pr(3, Some("a")), pr(4, Some("c"))];
        assert_eq!(pick(&v, Some("a")).unwrap().number, 3, "latest on the branch");
        assert_eq!(pick(&v, Some("b")).unwrap().number, 2);
        assert_eq!(pick(&v, Some("zzz")).unwrap().number, 4, "no match: last");
        assert_eq!(pick(&v, None).unwrap().number, 4, "no branch: last");
        assert_eq!(pick(&[], Some("a")), None);
    }
}
