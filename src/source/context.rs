//! Context-window usage of a session, as a percentage (0..=100).
//!
//! Primary source: the statusline script writes
//! `~/.cache/cmux-magigent/ctx/<session_id>.json` (or the former
//! `~/.cache/cmux-cc-monitor/ctx/`; the freshest valid file wins) with
//! `{"session_id": "...", "used_percentage": NN}`; it is used only while the
//! file is fresh (mtime within the configured age). Fallback: the last
//! `message.usage` of the session transcript (only its last [`TAIL_BYTES`]
//! are read), `input + cache_read + cache_creation` tokens over the configured
//! context limit. Anything malformed or out of range yields `None`.
//!
//! The same tail read also yields the session's model (`message.model` of the
//! last main-chain assistant record), so a transcript is read at most once per
//! change ([`TranscriptCache`]).

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde_json::Value;

/// Bytes read from the end of a transcript.
pub const TAIL_BYTES: u64 = 64 * 1024;
/// Larger window read once (per file change) when the tail misses a value
/// that the cache does not already know.
pub const RETRY_BYTES: u64 = 1024 * 1024;
/// Context files larger than this are not ours (or broken): ignored.
pub const MAX_CTX_FILE_BYTES: u64 = 4096;

/// Directories of the statusline context files under `home`: the current one
/// first, then the app's former name (existing statusline scripts keep
/// writing there).
pub fn ctx_dirs_in(home: &Path) -> Vec<PathBuf> {
    let cache = home.join(".cache");
    vec![cache.join("cmux-magigent").join("ctx"), cache.join("cmux-cc-monitor").join("ctx")]
}

