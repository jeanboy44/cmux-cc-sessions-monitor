//! Context-window usage of a session, as a percentage (0..=100).
//!
//! Primary source: the statusline script writes
//! `~/.cache/cmux-magigent/ctx/<session_id>.json` (or the former
//! `~/.cache/cmux-cc-monitor/ctx/`; the freshest valid file wins) with
//! `{"session_id": "...", "used_percentage": NN}`. That value is relative to
//! the session's real window, so it stays authoritative after the file goes
//! stale (see [`resolve_ctx`]). Without any file: the last `message.usage` of
//! the session transcript (only its last [`TAIL_BYTES`] are read),
//! `input + cache_read + cache_creation` tokens over the configured context
//! limit or the inferred window ([`window_for`]). Anything malformed or out of
//! range yields `None`.
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

/// Whether a file written at `mtime` is at most `max_age` old at `now` (an
/// mtime in the future counts as fresh).
fn is_fresh(mtime: SystemTime, now: SystemTime, max_age: Duration) -> bool {
    now.duration_since(mtime).map_or(true, |age| age <= max_age)
}

/// `used_percentage` from `<dir>/<session_id>.json`, when the file is at most
/// `max_age` old at `now` (an mtime in the future counts as fresh). A
/// `session_id` in the file that names another session rejects it.
#[cfg(test)]
pub fn read_ctx_file(dir: &Path, session_id: &str, now: SystemTime, max_age: Duration) -> Option<f32> {
    read_ctx_entry(dir, session_id).filter(|(m, _)| is_fresh(*m, now, max_age)).map(|(_, p)| p)
}

/// [`read_ctx_file`] over several directories: the freshest valid file wins.
#[cfg(test)]
pub fn read_ctx_files<P: AsRef<Path>>(dirs: &[P], session_id: &str, now: SystemTime, max_age: Duration) -> Option<f32> {
    read_ctx_latest(dirs, session_id).filter(|(m, _)| is_fresh(*m, now, max_age)).map(|(_, p)| p)
}

/// The newest valid context file of the session over `dirs`, whatever its
/// age: (mtime, `used_percentage`). Freshness is decided by [`resolve_ctx`].
pub fn read_ctx_latest<P: AsRef<Path>>(dirs: &[P], session_id: &str) -> Option<(SystemTime, f32)> {
    dirs.iter().filter_map(|d| read_ctx_entry(d.as_ref(), session_id)).max_by_key(|(m, _)| *m)
}

fn read_ctx_entry(dir: &Path, session_id: &str) -> Option<(SystemTime, f32)> {
    if !safe_id(session_id) {
        return None;
    }
    let path = dir.join(format!("{session_id}.json"));
    let meta = std::fs::metadata(&path).ok()?;
    if !meta.is_file() || meta.len() > MAX_CTX_FILE_BYTES {
        return None;
    }
    let mtime = meta.modified().ok()?;
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

/// The state of the main-chain turn at the end of a transcript.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Turn {
    /// A prompt, a tool call/result or a model request is the newest record.
    Active,
    /// The turn ended (assistant `end_turn`, `stop_hook_summary` /
    /// `turn_duration`, an interrupt) or is parked on background agents
    /// (an `async_launched` tool result), at this time in epoch ms if known.
    Finished(Option<u64>),
}

/// What the tail of a transcript tells: the context tokens of the last
/// main-chain usage record, the model of the last main-chain assistant
/// reply and the state of the last turn. Each is searched on its own, so they
/// may come from different records.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TranscriptTail {
    pub tokens: Option<u64>,
    pub model: Option<String>,
    pub turn: Option<Turn>,
}

