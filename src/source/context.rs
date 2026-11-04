//! Context-window usage of a session, as a percentage (0..=100).
//!
//! Primary source: the statusline script writes
//! `~/.cache/cmux-cc-monitor/ctx/<session_id>.json` with
//! `{"session_id": "...", "used_percentage": NN}`; it is used only while the
//! file is fresh (mtime within the configured age). Fallback: the last
//! `message.usage` of the session transcript (only its last [`TAIL_BYTES`]
//! are read), `input + cache_read + cache_creation` tokens over the configured
//! context limit. Anything malformed or out of range yields `None`.

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde_json::Value;

/// Bytes read from the end of a transcript.
pub const TAIL_BYTES: u64 = 64 * 1024;
/// Context files larger than this are not ours (or broken): ignored.
pub const MAX_CTX_FILE_BYTES: u64 = 4096;

/// Default directory of the statusline context files.
pub fn default_ctx_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_default().join(".cache").join("cmux-cc-monitor").join("ctx")
}

/// A session id that is safe to use as a file name.
pub(crate) fn safe_id(id: &str) -> bool {
    !id.is_empty() && id != "." && id != ".." && !id.contains(['/', '\\', '\0'])
}

/// A finite, non-negative percentage clamped to 100; anything else is `None`.
fn percent(v: f64) -> Option<f32> {
    (v.is_finite() && v >= 0.0).then(|| v.min(100.0) as f32)
}

/// `used_percentage` from `<dir>/<session_id>.json`, when the file is at most
/// `max_age` old at `now` (an mtime in the future counts as fresh). A
/// `session_id` in the file that names another session rejects it.
pub fn read_ctx_file(dir: &Path, session_id: &str, now: SystemTime, max_age: Duration) -> Option<f32> {
    if !safe_id(session_id) {
        return None;
    }
    let path = dir.join(format!("{session_id}.json"));
    let meta = std::fs::metadata(&path).ok()?;
    if !meta.is_file() || meta.len() > MAX_CTX_FILE_BYTES {
        return None;
    }
    let mtime = meta.modified().ok()?;
    if now.duration_since(mtime).is_ok_and(|age| age > max_age) {
        return None;
    }
    let text = std::fs::read_to_string(&path).ok()?;
    let v: Value = serde_json::from_str(&text).ok()?;
    if let Some(id) = v.get("session_id") {
        if id.as_str() != Some(session_id) {
            return None;
        }
    }
    percent(v.get("used_percentage")?.as_f64()?)
}

/// Context tokens of a usage record; `None` when a counted field is not a
/// non-negative integer or none is present.
fn usage_tokens(usage: &Value) -> Option<u64> {
    let mut total = 0u64;
    let mut any = false;
    for key in ["input_tokens", "cache_read_input_tokens", "cache_creation_input_tokens"] {
        match usage.get(key) {
            None | Some(Value::Null) => {}
            Some(n) => {
                total = total.saturating_add(n.as_u64()?);
                any = true;
            }
        }
    }
    any.then_some(total)
}

/// Usage percentage from the last main-chain `message.usage` in the last
/// [`TAIL_BYTES`] of `path`, over `limit` tokens. A first line cut by the
/// window and unparsable lines are skipped.
pub fn read_transcript_usage(path: &Path, limit: u64) -> Option<f32> {
    if limit == 0 {
        return None;
    }
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let start = len.saturating_sub(TAIL_BYTES);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut bytes = Vec::new();
    file.take(TAIL_BYTES).read_to_end(&mut bytes).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    let mut body: &str = &text;
    if start > 0 {
        // The first line is probably cut in the middle: drop it.
        body = body.split_once('\n').map_or("", |(_, rest)| rest);
    }
    for line in body.lines().rev() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if v.get("isSidechain").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        if v.get("type").and_then(Value::as_str).is_some_and(|t| t != "assistant") {
            continue;
        }
        let Some(usage) = v.pointer("/message/usage") else {
            continue;
        };
        let tokens = usage_tokens(usage)?;
        return percent(tokens as f64 / limit as f64 * 100.0);
    }
    None
}

