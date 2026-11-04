//! Claude Code subagents of a session, read from the folder next to its
//! transcript: `<transcript without .jsonl>/subagents/` holds, per subagent,
//! `agent-<id>.meta.json` (agentType, description, model, and on newer Claude
//! Code versions `finished: true` once it is done) and `agent-<id>.jsonl`
//! (its own transcript, appended while it works).
//!
//! A subagent counts as running while its transcript was written within the
//! freshness window and its meta does not say `finished`. Older Claude Code
//! versions never write `finished`, so a just-finished subagent may still
//! count for up to the window.
//!
//! Cost per round and session: one `stat` of the folder (a `read_dir` only
//! when the folder changed), one `stat` per subagent transcript, and a `stat`
//! of the meta files of the shown subagents; a meta file is parsed once per
//! (mtime, length). Transcript bodies are never read.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::Deserialize;

use crate::model::model_family;

/// At most this many subagents (the newest) are kept per session.
pub const MAX_SUBAGENTS: usize = 8;
/// Larger meta files are not parsed (they fall back to the defaults).
const META_MAX_BYTES: u64 = 64 * 1024;
/// Kind shown when the meta has none.
const DEFAULT_KIND: &str = "agent";

/// One subagent of a session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubagentInfo {
    /// The `<id>` of `agent-<id>.*`.
    pub id: String,
    /// `agentType` from the meta (e.g. `Explore`), else `agent`.
    pub kind: String,
    /// The task description from the meta, one line; empty when unknown.
    pub description: String,
    /// Model family word (`opus`, `sonnet`, `haiku`, `fable`) when known.
    pub model_family: Option<String>,
    /// Recently active and not marked finished (and the parent is not done;
    /// the refresh clears it for finished sessions).
    pub running: bool,
    /// Seconds since its transcript was last written.
    pub last_activity_secs: u64,
}

/// Subagents marked running.
pub fn running_count(list: &[SubagentInfo]) -> usize {
    list.iter().filter(|a| a.running).count()
}

/// The subagents folder of a session transcript: the transcript path without
/// its extension, plus `subagents`.
pub fn subagents_dir(transcript: &Path) -> PathBuf {
    transcript.with_extension("").join("subagents")
}

/// Meta fields as written by Claude Code; a field of the wrong type is `None`.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawMeta {
    #[serde(rename = "agentType", deserialize_with = "super::lenient")]
    agent_type: Option<String>,
    #[serde(deserialize_with = "super::lenient")]
    description: Option<String>,
    #[serde(deserialize_with = "super::lenient")]
    model: Option<String>,
    #[serde(deserialize_with = "super::lenient")]
    finished: Option<bool>,
}

/// The parsed, display-ready meta of one subagent.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Meta {
    kind: String,
    description: String,
    family: Option<String>,
    finished: bool,
}

impl Default for Meta {
    fn default() -> Self {
        Meta { kind: DEFAULT_KIND.into(), description: String::new(), family: None, finished: false }
    }
}

/// One line: control characters as spaces, trimmed.
fn one_line(s: &str) -> String {
    s.chars().map(|c| if c.is_control() { ' ' } else { c }).collect::<String>().trim().to_string()
}

/// Parses a meta file; anything unreadable, too big or malformed gives the defaults.
fn read_meta(path: &Path, len: u64) -> Meta {
    if len > META_MAX_BYTES {
        return Meta::default();
    }
    let Ok(bytes) = std::fs::read(path) else { return Meta::default() };
    let Ok(raw) = serde_json::from_slice::<RawMeta>(&bytes) else { return Meta::default() };
    let kind = raw.agent_type.as_deref().map(one_line).filter(|k| !k.is_empty()).unwrap_or_else(|| DEFAULT_KIND.into());
    Meta {
        kind,
        description: raw.description.as_deref().map(one_line).unwrap_or_default(),
        family: raw.model.as_deref().and_then(model_family).map(|f| f.as_key().to_string()),
        finished: raw.finished == Some(true),
    }
}