/// The turn state a main-chain record shows, or `None` when it tells nothing
/// (sidechain, meta and bookkeeping records).
fn turn_of(v: &Value) -> Option<Turn> {
    if v.get("isSidechain").and_then(Value::as_bool) == Some(true) {
        return None;
    }
    let finished = || {
        let at = v.get("timestamp").and_then(Value::as_str).and_then(crate::merge::parse_time);
        Turn::Finished(at.and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_millis() as u64))
    };
    match v.get("type").and_then(Value::as_str)? {
        "assistant" => match v.pointer("/message/stop_reason").and_then(Value::as_str) {
            Some("end_turn" | "stop_sequence") => Some(finished()),
            _ => Some(Turn::Active),
        },
        "user" => {
            if v.get("isMeta").and_then(Value::as_bool) == Some(true) {
                return None;
            }
            if v.pointer("/toolUseResult/status").and_then(Value::as_str) == Some("async_launched") {
                return Some(finished());
            }
            let text = match v.pointer("/message/content") {
                Some(Value::String(s)) => Some(s.as_str()),
                Some(Value::Array(items)) => items.first().and_then(|i| i.get("text")).and_then(Value::as_str),
                _ => None,
            };
            if text.is_some_and(|t| t.starts_with("[Request interrupted by user")) {
                return Some(finished());
            }
            Some(Turn::Active)
        }
        "system" => match v.get("subtype").and_then(Value::as_str) {
            Some("turn_duration" | "stop_hook_summary") => Some(finished()),
            _ => None,
        },
        "api-request" => Some(Turn::Active),
        _ => None,
    }
}