/// Transcript results keyed by path, reused while the file's (mtime, length)
/// and the limit are unchanged. Carried from one refresh round to the next.
#[derive(Debug, Clone, Default)]
pub struct TranscriptCache {
    entries: HashMap<PathBuf, (SystemTime, u64, u64, Option<f32>)>,
}

impl TranscriptCache {
    /// [`read_transcript_usage`], cached.
    pub fn usage(&mut self, path: &Path, limit: u64) -> Option<f32> {
        let meta = std::fs::metadata(path).ok()?;
        let key = (meta.modified().ok()?, meta.len(), limit);
        if let Some(&(m, l, lim, v)) = self.entries.get(path) {
            if (m, l, lim) == key {
                return v;
            }
        }
        let v = read_transcript_usage(path, limit);
        self.entries.insert(path.to_path_buf(), (key.0, key.1, key.2, v));
        v
    }

    /// Keeps only the entries of `paths`.
    pub fn retain(&mut self, paths: &[PathBuf]) {
        self.entries.retain(|p, _| paths.contains(p));
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

/// Context usage of one session: the fresh statusline file, else the
/// transcript (through `cache`), else `None`.
pub fn context_percent(
    ctx_dir: &Path,
    session_id: &str,
    transcript: Option<&Path>,
    limit: u64,
    max_age: Duration,
    now: SystemTime,
    cache: &mut TranscriptCache,
) -> Option<f32> {
    read_ctx_file(ctx_dir, session_id, now, max_age).or_else(|| cache.usage(transcript?, limit))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::time::{Duration, SystemTime};

    const FRESH: Duration = Duration::from_secs(300);

    fn write_ctx(dir: &Path, id: &str, body: &str) -> PathBuf {
        let p = dir.join(format!("{id}.json"));
        std::fs::write(&p, body).unwrap();
        p
    }

    fn mtime(p: &Path) -> SystemTime {
        std::fs::metadata(p).unwrap().modified().unwrap()
    }

    #[test]
    fn ctx_file_valid_is_read() {
        let dir = tempfile::tempdir().unwrap();
        let p = write_ctx(dir.path(), "sess-1", r#"{"session_id":"sess-1","used_percentage":42}"#);
        assert_eq!(read_ctx_file(dir.path(), "sess-1", mtime(&p), FRESH), Some(42.0));
        let p = write_ctx(dir.path(), "sess-2", r#"{"session_id":"sess-2","used_percentage":12.5}"#);
        assert_eq!(read_ctx_file(dir.path(), "sess-2", mtime(&p) + Duration::from_secs(10), FRESH), Some(12.5));
    }

    #[test]
    fn ctx_file_missing_or_malformed_is_none() {
        let dir = tempfile::tempdir().unwrap();
        let now = SystemTime::now();
        assert_eq!(read_ctx_file(dir.path(), "nope", now, FRESH), None, "missing");
        assert_eq!(read_ctx_file(&dir.path().join("no-dir"), "nope", now, FRESH), None, "missing dir");
        for (i, body) in ["{{{", "", "[]", "null", r#"{"session_id":"x"}"#, r#"{"used_percentage":"42"}"#, r#"{"used_percentage":null}"#]
            .iter()
            .enumerate()
        {
            let id = format!("bad-{i}");
            let p = write_ctx(dir.path(), &id, body);
            assert_eq!(read_ctx_file(dir.path(), &id, mtime(&p), FRESH), None, "{body}");
        }
    }

    #[test]
    fn ctx_file_out_of_range_values() {
        let dir = tempfile::tempdir().unwrap();
        let p = write_ctx(dir.path(), "big", r#"{"used_percentage":150}"#);
        assert_eq!(read_ctx_file(dir.path(), "big", mtime(&p), FRESH), Some(100.0), "clamped");
        let p = write_ctx(dir.path(), "neg", r#"{"used_percentage":-3}"#);
        assert_eq!(read_ctx_file(dir.path(), "neg", mtime(&p), FRESH), None, "negative");
        let p = write_ctx(dir.path(), "huge", r#"{"used_percentage":1e300}"#);
        assert_eq!(read_ctx_file(dir.path(), "huge", mtime(&p), FRESH), Some(100.0), "huge finite clamps");
        let p = write_ctx(dir.path(), "inf", r#"{"used_percentage":1e999}"#);
        assert_eq!(read_ctx_file(dir.path(), "inf", mtime(&p), FRESH), None, "overflowing number");
        let p = write_ctx(dir.path(), "other", r#"{"session_id":"someone-else","used_percentage":50}"#);
        assert_eq!(read_ctx_file(dir.path(), "other", mtime(&p), FRESH), None, "session id mismatch");
    }

    #[test]
    fn ctx_file_freshness_uses_mtime() {
        let dir = tempfile::tempdir().unwrap();
        let p = write_ctx(dir.path(), "s", r#"{"used_percentage":55}"#);
        let m = mtime(&p);
        assert_eq!(read_ctx_file(dir.path(), "s", m + FRESH, FRESH), Some(55.0), "exactly at the limit");
        assert_eq!(read_ctx_file(dir.path(), "s", m + FRESH + Duration::from_secs(1), FRESH), None, "stale");
        assert_eq!(read_ctx_file(dir.path(), "s", m - Duration::from_secs(5), FRESH), Some(55.0), "clock skew: future mtime is fresh");
    }

    #[test]
    fn ctx_file_rejects_unsafe_ids_and_huge_files() {
        let dir = tempfile::tempdir().unwrap();
        let now = SystemTime::now();
        for id in ["", "../x", "a/b", ".."] {
            assert_eq!(read_ctx_file(dir.path(), id, now, FRESH), None, "{id:?}");
        }
        let pad = " ".repeat(MAX_CTX_FILE_BYTES as usize + 10);
        let p = write_ctx(dir.path(), "fat", &format!(r#"{{"used_percentage":10}}{pad}"#));
        assert_eq!(read_ctx_file(dir.path(), "fat", mtime(&p), FRESH), None);
    }

    fn assistant(input: u64, read: u64, create: u64) -> String {
        format!(
            r#"{{"type":"assistant","isSidechain":false,"message":{{"role":"assistant","usage":{{"input_tokens":{input},"cache_read_input_tokens":{read},"cache_creation_input_tokens":{create},"output_tokens":7}}}}}}"#
        )
    }

    fn write_lines(path: &Path, lines: &[String]) {
        let mut f = std::fs::File::create(path).unwrap();
        for l in lines {
            writeln!(f, "{l}").unwrap();
        }
    }

    #[test]
    fn transcript_usage_sums_last_usage_over_limit() {
        let dir = tempfile::tempdir().unwrap();
        let t = dir.path().join("t.jsonl");
        write_lines(
            &t,
            &[
                assistant(1, 2, 3),
                r#"{"type":"user","message":{"role":"user","content":"hi"}}"#.into(),
                assistant(1000, 40_000, 9_000),
                r#"{"type":"user","message":{"role":"user","content":"next"}}"#.into(),
            ],
        );
        assert_eq!(read_transcript_usage(&t, 200_000), Some(25.0));
        assert_eq!(read_transcript_usage(&t, 100_000), Some(50.0));
        assert_eq!(read_transcript_usage(&t, 10_000), Some(100.0), "clamped");
        assert_eq!(read_transcript_usage(&t, 0), None, "zero limit");
    }

    #[test]
    fn transcript_usage_skips_sidechain_and_cut_last_line() {
        let dir = tempfile::tempdir().unwrap();
        let t = dir.path().join("t.jsonl");
        let side = r#"{"type":"assistant","isSidechain":true,"message":{"usage":{"input_tokens":190000}}}"#;
        let mut lines = vec![assistant(20_000, 0, 0), side.to_string()];
        lines.push(r#"{"type":"assistant","message":{"usage":{"input_tokens":150"#.into()); // cut
        write_lines(&t, &lines);
        assert_eq!(read_transcript_usage(&t, 200_000), Some(10.0));
    }

    #[test]
    fn transcript_without_usage_or_missing_is_none() {
        let dir = tempfile::tempdir().unwrap();
        let t = dir.path().join("t.jsonl");
        write_lines(&t, &[r#"{"type":"user","message":{"content":"x"}}"#.into(), "garbage".into()]);
        assert_eq!(read_transcript_usage(&t, 200_000), None);
        assert_eq!(read_transcript_usage(&dir.path().join("missing.jsonl"), 200_000), None);
        let bad = dir.path().join("bad.jsonl");
        write_lines(&bad, &[r#"{"type":"assistant","message":{"usage":{"input_tokens":-5}}}"#.into()]);
        assert_eq!(read_transcript_usage(&bad, 200_000), None, "negative tokens");
        let s = dir.path().join("str.jsonl");
        write_lines(&s, &[r#"{"type":"assistant","message":{"usage":{"input_tokens":"5"}}}"#.into()]);
        assert_eq!(read_transcript_usage(&s, 200_000), None, "string tokens");
    }

    #[test]
    fn transcript_reads_only_the_tail() {
        let dir = tempfile::tempdir().unwrap();
        let t = dir.path().join("t.jsonl");
        // usage only at the start, then > 64 KB of filler: out of reach
        let filler = format!(r#"{{"type":"user","message":{{"content":"{}"}}}}"#, "x".repeat(1000));
        let mut lines = vec![assistant(100_000, 0, 0)];
        lines.extend(std::iter::repeat_n(filler.clone(), (TAIL_BYTES as usize / filler.len()) + 2));
        write_lines(&t, &lines);
        assert!(std::fs::metadata(&t).unwrap().len() > TAIL_BYTES);
        assert_eq!(read_transcript_usage(&t, 200_000), None, "head is not read");
        // a usage line that straddles the 64 KB boundary is cut mid-line and skipped;
        // the earlier one inside the window is used
        let big = format!(
            r#"{{"type":"assistant","message":{{"usage":{{"input_tokens":180000}},"content":"{}"}}}}"#,
            "y".repeat(300)
        );
        let mid = assistant(30_000, 0, 0);
        let (pre, post) = (r#"{"type":"user","message":{"content":""#, r#""}}"#);
        // window start lands 50 bytes before the end of `big`
        let fill = TAIL_BYTES as usize - 50 - (mid.len() + 1) - (pre.len() + post.len() + 1);
        let last = format!("{pre}{}{post}", "z".repeat(fill));
        let lines = vec![big.clone(), mid.clone(), last];
        write_lines(&t, &lines);
        let len = std::fs::metadata(&t).unwrap().len();
        assert_eq!(len - TAIL_BYTES, big.len() as u64 + 1 - 50, "test setup: cut inside the big line");
        assert_eq!(read_transcript_usage(&t, 200_000), Some(15.0));
    }

    #[test]
    fn context_percent_prefers_fresh_file_then_transcript() {
        let dir = tempfile::tempdir().unwrap();
        let t = dir.path().join("t.jsonl");
        write_lines(&t, &[assistant(50_000, 0, 0)]);
        let p = write_ctx(dir.path(), "s", r#"{"used_percentage":80}"#);
        let m = mtime(&p);
        let mut cache = TranscriptCache::default();
        assert_eq!(context_percent(dir.path(), "s", Some(&t), 200_000, FRESH, m, &mut cache), Some(80.0));
        let stale = m + FRESH + Duration::from_secs(60);
        assert_eq!(context_percent(dir.path(), "s", Some(&t), 200_000, FRESH, stale, &mut cache), Some(25.0));
        assert_eq!(context_percent(dir.path(), "s", None, 200_000, FRESH, stale, &mut cache), None);
        assert_eq!(context_percent(dir.path(), "zz", Some(&dir.path().join("none")), 200_000, FRESH, m, &mut cache), None);
    }

    #[test]
    fn transcript_cache_reuses_unchanged_files() {
        let dir = tempfile::tempdir().unwrap();
        let t = dir.path().join("t.jsonl");
        write_lines(&t, &[assistant(50_000, 0, 0)]);
        let mut cache = TranscriptCache::default();
        assert_eq!(cache.usage(&t, 200_000), Some(25.0));
        assert_eq!(cache.len(), 1);
        // grow the file: the new value is read
        let mut f = std::fs::OpenOptions::new().append(true).open(&t).unwrap();
        writeln!(f, "{}", assistant(100_000, 0, 0)).unwrap();
        drop(f);
        assert_eq!(cache.usage(&t, 200_000), Some(50.0));
        assert_eq!(cache.usage(&t, 100_000), Some(100.0), "limit change is applied");
        cache.retain(&[]);
        assert_eq!(cache.len(), 0);
    }
}