/// Cached state of one subagents folder.
#[derive(Debug, Clone)]
struct DirCache {
    /// Folder mtime of the listing (`None`: unknown, list every time).
    mtime: Option<SystemTime>,
    ids: Vec<String>,
    /// Parsed metas by id, with the (mtime, length) they were read at.
    metas: HashMap<String, (SystemTime, u64, Meta)>,
}

/// Folder listings and parsed metas, carried from one refresh round to the next.
#[derive(Debug, Clone, Default)]
pub struct SubagentCache {
    dirs: HashMap<PathBuf, DirCache>,
    #[cfg(test)]
    meta_reads: usize,
    #[cfg(test)]
    list_reads: usize,
}

/// The `<id>` of an `agent-<id>.jsonl` / `agent-<id>.meta.json` file name.
fn agent_id(name: &str) -> Option<&str> {
    let rest = name.strip_prefix("agent-")?;
    let id = rest.strip_suffix(".meta.json").or_else(|| rest.strip_suffix(".jsonl"))?;
    (!id.is_empty()).then_some(id)
}

fn list_ids(dir: &Path) -> Option<Vec<String>> {
    let mut ids = BTreeSet::new();
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let name = entry.file_name();
        if let Some(id) = name.to_str().and_then(agent_id) {
            ids.insert(id.to_string());
        }
    }
    Some(ids.into_iter().collect())
}

/// mtime of a regular file.
fn file_mtime(path: &Path) -> Option<SystemTime> {
    let md = std::fs::metadata(path).ok()?;
    md.is_file().then(|| md.modified().ok()).flatten()
}

impl SubagentCache {
    /// Keeps only the folders in `dirs` (the sessions of the round).
    pub fn retain(&mut self, dirs: &[PathBuf]) {
        self.dirs.retain(|d, _| dirs.contains(d));
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.dirs.len()
    }

    /// Meta files parsed (cache misses), for tests.
    #[cfg(test)]
    pub fn meta_reads(&self) -> usize {
        self.meta_reads
    }

    /// Folder listings done (cache misses), for tests.
    #[cfg(test)]
    pub fn list_reads(&self) -> usize {
        self.list_reads
    }
}