impl TranscriptTail {
    /// Usage in percent of `limit` tokens (clamped to 100); `None` without a
    /// usage record or with a zero limit.
    #[cfg(test)]
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
        if tail.tokens.is_some() && tail.model.is_some() && tail.turn.is_some() {
            break;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if tail.turn.is_none() {
            tail.turn = turn_of(&v);
        }
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
                tail.turn = tail.turn.or(wide.turn);
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

    /// The mtime of `path` as of its last [`TranscriptCache::tail`] read.
    pub fn mtime(&self, path: &Path) -> Option<SystemTime> {
        self.entries.get(path).map(|(m, _, _)| *m)
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

/// The standard context window, used for a transcript-only session that has
/// not gone past it.
pub const DEFAULT_WINDOW_TOKENS: u64 = 200_000;
/// The large context window: a session past [`DEFAULT_WINDOW_TOKENS`] must be on it.
pub const LARGE_WINDOW_TOKENS: u64 = 1_000_000;
/// `context_limit_tokens` that every save before the auto window wrote; read as auto.
pub const LEGACY_DEFAULT_LIMIT: u64 = 200_000;

/// Context window for `tokens` used tokens: the configured `limit`, or, when
/// it is 0 (auto) or [`LEGACY_DEFAULT_LIMIT`], 200k up to 200k tokens and 1M
/// beyond.
pub fn window_for(tokens: u64, limit: u64) -> u64 {
    match limit {
        0 | LEGACY_DEFAULT_LIMIT if tokens > DEFAULT_WINDOW_TOKENS => LARGE_WINDOW_TOKENS,
        0 | LEGACY_DEFAULT_LIMIT => DEFAULT_WINDOW_TOKENS,
        n => n,
    }
}

/// What the monitor saw while a statusline file was fresh: that write's mtime
/// and the transcript tokens at its first sighting. Carried per session from
/// round to round, so a stale file can follow later transcript growth.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CtxCalibration {
    pub file_mtime: SystemTime,
    pub tokens: u64,
}

/// Context usage of one session and its (possibly updated) calibration.
///
/// - Statusline file fresh (within `max_age`): its value; the first sighting
///   of a write records the current transcript `tokens` as the calibration.
/// - File stale: still its value (it is window-relative, the transcript is
///   not), unless the transcript clearly grew since: written more than
///   `max_age` after the file, with more tokens than the calibration of that
///   same write. Then `file % × tokens / calibration tokens`.
/// - No file: transcript `tokens` over [`window_for`] (`limit` 0 = auto).
/// - No file and no tokens: `None`.
///
/// The result is always finite and within 0..=100.
pub fn resolve_ctx(
    file: Option<(SystemTime, f32)>,
    tokens: Option<u64>,
    transcript_mtime: Option<SystemTime>,
    calib: Option<CtxCalibration>,
    limit: u64,
    max_age: Duration,
    now: SystemTime,
) -> (Option<f32>, Option<CtxCalibration>) {
    let Some((mtime, file_pct)) = file else {
        let pct = tokens.and_then(|t| percent(t as f64 / window_for(t, limit) as f64 * 100.0));
        return (pct, None);
    };
    if is_fresh(mtime, now, max_age) {
        let calib = match (calib, tokens) {
            (Some(c), _) if c.file_mtime == mtime => Some(c),
            (_, Some(t)) => Some(CtxCalibration { file_mtime: mtime, tokens: t }),
            (c, None) => c,
        };
        return (percent(f64::from(file_pct)), calib);
    }
    let grown = match (calib, tokens, transcript_mtime) {
        (Some(c), Some(t), Some(tm)) if c.file_mtime == mtime && c.tokens > 0 && t > c.tokens => {
            tm.duration_since(mtime).is_ok_and(|d| d > max_age).then_some((t, c.tokens))
        }
        _ => None,
    };
    let pct = match grown {
        Some((t, base)) => f64::from(file_pct) * (t as f64 / base as f64),
        None => f64::from(file_pct),
    };
    (percent(pct), calib)
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

    fn close(got: Option<f32>, want: f32) -> bool {
        got.is_some_and(|g| (g - want).abs() < 0.01)
    }

    const HOUR: Duration = Duration::from_secs(3600);

    #[test]
    fn ctx_latest_keeps_stale_files_with_their_mtime() {
        let home = tempfile::tempdir().unwrap();
        let dirs = ctx_dirs_in(home.path());
        std::fs::create_dir_all(&dirs[1]).unwrap();
        let p = write_ctx(&dirs[1], "s", r#"{"session_id":"s","used_percentage":11}"#);
        let m = mtime(&p);
        // an hour old: still the authoritative value, with its mtime
        assert_eq!(read_ctx_latest(&dirs, "s"), Some((m, 11.0)));
        assert_eq!(read_ctx_latest(&dirs, "other"), None);
        write_ctx(&dirs[1], "bad", "{{{");
        assert_eq!(read_ctx_latest(&dirs, "bad"), None);
    }

    #[test]
    fn stale_ctx_file_is_kept_not_replaced_by_the_transcript() {
        // measured case: statusline 11 % of a 1M window, transcript 113k tokens,
        // file an hour old. Before: 113k / 200k = 56.6 %. Now: 11 %.
        let m = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let now = m + HOUR;
        let (pct, cal) = resolve_ctx(Some((m, 11.0)), Some(113_230), Some(now), None, 0, FRESH, now);
        assert_eq!(pct, Some(11.0));
        assert_eq!(cal, None, "never seen fresh: no calibration");
        // a calibration without growth changes nothing
        let known = CtxCalibration { file_mtime: m, tokens: 113_230 };
        assert_eq!(resolve_ctx(Some((m, 11.0)), Some(113_230), Some(now), Some(known), 0, FRESH, now), (Some(11.0), Some(known)));
        // a big transcript never pushes a stale file to 100 % by itself
        assert_eq!(resolve_ctx(Some((m, 11.0)), Some(900_000), Some(now), None, 0, FRESH, now).0, Some(11.0));
    }

    #[test]
    fn fresh_ctx_file_wins_and_calibrates() {
        let m = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let now = m + Duration::from_secs(10);
        let (pct, cal) = resolve_ctx(Some((m, 11.0)), Some(113_230), Some(now), None, 0, FRESH, now);
        assert_eq!(pct, Some(11.0));
        assert_eq!(cal, Some(CtxCalibration { file_mtime: m, tokens: 113_230 }));
        // the same write keeps its first calibration (tokens at the write)
        let (_, again) = resolve_ctx(Some((m, 11.0)), Some(130_000), Some(now), cal, 0, FRESH, now + Duration::from_secs(5));
        assert_eq!(again, cal);
        // a new write re-calibrates
        let m2 = m + Duration::from_secs(60);
        let (pct, cal2) = resolve_ctx(Some((m2, 13.0)), Some(130_000), Some(m2), cal, 0, FRESH, m2);
        assert_eq!((pct, cal2), (Some(13.0), Some(CtxCalibration { file_mtime: m2, tokens: 130_000 })));
        // fresh without a transcript value: the previous calibration stays
        assert_eq!(resolve_ctx(Some((m2, 13.0)), None, None, cal2, 0, FRESH, m2), (Some(13.0), cal2));
    }

    #[test]
    fn stale_ctx_file_scales_with_clear_transcript_growth() {
        let m = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let now = m + HOUR;
        let cal = Some(CtxCalibration { file_mtime: m, tokens: 100_000 });
        // transcript written 30 min after the file, 250k tokens vs 100k at the write
        let grown = Some(m + Duration::from_secs(1800));
        assert!(close(resolve_ctx(Some((m, 10.0)), Some(250_000), grown, cal, 0, FRESH, now).0, 25.0));
        // the calibration is carried unchanged
        assert_eq!(resolve_ctx(Some((m, 10.0)), Some(250_000), grown, cal, 0, FRESH, now).1, cal);
        // transcript touched only within the fresh window after the file: not "clearly grown"
        let soon = Some(m + Duration::from_secs(120));
        assert_eq!(resolve_ctx(Some((m, 10.0)), Some(250_000), soon, cal, 0, FRESH, now).0, Some(10.0));
        // fewer tokens than at the write (e.g. after /compact): the file value
        assert_eq!(resolve_ctx(Some((m, 10.0)), Some(50_000), grown, cal, 0, FRESH, now).0, Some(10.0));
        // a calibration of another write does not apply
        let other = Some(CtxCalibration { file_mtime: m - HOUR, tokens: 100_000 });
        assert_eq!(resolve_ctx(Some((m, 10.0)), Some(250_000), grown, other, 0, FRESH, now).0, Some(10.0));
        // huge growth clamps at 100
        assert_eq!(resolve_ctx(Some((m, 60.0)), Some(1_000_000), grown, cal, 0, FRESH, now).0, Some(100.0));
    }

    #[test]
    fn no_ctx_file_infers_the_window_from_the_tokens() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let pct = |tokens: u64, limit: u64| resolve_ctx(None, Some(tokens), Some(now), None, limit, FRESH, now).0;
        assert!(close(pct(113_230, 0), 56.615), "<= 200k: 200k window");
        assert_eq!(pct(200_000, 0), Some(100.0));
        assert!(close(pct(240_000, 0), 24.0), "> 200k must be the 1M window");
        assert!(close(pct(356_220, 0), 35.622));
        assert_eq!(pct(2_000_000, 0), Some(100.0), "clamped");
        // an explicit limit is honoured
        assert!(close(pct(113_230, 1_000_000), 11.323));
        assert_eq!(pct(150_000, 100_000), Some(100.0));
        // the old default, written by every earlier save, means auto
        assert!(close(pct(240_000, LEGACY_DEFAULT_LIMIT), 24.0));
        assert!(close(pct(113_230, LEGACY_DEFAULT_LIMIT), 56.615));
        assert_eq!(window_for(1, 0), 200_000);
        assert_eq!(window_for(200_001, 0), 1_000_000);
        assert_eq!(window_for(5, 300_000), 300_000);
        // no transcript value (brand-new session): None
        assert_eq!(resolve_ctx(None, None, None, None, 0, FRESH, now), (None, None));
    }

    #[test]
    fn resolved_ctx_is_never_nan_or_over_100() {
        let m = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let now = m + HOUR;
        let grown = Some(m + Duration::from_secs(1800));
        for file_pct in [0.0f32, 0.5, 50.0, 100.0] {
            for cal_tokens in [0u64, 1, 100_000] {
                for tokens in [None, Some(0u64), Some(1), Some(u64::MAX)] {
                    for limit in [0u64, 1, 200_000, u64::MAX] {
                        let cal = Some(CtxCalibration { file_mtime: m, tokens: cal_tokens });
                        for file in [None, Some((m, file_pct))] {
                            let got = resolve_ctx(file, tokens, grown, cal, limit, FRESH, now).0;
                            if let Some(g) = got {
                                assert!(g.is_finite() && (0.0..=100.0).contains(&g), "{file:?} {tokens:?} {limit} -> {g}");
                            }
                        }
                    }
                }
            }
        }
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
        assert_eq!(read_transcript_tail(&t), Some(TranscriptTail { tokens: Some(1000), model: None, turn: Some(Turn::Active) }));
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
        // no usage or model yet; the first prompt is an active turn
        assert_eq!(cache.tail(&t), Some(TranscriptTail { turn: Some(Turn::Active), ..TranscriptTail::default() }));
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

    /// 2026-11-04T05:00:00.000Z
    const T0_MS: u64 = 1_793_768_400_000;

    fn rec(body: &str, secs: u64) -> String {
        format!(r#"{{{body},"isSidechain":false,"timestamp":"2026-11-04T05:00:{secs:02}.000Z"}}"#)
    }

    fn turn_of(lines: &[String]) -> Option<Turn> {
        let dir = tempfile::tempdir().unwrap();
        let t = dir.path().join("t.jsonl");
        write_lines(&t, lines);
        read_transcript_tail(&t).unwrap().turn
    }

    /// Measured tail shapes (record kinds only, synthetic bodies).
    #[test]
    fn transcript_turn_state_table() {
        let a_tool = rec(r#""type":"assistant","message":{"stop_reason":"tool_use","content":[{"type":"tool_use"}]}"#, 1);
        let tool_result = rec(r#""type":"user","message":{"content":[{"type":"tool_result"}]}"#, 2);
        let api = rec(r#""type":"api-request""#, 3);
        let a_end = rec(r#""type":"assistant","message":{"stop_reason":"end_turn","content":[{"type":"text"}]}"#, 4);
        let hook_sum = rec(r#""type":"system","subtype":"stop_hook_summary""#, 5);
        let duration = rec(r#""type":"system","subtype":"turn_duration""#, 6);
        let attach = rec(r#""type":"attachment""#, 7);
        let meta = r#"{"type":"mode"}"#.to_string();
        let prompt = rec(r#""type":"user","message":{"content":"next"}"#, 8);
        let async_launch = rec(
            r#""type":"user","message":{"content":[{"type":"tool_result"}]},"toolUseResult":{"status":"async_launched","isAsync":true}"#,
            9,
        );
        let interrupted = rec(r#""type":"user","message":{"content":[{"type":"text","text":"[Request interrupted by user]"}]}"#, 10);
        let meta_user = rec(r#""type":"user","isMeta":true,"message":{"content":"x"}"#, 11);
        let side = r#"{"type":"user","isSidechain":true,"message":{"content":"x"},"timestamp":"2026-11-04T05:00:12.000Z"}"#.to_string();
        let local = rec(r#""type":"system","subtype":"local_command""#, 13);
        let fin = |secs: u64| Some(Turn::Finished(Some(T0_MS + secs * 1000)));
        let cases: Vec<(Vec<String>, Option<Turn>, &str)> = vec![
            (vec![a_tool.clone()], Some(Turn::Active), "tool running"),
            (vec![a_tool.clone(), tool_result.clone()], Some(Turn::Active), "tool result, model next"),
            (vec![tool_result.clone(), api.clone()], Some(Turn::Active), "request in flight"),
            (vec![a_tool.clone(), a_end.clone()], fin(4), "end_turn"),
            (vec![a_end.clone(), hook_sum.clone(), duration.clone(), attach.clone(), meta.clone()], fin(6), "ended + trailing metadata"),
            (vec![duration.clone(), prompt.clone()], Some(Turn::Active), "new prompt after a finished turn"),
            (vec![a_tool.clone(), async_launch.clone(), meta.clone()], fin(9), "parked on a background agent"),
            (vec![a_tool.clone(), interrupted.clone()], fin(10), "interrupted"),
            (vec![duration.clone(), meta_user.clone(), side.clone(), local.clone()], fin(6), "meta, sidechain, local command skipped"),
            (vec![meta.clone(), attach.clone()], None, "no turn record"),
            (vec![], None, "empty"),
        ];
        for (lines, want, why) in cases {
            assert_eq!(turn_of(&lines), want, "{why}");
        }
        // an unparsable timestamp still finishes the turn, without a time
        let no_ts = r#"{"type":"system","subtype":"turn_duration","timestamp":"later"}"#.to_string();
        assert_eq!(turn_of(&[a_tool, no_ts]), Some(Turn::Finished(None)));
    }

    #[test]
    fn transcript_turn_is_never_carried_over_by_the_cache() {
        let dir = tempfile::tempdir().unwrap();
        let t = dir.path().join("t.jsonl");
        write_lines(&t, &[rec(r#""type":"system","subtype":"turn_duration""#, 1)]);
        let mut cache = TranscriptCache::default();
        assert!(matches!(cache.tail(&t).unwrap().turn, Some(Turn::Finished(_))));
        append(&t, &big_user(RETRY_BYTES as usize + 1000));
        assert_eq!(cache.tail(&t).unwrap().turn, None, "an unknown turn is not the old one");
    }
}