/// Default directories of the statusline context files.
pub fn default_ctx_dirs() -> Vec<PathBuf> {
    ctx_dirs_in(&dirs::home_dir().unwrap_or_default())
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
#[cfg(test)]
pub fn read_ctx_file(dir: &Path, session_id: &str, now: SystemTime, max_age: Duration) -> Option<f32> {
    read_ctx_entry(dir, session_id, now, max_age).map(|(_, p)| p)
}

/// [`read_ctx_file`] over several directories: the freshest valid file wins.
pub fn read_ctx_files<P: AsRef<Path>>(dirs: &[P], session_id: &str, now: SystemTime, max_age: Duration) -> Option<f32> {
    dirs.iter()
        .filter_map(|d| read_ctx_entry(d.as_ref(), session_id, now, max_age))
        .max_by_key(|(m, _)| *m)
        .map(|(_, p)| p)
}

fn read_ctx_entry(dir: &Path, session_id: &str, now: SystemTime, max_age: Duration) -> Option<(SystemTime, f32)> {
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
    Some((mtime, percent(v.get("used_percentage")?.as_f64()?)?))
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

/// What the tail of a transcript tells: the context tokens of the last
/// main-chain usage record and the model of the last main-chain assistant
/// reply. Each is searched on its own, so they may come from different records.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TranscriptTail {
    pub tokens: Option<u64>,
    pub model: Option<String>,
}

impl TranscriptTail {
    /// Usage in percent of `limit` tokens (clamped to 100); `None` without a
    /// usage record or with a zero limit.
    pub fn usage_percent(&self, limit: u64) -> Option<f32> {
        if limit == 0 {
            return None;
        }
        percent(self.tokens? as f64 / limit as f64 * 100.0)
    }
}

/// A real model id: a non-empty string that is not a placeholder like
/// `<synthetic>` (written for locally generated replies).
fn real_model(v: &Value) -> Option<&str> {
    let m = v.as_str()?.trim();
    (!m.is_empty() && !m.starts_with('<')).then_some(m)
}

/// Reads the last [`TAIL_BYTES`] of `path` once and extracts the last
/// main-chain usage and model. A first line cut by the window, unparsable
/// lines, sidechain and non-assistant records, malformed usage and
/// placeholder models are skipped. `None` when the file cannot be read.
pub fn read_transcript_tail(path: &Path) -> Option<TranscriptTail> {
    read_transcript_window(path, TAIL_BYTES)
}

/// [`read_transcript_tail`] over the last `window` bytes.
fn read_transcript_window(path: &Path, window: u64) -> Option<TranscriptTail> {
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let start = len.saturating_sub(window);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut bytes = Vec::new();
    file.take(window).read_to_end(&mut bytes).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    let mut body: &str = &text;
    if start > 0 {
        // The first line is probably cut in the middle: drop it.
        body = body.split_once('\n').map_or("", |(_, rest)| rest);
    }
    let mut tail = TranscriptTail::default();
    for line in body.lines().rev() {
        if tail.tokens.is_some() && tail.model.is_some() {
            break;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if v.get("isSidechain").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        if v.get("type").and_then(Value::as_str).is_some_and(|t| t != "assistant") {
            continue;
        }
        if tail.tokens.is_none() {
            // A malformed usage falls back to the next older record.
            tail.tokens = v.pointer("/message/usage").and_then(usage_tokens);
        }
        if tail.model.is_none() {
            tail.model = v.pointer("/message/model").and_then(real_model).map(String::from);
        }
    }
    Some(tail)
}

/// Usage percentage from the last main-chain `message.usage` in the last
/// [`TAIL_BYTES`] of `path`, over `limit` tokens (see [`read_transcript_tail`]).
#[cfg(test)]
pub fn read_transcript_usage(path: &Path, limit: u64) -> Option<f32> {
    read_transcript_tail(path)?.usage_percent(limit)
}

/// Transcript tails keyed by path, reused while the file's (mtime, length)
/// is unchanged. Carried from one refresh round to the next.
///
/// A big non-assistant record (e.g. a huge tool_result) can push the last
/// assistant record out of the tail window. A value the tail misses is then
/// kept from the path's previous entry (the last known model / usage); a
/// value still unknown in a file longer than the tail is retried once (per
/// file change) with [`RETRY_BYTES`] before it stays `None`.
#[derive(Debug, Clone, Default)]
pub struct TranscriptCache {
    entries: HashMap<PathBuf, (SystemTime, u64, TranscriptTail)>,
    /// Tail reads done (cache misses), for tests.
    #[cfg(test)]
    reads: usize,
}

impl TranscriptCache {
    /// [`read_transcript_tail`], cached. `None` when the file is missing or unreadable.
    pub fn tail(&mut self, path: &Path) -> Option<TranscriptTail> {
        let meta = std::fs::metadata(path).ok()?;
        let key = (meta.modified().ok()?, meta.len());
        if let Some((m, l, tail)) = self.entries.get(path) {
            if (*m, *l) == key {
                return Some(tail.clone());
            }
        }
        #[cfg(test)]
        {
            self.reads += 1;
        }
        let mut tail = read_transcript_tail(path)?;
        if let Some((_, _, known)) = self.entries.get(path) {
            tail.tokens = tail.tokens.or(known.tokens);
            tail.model = tail.model.take().or_else(|| known.model.clone());
        }
        if (tail.tokens.is_none() || tail.model.is_none()) && key.1 > TAIL_BYTES {
            if let Some(wide) = read_transcript_window(path, RETRY_BYTES) {
                tail.tokens = tail.tokens.or(wide.tokens);
                tail.model = tail.model.take().or(wide.model);
            }
        }
        self.entries.insert(path.to_path_buf(), (key.0, key.1, tail.clone()));
        Some(tail)
    }

    /// Usage percentage of `path` over `limit` tokens, through [`TranscriptCache::tail`].
    #[cfg(test)]
    pub fn usage(&mut self, path: &Path, limit: u64) -> Option<f32> {
        self.tail(path)?.usage_percent(limit)
    }

    /// Keeps only the entries of `paths`.
    pub fn retain(&mut self, paths: &[PathBuf]) {
        self.entries.retain(|p, _| paths.contains(p));
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[cfg(test)]
    pub fn reads(&self) -> usize {
        self.reads
    }
}

/// Context usage of one session: the fresh statusline file, else the
/// transcript tail's usage, else `None`.
pub fn context_percent(
    ctx_dirs: &[PathBuf],
    session_id: &str,
    tail: Option<&TranscriptTail>,
    limit: u64,
    max_age: Duration,
    now: SystemTime,
) -> Option<f32> {
    read_ctx_files(ctx_dirs, session_id, now, max_age).or_else(|| tail?.usage_percent(limit))
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
    fn ctx_dirs_are_cmux_magigent_first_then_legacy() {
        let home = Path::new("/h");
        assert_eq!(
            ctx_dirs_in(home),
            vec![PathBuf::from("/h/.cache/cmux-magigent/ctx"), PathBuf::from("/h/.cache/cmux-cc-monitor/ctx")]
        );
    }

    #[test]
    fn legacy_ctx_dir_is_read_when_new_has_no_file() {
        let home = tempfile::tempdir().unwrap();
        let dirs = ctx_dirs_in(home.path());
        std::fs::create_dir_all(&dirs[1]).unwrap();
        let p = write_ctx(&dirs[1], "s", r#"{"session_id":"s","used_percentage":33}"#);
        assert_eq!(read_ctx_files(&dirs, "s", mtime(&p), FRESH), Some(33.0));
        assert_eq!(read_ctx_files(&dirs, "other", mtime(&p), FRESH), None);
    }

    #[test]
    fn freshest_valid_ctx_file_wins_across_dirs() {
        let home = tempfile::tempdir().unwrap();
        let dirs = ctx_dirs_in(home.path());
        for d in &dirs {
            std::fs::create_dir_all(d).unwrap();
        }
        let old = write_ctx(&dirs[0], "s", r#"{"used_percentage":10}"#);
        let f = std::fs::File::options().write(true).open(&old).unwrap();
        f.set_modified(SystemTime::now() - Duration::from_secs(100)).unwrap();
        let newer = write_ctx(&dirs[1], "s", r#"{"used_percentage":20}"#);
        let now = mtime(&newer);
        assert_eq!(read_ctx_files(&dirs, "s", now, FRESH), Some(20.0), "legacy is fresher");
        // an invalid fresher file does not shadow a valid older one
        write_ctx(&dirs[1], "s", "{{{");
        assert_eq!(read_ctx_files(&dirs, "s", SystemTime::now(), FRESH), Some(10.0));
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
    fn transcript_malformed_newest_usage_falls_back_to_older() {
        let dir = tempfile::tempdir().unwrap();
        let t = dir.path().join("t.jsonl");
        write_lines(&t, &[assistant(40_000, 0, 0), r#"{"type":"assistant","message":{"usage":{"input_tokens":"oops"}}}"#.into()]);
        assert_eq!(read_transcript_usage(&t, 200_000), Some(20.0));
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
        let tail = cache.tail(&t);
        assert_eq!(context_percent(&[dir.path().to_path_buf()], "s", tail.as_ref(), 200_000, FRESH, m), Some(80.0));
        let stale = m + FRESH + Duration::from_secs(60);
        assert_eq!(context_percent(&[dir.path().to_path_buf()], "s", tail.as_ref(), 200_000, FRESH, stale), Some(25.0));
        assert_eq!(context_percent(&[dir.path().to_path_buf()], "s", None, 200_000, FRESH, stale), None);
        let none = cache.tail(&dir.path().join("none"));
        assert_eq!(none, None, "missing transcript");
        assert_eq!(context_percent(&[dir.path().to_path_buf()], "zz", none.as_ref(), 200_000, FRESH, m), None);
    }

    fn assistant_model(model: &str) -> String {
        format!(
            r#"{{"type":"assistant","isSidechain":false,"message":{{"role":"assistant","model":"{model}","usage":{{"input_tokens":20000}}}}}}"#
        )
    }

    fn model_of(path: &Path) -> Option<String> {
        read_transcript_tail(path).and_then(|t| t.model)
    }

    #[test]
    fn transcript_model_is_the_last_main_chain_assistant_model() {
        let dir = tempfile::tempdir().unwrap();
        let t = dir.path().join("t.jsonl");
        write_lines(
            &t,
            &[
                assistant_model("claude-haiku-5-5"),
                assistant_model("claude-opus-4-5"),
                r#"{"type":"user","message":{"role":"user","content":"hi"}}"#.into(),
            ],
        );
        let tail = read_transcript_tail(&t).unwrap();
        assert_eq!(tail.model.as_deref(), Some("claude-opus-4-5"));
        assert_eq!(tail.tokens, Some(20_000), "usage and model come from the same read");
        assert_eq!(tail.usage_percent(200_000), Some(10.0));
        assert_eq!(tail.usage_percent(0), None);
    }

    #[test]
    fn transcript_model_none_without_assistant_record() {
        let dir = tempfile::tempdir().unwrap();
        let t = dir.path().join("t.jsonl");
        write_lines(&t, &[r#"{"type":"user","message":{"model":"claude-opus-4-5","content":"x"}}"#.into(), "garbage".into()]);
        assert_eq!(model_of(&t), None);
        // usage without a model: tokens only
        write_lines(&t, &[assistant(1000, 0, 0)]);
        assert_eq!(read_transcript_tail(&t), Some(TranscriptTail { tokens: Some(1000), model: None }));
        assert_eq!(read_transcript_tail(&dir.path().join("missing.jsonl")), None);
    }

    #[test]
    fn transcript_model_skips_sidechain_synthetic_and_broken_lines() {
        let dir = tempfile::tempdir().unwrap();
        let t = dir.path().join("t.jsonl");
        write_lines(
            &t,
            &[
                assistant_model("claude-sonnet-5-5"),
                r#"{"type":"assistant","isSidechain":true,"message":{"model":"claude-haiku-5-5"}}"#.into(),
                r#"{"type":"assistant","message":{"model":"<synthetic>"}}"#.into(),
                r#"{"type":"assistant","message":{"model":""}}"#.into(),
                r#"{"type":"assistant","message":{"model":42}}"#.into(),
                r#"{"type":"assistant","message":{"model":"claude-fable-5-1","usage":{"input_tok"#.into(), // cut
            ],
        );
        assert_eq!(model_of(&t), Some("claude-sonnet-5-5".into()));
    }

    #[test]
    fn transcript_model_line_cut_by_the_window_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let t = dir.path().join("t.jsonl");
        // a model line straddling the 64 KB window start is cut mid-line and
        // skipped; the earlier one inside the window is used
        let big = format!(r#"{{"type":"assistant","message":{{"model":"claude-haiku-5-5","content":"{}"}}}}"#, "y".repeat(300));
        let mid = assistant_model("claude-opus-4-5");
        let (pre, post) = (r#"{"type":"user","message":{"content":""#, r#""}}"#);
        let fill = TAIL_BYTES as usize - 50 - (mid.len() + 1) - (pre.len() + post.len() + 1);
        let last = format!("{pre}{}{post}", "z".repeat(fill));
        write_lines(&t, &[big.clone(), mid, last]);
        let len = std::fs::metadata(&t).unwrap().len();
        assert_eq!(len - TAIL_BYTES, big.len() as u64 + 1 - 50, "test setup: cut inside the big line");
        assert_eq!(model_of(&t), Some("claude-opus-4-5".into()));
        // only a cut model line in reach: no model
        let mut lines = vec![big];
        let filler = format!(r#"{{"type":"user","message":{{"content":"{}"}}}}"#, "x".repeat(1000));
        lines.extend(std::iter::repeat_n(filler.clone(), (TAIL_BYTES as usize / filler.len()) + 2));
        write_lines(&t, &lines);
        assert_eq!(model_of(&t), None, "head is not read");
    }

    #[test]
    fn transcript_cache_reads_each_unchanged_file_once() {
        let dir = tempfile::tempdir().unwrap();
        let t = dir.path().join("t.jsonl");
        write_lines(&t, &[assistant_model("claude-opus-4-5")]);
        let mut cache = TranscriptCache::default();
        let first = cache.tail(&t).unwrap();
        assert_eq!(first.model.as_deref(), Some("claude-opus-4-5"));
        assert_eq!(cache.usage(&t, 200_000), Some(10.0));
        assert_eq!(cache.tail(&t), Some(first));
        assert_eq!(cache.reads(), 1, "model and usage share one read");
        let mut f = std::fs::OpenOptions::new().append(true).open(&t).unwrap();
        writeln!(f, "{}", assistant_model("claude-haiku-5-5")).unwrap();
        drop(f);
        assert_eq!(cache.tail(&t).unwrap().model.as_deref(), Some("claude-haiku-5-5"));
        assert_eq!(cache.reads(), 2, "a changed file is read again");
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

    /// A non-assistant line of about `bytes` bytes (e.g. a huge tool_result).
    fn big_user(bytes: usize) -> String {
        format!(r#"{{"type":"user","message":{{"content":"{}"}}}}"#, "x".repeat(bytes))
    }

    fn append(path: &Path, line: &str) {
        let mut f = std::fs::OpenOptions::new().append(true).open(path).unwrap();
        writeln!(f, "{line}").unwrap();
    }

    #[test]
    fn transcript_cache_keeps_model_and_tokens_past_a_big_user_record() {
        let dir = tempfile::tempdir().unwrap();
        let t = dir.path().join("t.jsonl");
        // 20_000 tokens = 10 % of 200k
        write_lines(&t, &[assistant_model("claude-fable-5-1")]);
        let mut cache = TranscriptCache::default();
        assert_eq!(cache.tail(&t).unwrap().model.as_deref(), Some("claude-fable-5-1"));
        // a > 64 KiB user record pushes the assistant out of the tail window
        append(&t, &big_user(TAIL_BYTES as usize + 1000));
        assert_eq!(read_transcript_tail(&t).unwrap().model, None, "test setup: out of the 64 KiB window");
        let tail = cache.tail(&t).unwrap();
        assert_eq!(tail.model.as_deref(), Some("claude-fable-5-1"));
        assert_eq!(tail.usage_percent(200_000), Some(10.0));
        // even past the larger retry window, the last known values stay
        append(&t, &big_user(RETRY_BYTES as usize + 1000));
        let tail = cache.tail(&t).unwrap();
        assert_eq!(tail.model.as_deref(), Some("claude-fable-5-1"));
        assert_eq!(tail.tokens, Some(20_000));
    }

    #[test]
    fn transcript_cache_retries_a_miss_with_a_larger_window() {
        let dir = tempfile::tempdir().unwrap();
        let t = dir.path().join("t.jsonl");
        // never seen before: the 64 KiB tail misses, the 1 MiB retry finds it
        write_lines(&t, &[assistant_model("claude-sonnet-5-5"), big_user(TAIL_BYTES as usize + 1000)]);
        let mut cache = TranscriptCache::default();
        let tail = cache.tail(&t).unwrap();
        assert_eq!(tail.model.as_deref(), Some("claude-sonnet-5-5"));
        assert_eq!(tail.tokens, Some(20_000));
        assert_eq!(cache.tail(&t), Some(tail), "cached by (mtime, len)");
        assert_eq!(cache.reads(), 1);
    }

    #[test]
    fn transcript_cache_new_session_without_assistant_is_none() {
        let dir = tempfile::tempdir().unwrap();
        let t = dir.path().join("t.jsonl");
        write_lines(&t, &[r#"{"type":"user","message":{"content":"hi"}}"#.into()]);
        let mut cache = TranscriptCache::default();
        assert_eq!(cache.tail(&t), Some(TranscriptTail::default()));
        // a fresh unseen file whose assistant is beyond every window: None
        let u = dir.path().join("u.jsonl");
        write_lines(&u, &[assistant_model("claude-opus-4-5"), big_user(RETRY_BYTES as usize + 1000)]);
        assert_eq!(cache.tail(&u), Some(TranscriptTail::default()));
    }

    #[test]
    fn transcript_cache_pruned_entries_lose_their_last_known_values() {
        let dir = tempfile::tempdir().unwrap();
        let t = dir.path().join("t.jsonl");
        write_lines(&t, &[assistant_model("claude-opus-4-5")]);
        let mut cache = TranscriptCache::default();
        assert!(cache.tail(&t).unwrap().model.is_some());
        cache.retain(&[]);
        assert_eq!(cache.len(), 0);
        append(&t, &big_user(RETRY_BYTES as usize + 1000));
        assert_eq!(cache.tail(&t), Some(TranscriptTail::default()), "pruned: nothing carried over");
    }

    #[test]
    fn transcript_cache_updates_a_genuinely_changed_model() {
        let dir = tempfile::tempdir().unwrap();
        let t = dir.path().join("t.jsonl");
        write_lines(&t, &[assistant_model("claude-opus-4-5")]);
        let mut cache = TranscriptCache::default();
        assert_eq!(cache.tail(&t).unwrap().model.as_deref(), Some("claude-opus-4-5"));
        append(&t, &big_user(RETRY_BYTES as usize + 1000));
        assert_eq!(cache.tail(&t).unwrap().model.as_deref(), Some("claude-opus-4-5"));
        append(&t, &assistant(150_000, 0, 0));
        append(&t, &assistant_model("claude-haiku-5-5"));
        let tail = cache.tail(&t).unwrap();
        assert_eq!(tail.model.as_deref(), Some("claude-haiku-5-5"));
        assert_eq!(tail.tokens, Some(20_000));
    }
}