/// The newest [`MAX_SUBAGENTS`] subagents in `dir`, newest first. Running:
/// written within `active_secs` of `now` and not marked finished. A missing
/// or unreadable folder yields nothing.
pub fn scan(dir: &Path, now: SystemTime, active_secs: u64, cache: &mut SubagentCache) -> Vec<SubagentInfo> {
    let md = match std::fs::metadata(dir) {
        Ok(md) if md.is_dir() => md,
        _ => {
            cache.dirs.remove(dir);
            return Vec::new();
        }
    };
    let mtime = md.modified().ok();
    let fresh = cache.dirs.get(dir).is_some_and(|d| d.mtime.is_some() && d.mtime == mtime);
    if !fresh {
        #[cfg(test)]
        {
            cache.list_reads += 1;
        }
        let Some(ids) = list_ids(dir) else {
            cache.dirs.remove(dir);
            return Vec::new();
        };
        let mut metas = cache.dirs.remove(dir).map(|d| d.metas).unwrap_or_default();
        metas.retain(|id, _| ids.contains(id));
        cache.dirs.insert(dir.to_path_buf(), DirCache { mtime, ids, metas });
    }
    let Some(entry) = cache.dirs.get_mut(dir) else { return Vec::new() };
    let mut reads = 0usize;

    let mut seen: Vec<(SystemTime, &str)> = entry
        .ids
        .iter()
        .filter_map(|id| {
            let t = file_mtime(&dir.join(format!("agent-{id}.jsonl")))
                .or_else(|| file_mtime(&dir.join(format!("agent-{id}.meta.json"))))?;
            Some((t, id.as_str()))
        })
        .collect();
    seen.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(b.1)));
    seen.truncate(MAX_SUBAGENTS);

    let mut out = Vec::with_capacity(seen.len());
    for (t, id) in seen {
        let path = dir.join(format!("agent-{id}.meta.json"));
        let meta = match std::fs::metadata(&path).ok().filter(|m| m.is_file()) {
            None => Meta::default(),
            Some(m) => {
                let key = (m.modified().unwrap_or(SystemTime::UNIX_EPOCH), m.len());
                match entry.metas.get(id) {
                    Some((mt, len, meta)) if (*mt, *len) == key => meta.clone(),
                    _ => {
                        reads += 1;
                        let meta = read_meta(&path, key.1);
                        entry.metas.insert(id.to_string(), (key.0, key.1, meta.clone()));
                        meta
                    }
                }
            }
        };
        let secs = now.duration_since(t).map(|d| d.as_secs()).unwrap_or(0);
        out.push(SubagentInfo {
            id: id.to_string(),
            kind: meta.kind,
            description: meta.description,
            model_family: meta.family,
            running: !meta.finished && secs <= active_secs,
            last_activity_secs: secs,
        });
    }
    #[cfg(test)]
    {
        cache.meta_reads += reads;
    }
    #[cfg(not(test))]
    let _ = reads;
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn touch(path: &Path, at: SystemTime) {
        std::fs::File::options().write(true).open(path).unwrap().set_modified(at).unwrap();
    }

    /// Writes a synthetic subagent: transcript (`jsonl`, mtime `age` ago) and,
    /// when given, the raw meta file.
    fn agent(dir: &Path, id: &str, meta: Option<&[u8]>, now: SystemTime, age: u64) {
        std::fs::create_dir_all(dir).unwrap();
        let jsonl = dir.join(format!("agent-{id}.jsonl"));
        std::fs::write(&jsonl, "{\"type\":\"user\"}\n").unwrap();
        touch(&jsonl, now - Duration::from_secs(age));
        if let Some(m) = meta {
            std::fs::write(dir.join(format!("agent-{id}.meta.json")), m).unwrap();
        }
    }

    fn meta(kind: &str, desc: &str, model: &str) -> Vec<u8> {
        format!(r#"{{"agentType":"{kind}","description":"{desc}","model":"{model}","spawnDepth":1,"toolUseId":"toolu_x"}}"#).into_bytes()
    }

    #[test]
    fn running_by_recent_activity_newest_first() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("subagents");
        let now = SystemTime::now();
        agent(&dir, "aaa", Some(&meta("Explore", "find the parser", "sonnet")), now, 5);
        agent(&dir, "bbb", Some(&meta("general-purpose", "old job", "opus")), now, 120);
        agent(&dir, "ccc", Some(&meta("Plan", "plan it", "claude-haiku-5-5")), now, 1);
        let mut cache = SubagentCache::default();
        let out = scan(&dir, now, 30, &mut cache);
        let ids: Vec<&str> = out.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, ["ccc", "aaa", "bbb"], "newest first");
        let running: Vec<bool> = out.iter().map(|a| a.running).collect();
        assert_eq!(running, [true, true, false], "stale beyond the window");
        assert_eq!((out[1].kind.as_str(), out[1].description.as_str()), ("Explore", "find the parser"));
        let fam: Vec<Option<&str>> = out.iter().map(|a| a.model_family.as_deref()).collect();
        assert_eq!(fam, [Some("haiku"), Some("sonnet"), Some("opus")]);
        let ages: Vec<u64> = out.iter().map(|a| a.last_activity_secs).collect();
        assert_eq!(ages, [1, 5, 120]);
        // a wider window counts the old one too
        assert!(scan(&dir, now, 600, &mut cache).iter().all(|a| a.running));
        // the window edge is inclusive
        assert!(scan(&dir, now, 120, &mut cache)[2].running);
        assert!(!scan(&dir, now, 119, &mut cache)[2].running);
    }

    #[test]
    fn finished_meta_is_never_running() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("subagents");
        let now = SystemTime::now();
        let done = br#"{"agentType":"Explore","description":"done","model":"sonnet","finished":true}"#;
        let open = br#"{"agentType":"Explore","description":"open","model":"sonnet","finished":false}"#;
        agent(&dir, "done", Some(done), now, 1);
        agent(&dir, "open", Some(open), now, 2);
        let out = scan(&dir, now, 30, &mut SubagentCache::default());
        assert_eq!(out.iter().map(|a| (a.id.as_str(), a.running)).collect::<Vec<_>>(), [("done", false), ("open", true)]);
    }

    #[test]
    fn missing_or_malformed_meta_falls_back() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("subagents");
        let now = SystemTime::now();
        agent(&dir, "none", None, now, 1);
        agent(&dir, "garbage", Some(b"{{{ not json"), now, 2);
        agent(&dir, "binary", Some(&[0xff, 0xfe, 0x00, 0x9c, b'{']), now, 3);
        agent(&dir, "types", Some(br#"{"agentType":5,"description":"kept","model":["x"]}"#), now, 4);
        agent(&dir, "ctrl", Some(b"{\"agentType\":\"\",\"description\":\"  line one\\nline\\ttwo  \"}"), now, 5);
        let big = format!(r#"{{"agentType":"Explore","description":"{}"}}"#, "x".repeat(200_000));
        agent(&dir, "huge", Some(big.as_bytes()), now, 6);
        let out = scan(&dir, now, 30, &mut SubagentCache::default());
        let got: Vec<(&str, &str, &str, Option<&str>)> =
            out.iter().map(|a| (a.id.as_str(), a.kind.as_str(), a.description.as_str(), a.model_family.as_deref())).collect();
        assert_eq!(
            got,
            [
                ("none", "agent", "", None),
                ("garbage", "agent", "", None),
                ("binary", "agent", "", None),
                ("types", "agent", "kept", None),
                ("ctrl", "agent", "line one line two", None),
                ("huge", "agent", "", None),
            ]
        );
        assert!(out.iter().all(|a| a.running));
    }

    #[test]
    fn huge_or_binary_transcripts_are_only_stat_ed() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("subagents");
        std::fs::create_dir_all(&dir).unwrap();
        let now = SystemTime::now();
        let jsonl = dir.join("agent-big.jsonl");
        std::fs::write(&jsonl, vec![0xffu8; 4 << 20]).unwrap();
        touch(&jsonl, now - Duration::from_secs(3));
        let out = scan(&dir, now, 30, &mut SubagentCache::default());
        assert_eq!(out.len(), 1);
        assert!(out[0].running && out[0].last_activity_secs == 3);
    }

    #[test]
    fn caps_at_eight_newest() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("subagents");
        let now = SystemTime::now();
        for i in 0..11u64 {
            agent(&dir, &format!("a{i:02}"), Some(&meta("Explore", "x", "opus")), now, 10 * i);
        }
        // unrelated files are ignored
        std::fs::write(dir.join("notes.txt"), "x").unwrap();
        std::fs::create_dir(dir.join("agent-sub.jsonl.d")).unwrap();
        let mut cache = SubagentCache::default();
        let out = scan(&dir, now, 30, &mut cache);
        let ids: Vec<&str> = out.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, ["a00", "a01", "a02", "a03", "a04", "a05", "a06", "a07"]);
        assert_eq!(MAX_SUBAGENTS, 8);
        assert_eq!(cache.meta_reads(), 8, "metas parsed only for the shown agents");
    }

    #[test]
    fn cache_parses_each_meta_once_and_lists_the_dir_once_while_unchanged() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("subagents");
        let now = SystemTime::now();
        agent(&dir, "aaa", Some(&meta("Explore", "one", "opus")), now, 1);
        agent(&dir, "bbb", Some(&meta("Plan", "two", "opus")), now, 2);
        let mut cache = SubagentCache::default();
        let first = scan(&dir, now, 30, &mut cache);
        assert_eq!((cache.meta_reads(), cache.list_reads()), (2, 1));
        // later rounds: transcripts keep growing, metas and the listing are reused
        let later = now + Duration::from_secs(10);
        touch(&dir.join("agent-aaa.jsonl"), later);
        let second = scan(&dir, later, 30, &mut cache);
        assert_eq!((cache.meta_reads(), cache.list_reads()), (2, 1), "cache hits");
        assert_eq!(second[0].id, "aaa");
        assert_eq!(second[0].last_activity_secs, 0);
        assert_eq!(first[0].description, second[0].description);
        // a rewritten meta (e.g. marked finished) is parsed again
        std::fs::write(dir.join("agent-bbb.meta.json"), br#"{"agentType":"Plan","description":"two","finished":true}"#).unwrap();
        let third = scan(&dir, later, 30, &mut cache);
        assert_eq!(cache.meta_reads(), 3);
        assert!(!third.iter().find(|a| a.id == "bbb").unwrap().running);
        // a new agent changes the folder: listed again
        agent(&dir, "ccc", None, later, 0);
        let fourth = scan(&dir, later, 30, &mut cache);
        assert_eq!(cache.list_reads(), 2);
        assert_eq!(fourth.len(), 3);
    }

    #[test]
    fn missing_dir_or_file_yields_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let now = SystemTime::now();
        let mut cache = SubagentCache::default();
        assert!(scan(&tmp.path().join("nope"), now, 30, &mut cache).is_empty());
        let file = tmp.path().join("file");
        std::fs::write(&file, "x").unwrap();
        assert!(scan(&file, now, 30, &mut cache).is_empty());
        assert!(scan(tmp.path(), now, 30, &mut cache).is_empty(), "an empty dir");
        // a folder that vanishes clears its cached listing
        let dir = tmp.path().join("subagents");
        agent(&dir, "aaa", None, now, 1);
        assert_eq!(scan(&dir, now, 30, &mut cache).len(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(scan(&dir, now, 30, &mut cache).is_empty());
    }

    #[test]
    fn unreadable_dir_yields_nothing() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("subagents");
        let now = SystemTime::now();
        agent(&dir, "aaa", None, now, 1);
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o000)).unwrap();
        let out = scan(&dir, now, 30, &mut SubagentCache::default());
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        // root can still read it; everyone else gets nothing (never a panic)
        assert!(out.len() <= 1);
    }

    #[test]
    fn future_mtime_counts_as_now() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("subagents");
        let now = SystemTime::now();
        agent(&dir, "aaa", None, now, 0);
        touch(&dir.join("agent-aaa.jsonl"), now + Duration::from_secs(50));
        let out = scan(&dir, now, 30, &mut SubagentCache::default());
        assert_eq!((out[0].last_activity_secs, out[0].running), (0, true));
    }

    #[test]
    fn retain_prunes_closed_sessions() {
        let tmp = tempfile::tempdir().unwrap();
        let now = SystemTime::now();
        let (a, b) = (tmp.path().join("a").join("subagents"), tmp.path().join("b").join("subagents"));
        agent(&a, "aaa", Some(&meta("Explore", "x", "opus")), now, 1);
        agent(&b, "bbb", Some(&meta("Explore", "y", "opus")), now, 1);
        let mut cache = SubagentCache::default();
        scan(&a, now, 30, &mut cache);
        scan(&b, now, 30, &mut cache);
        assert_eq!(cache.len(), 2);
        cache.retain(std::slice::from_ref(&a));
        assert_eq!(cache.len(), 1);
        scan(&a, now, 30, &mut cache);
        assert_eq!(cache.meta_reads(), 2, "the kept session's metas stay cached");
        cache.retain(&[]);
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn subagents_dir_sits_next_to_the_transcript() {
        assert_eq!(subagents_dir(Path::new("/p/proj/sess-1.jsonl")), PathBuf::from("/p/proj/sess-1/subagents"));
        assert_eq!(subagents_dir(Path::new("/p/proj/sess-1")), PathBuf::from("/p/proj/sess-1/subagents"));
    }

    #[test]
    fn running_count_counts_running_only() {
        let a = |running| SubagentInfo { id: "x".into(), kind: "agent".into(), description: String::new(), model_family: None, running, last_activity_secs: 0 };
        assert_eq!(running_count(&[a(true), a(false), a(true)]), 2);
        assert_eq!(running_count(&[]), 0);
    }
}
