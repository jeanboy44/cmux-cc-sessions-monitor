//! Background collector that syncs Claude Code memories, conversations,
//! and tool executions in real-time into SQLite.

use std::fs::File;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use serde_json::Value;

use super::db::Db;

/// Default directory for Claude projects.
pub fn default_projects_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".claude")
        .join("projects")
}

/// Default path to claude-mem SQLite database.
pub fn default_claude_mem_db_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".claude-mem")
        .join("claude-mem.db")
}

/// Extracts a human-friendly project name from the encoded directory name.
/// e.g. "-Users-user-workspace-my-proj" -> "my-proj" (or full decoded path).
pub fn decode_project_name(folder_name: &str) -> String {
    let clean = folder_name.trim_start_matches('-');
    if let Some(last) = clean.split('-').last() {
        if !last.is_empty() {
            return last.to_string();
        }
    }
    folder_name.to_string()
}

/// Parses frontmatter and body from a markdown file.
pub fn parse_memory_markdown(text: &str) -> (String, String, String, Option<String>, String) {
    let mut title = String::new();
    let mut description = String::new();
    let mut mem_type = String::from("memory");
    let mut origin_session_id = None;
    let mut body = text.to_string();

    let trimmed = text.trim_start();
    if trimmed.starts_with("---") {
        if let Some(end_idx) = trimmed[3..].find("\n---") {
            let yaml_str = &trimmed[3..3 + end_idx];
            body = trimmed[3 + end_idx + 4..].trim_start().to_string();

            for line in yaml_str.lines() {
                let line = line.trim();
                if let Some(rest) = line.strip_prefix("name:") {
                    title = rest.trim().trim_matches('"').trim_matches('\'').to_string();
                } else if let Some(rest) = line.strip_prefix("description:") {
                    description = rest.trim().trim_matches('"').trim_matches('\'').to_string();
                } else if let Some(rest) = line.strip_prefix("type:") {
                    mem_type = rest.trim().trim_matches('"').trim_matches('\'').to_string();
                } else if let Some(rest) = line.strip_prefix("originSessionId:") {
                    origin_session_id = Some(rest.trim().trim_matches('"').trim_matches('\'').to_string());
                }
            }
        }
    }

    if title.is_empty() {
        // Fall back to first markdown header or first non-empty line
        for line in body.lines() {
            let l = line.trim();
            if let Some(h) = l.strip_prefix('#') {
                title = h.trim().trim_start_matches('#').trim().to_string();
                break;
            }
        }
    }

    (title, description, mem_type, origin_session_id, body)
}

/// Helper to get current epoch milliseconds from SystemTime.
fn epoch_ms(st: SystemTime) -> i64 {
    st.duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Integrates structured observations from thedotmack/claude-mem SQLite database if present.
pub fn sync_claude_mem_db(db: &Db, path: &Path) -> Result<usize> {
    if !path.is_file() {
        return Ok(0);
    }

    let conn = match rusqlite::Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    ) {
        Ok(c) => c,
        Err(_) => return Ok(0),
    };

    let path_str = path.to_string_lossy().into_owned();
    let (last_offset, _) = db.get_sync_state(&path_str)?.unwrap_or((0, 0));

    let mut stmt = match conn.prepare(
        "SELECT id, memory_session_id, project, type, title, subtitle, narrative, facts, concepts, files_read, files_modified, created_at_epoch
         FROM observations WHERE id > ?1 ORDER BY id ASC LIMIT 500"
    ) {
        Ok(s) => s,
        Err(_) => return Ok(0),
    };

    let rows = stmt.query_map([last_offset as i64], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, Option<String>>(4)?.unwrap_or_default(),
            r.get::<_, Option<String>>(5)?.unwrap_or_default(),
            r.get::<_, Option<String>>(6)?.unwrap_or_default(),
            r.get::<_, Option<String>>(7)?.unwrap_or_else(|| "[]".into()),
            r.get::<_, Option<String>>(8)?.unwrap_or_else(|| "[]".into()),
            r.get::<_, Option<String>>(9)?.unwrap_or_else(|| "[]".into()),
            r.get::<_, Option<String>>(10)?.unwrap_or_else(|| "[]".into()),
            r.get::<_, i64>(11)?,
        ))
    })?;

    let mut count = 0;
    let mut max_id = last_offset;

    for row in rows.flatten() {
        let (id, session_id, proj, o_type, title, subtitle, narrative, facts, concepts, f_read, f_mod, epoch) = row;
        db.upsert_observation(
            &session_id,
            &proj,
            &o_type,
            &title,
            &subtitle,
            &narrative,
            &facts,
            &concepts,
            &f_read,
            &f_mod,
            "claude-mem",
            epoch,
        )?;
        if (id as u64) > max_id {
            max_id = id as u64;
        }
        count += 1;
    }

    if count > 0 {
        let _ = db.set_sync_state(&path_str, max_id, epoch_ms(SystemTime::now()));
    }

    Ok(count)
}

/// Synchronizes all memories and transcripts from `projects_dir` into `db`.
pub fn sync_once(db: &Db, projects_dir: &Path) -> Result<usize> {
    let mut count = 0;

    // 1. Sync claude-mem database if available
    let cm_path = default_claude_mem_db_path();
    count += sync_claude_mem_db(db, &cm_path).unwrap_or(0);

    let Ok(entries) = std::fs::read_dir(projects_dir) else {
        return Ok(count);
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let folder_name = path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let project_name = decode_project_name(&folder_name);

        // 2. Sync memory files
        let mem_dir = path.join("memory");
        if mem_dir.is_dir() {
            count += sync_memory_dir(db, &mem_dir, &project_name)?;
        }

        // 3. Sync session transcripts (*.jsonl)
        count += sync_transcripts_in(db, &path, &project_name)?;
    }

    Ok(count)
}

fn sync_memory_dir(db: &Db, mem_dir: &Path, project_name: &str) -> Result<usize> {
    let mut updated = 0;
    let Ok(entries) = std::fs::read_dir(mem_dir) else {
        return Ok(0);
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "md") {
            let file_name = path
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            let Ok(meta) = std::fs::metadata(&path) else {
                continue;
            };
            let mtime_ms = meta.modified().map(epoch_ms).unwrap_or(0);
            let path_str = path.to_string_lossy().into_owned();

            let last_sync = db.get_sync_state(&path_str)?;
            if let Some((_, last_mtime)) = last_sync {
                if mtime_ms <= last_mtime {
                    continue; // No change
                }
            }

            if let Ok(content) = std::fs::read_to_string(&path) {
                let (mut title, desc, mem_type, origin_id, body) = parse_memory_markdown(&content);
                if title.is_empty() {
                    title = file_name.trim_end_matches(".md").to_string();
                }

                db.upsert_memory(
                    project_name,
                    &file_name,
                    &title,
                    &desc,
                    &mem_type,
                    origin_id.as_deref(),
                    &body,
                    mtime_ms,
                )?;

                // Also record as a structured observation
                let _ = db.upsert_observation(
                    origin_id.as_deref().unwrap_or(project_name),
                    project_name,
                    "memory",
                    &format!("메모리: {title}"),
                    &desc,
                    &body,
                    "[]",
                    &format!("[\"{mem_type}\"]"),
                    "[]",
                    "[]",
                    "auto-memory",
                    mtime_ms,
                );

                db.set_sync_state(&path_str, meta.len(), mtime_ms)?;
                updated += 1;
            }
        }
    }
    Ok(updated)
}

fn sync_transcripts_in(db: &Db, project_dir: &Path, project_name: &str) -> Result<usize> {
    let mut updated = 0;
    let Ok(entries) = std::fs::read_dir(project_dir) else {
        return Ok(0);
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "jsonl") {
            let session_id = path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            if session_id.is_empty() {
                continue;
            }

            let Ok(meta) = std::fs::metadata(&path) else {
                continue;
            };
            let len = meta.len();
            let mtime_ms = meta.modified().map(epoch_ms).unwrap_or(0);
            let path_str = path.to_string_lossy().into_owned();

            let (last_offset, last_mtime) = db.get_sync_state(&path_str)?.unwrap_or((0, 0));
            if len <= last_offset && mtime_ms <= last_mtime {
                continue;
            }

            let start_offset = if len < last_offset { 0 } else { last_offset };
            let Ok(mut file) = File::open(&path) else {
                continue;
            };
            if file.seek(SeekFrom::Start(start_offset)).is_err() {
                continue;
            }

            let mut reader = BufReader::new(file);
            let mut line = String::new();
            let mut current_offset = start_offset;

            while let Ok(bytes_read) = reader.read_line(&mut line) {
                if bytes_read == 0 {
                    break;
                }
                // Avoid incomplete line
                if !line.ends_with('\n') && !line.ends_with('\r') {
                    break;
                }
                current_offset += bytes_read as u64;

                if let Ok(val) = serde_json::from_str::<Value>(&line) {
                    process_transcript_entry(db, &val, &session_id, project_name);
                    updated += 1;
                }
                line.clear();
            }

            let _ = db.set_sync_state(&path_str, current_offset, mtime_ms);
        }
    }
    Ok(updated)
}

fn process_transcript_entry(db: &Db, val: &Value, session_id: &str, project_name: &str) {
    let msg_type = val.get("type").and_then(|v| v.as_str()).unwrap_or_default();
    let uuid = val.get("uuid").and_then(|v| v.as_str());
    let parent_uuid = val.get("parentUuid").and_then(|v| v.as_str());
    let ts_ms = val
        .get("timestamp")
        .and_then(|v| v.as_str())
        .and_then(|s| chrono_like_parse(s))
        .unwrap_or(0);

    let message = val.get("message");
    let role = message
        .and_then(|m| m.get("role"))
        .and_then(|v| v.as_str())
        .unwrap_or(msg_type);

    let model = message
        .and_then(|m| m.get("model"))
        .and_then(|v| v.as_str())
        .or_else(|| val.get("requestedModel").and_then(|v| v.as_str()));

    if role == "user" {
        // Extract human prompt or tool result
        if let Some(msg) = message {
            if let Some(content_str) = msg.get("content").and_then(|v| v.as_str()) {
                if !content_str.trim().is_empty() {
                    let _ = db.insert_conversation(
                        session_id,
                        project_name,
                        "user",
                        uuid,
                        parent_uuid,
                        ts_ms,
                        None,
                        content_str,
                    );
                }
            } else if let Some(arr) = msg.get("content").and_then(|v| v.as_array()) {
                for item in arr {
                    let block_type = item.get("type").and_then(|v| v.as_str()).unwrap_or_default();
                    if block_type == "text" {
                        if let Some(t) = item.get("text").and_then(|v| v.as_str()) {
                            if !t.trim().is_empty() {
                                let _ = db.insert_conversation(
                                    session_id,
                                    project_name,
                                    "user",
                                    uuid,
                                    parent_uuid,
                                    ts_ms,
                                    None,
                                    t,
                                );
                            }
                        }
                    } else if block_type == "tool_result" {
                        if let Some(tool_use_id) = item.get("tool_use_id").and_then(|v| v.as_str()) {
                            let is_error = item.get("is_error").and_then(|v| v.as_bool()).unwrap_or(false);
                            let status = if is_error { "error" } else { "success" };
                            let snippet = item
                                .get("content")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default();
                            let short_snippet: String = snippet.chars().take(200).collect();
                            let _ = db.update_execution_result(tool_use_id, status, &short_snippet);
                        }
                    }
                }
            }
        }
    } else if role == "assistant" {
        if let Some(msg) = message {
            let mut text_parts = Vec::new();
            if let Some(content_str) = msg.get("content").and_then(|v| v.as_str()) {
                text_parts.push(content_str.to_string());
            } else if let Some(arr) = msg.get("content").and_then(|v| v.as_array()) {
                for item in arr {
                    let block_type = item.get("type").and_then(|v| v.as_str()).unwrap_or_default();
                    if block_type == "text" {
                        if let Some(t) = item.get("text").and_then(|v| v.as_str()) {
                            text_parts.push(t.to_string());
                        }
                    } else if block_type == "tool_use" {
                        let tool_name = item.get("name").and_then(|v| v.as_str()).unwrap_or("unknown");
                        let tool_id = item.get("id").and_then(|v| v.as_str()).unwrap_or_default();
                        let input = item.get("input").cloned().unwrap_or(Value::Null);
                        let input_json = serde_json::to_string(&input).unwrap_or_default();

                        // Short command hint
                        let hint = extract_tool_hint(tool_name, &input);
                        let _ = db.upsert_execution(
                            session_id,
                            project_name,
                            tool_id,
                            tool_name,
                            &input_json,
                            &hint,
                            "running",
                            "",
                            ts_ms,
                        );

                        // Observation classification (claude-mem pattern)
                        let (obs_type, title, f_read, f_mod) = match tool_name {
                            "Edit" | "Write" | "FileEdit" | "NotebookEdit" => {
                                ("change", format!("코드 수정: {hint}"), "[]".to_string(), format!("[\"{hint}\"]"))
                            }
                            "Read" => {
                                ("discovery", format!("파일 확인: {hint}"), format!("[\"{hint}\"]"), "[]".to_string())
                            }
                            "Bash" => {
                                ("execution", format!("명령어 실행: {hint}"), "[]".to_string(), "[]".to_string())
                            }
                            _ => ("tool", format!("{tool_name}: {hint}"), "[]".to_string(), "[]".to_string()),
                        };

                        let _ = db.upsert_observation(
                            session_id,
                            project_name,
                            obs_type,
                            &title,
                            &hint,
                            &input_json,
                            "[]",
                            "[]",
                            &f_read,
                            &f_mod,
                            "native",
                            ts_ms,
                        );
                    }
                }
            }

            let full_text = text_parts.join("\n");
            if !full_text.trim().is_empty() {
                let _ = db.insert_conversation(
                    session_id,
                    project_name,
                    "assistant",
                    uuid,
                    parent_uuid,
                    ts_ms,
                    model,
                    &full_text,
                );
            }
        }
    }
}

/// Generates a concise hint for the tool execution (e.g. bash command or file path).
fn extract_tool_hint(tool_name: &str, input: &Value) -> String {
    match tool_name {
        "Bash" => input
            .get("description")
            .and_then(|v| v.as_str())
            .or_else(|| input.get("command").and_then(|v| v.as_str()))
            .map(|s| s.lines().next().unwrap_or(s))
            .unwrap_or("")
            .chars()
            .take(60)
            .collect(),
        "Read" | "Edit" | "Write" | "FileEdit" | "NotebookEdit" => input
            .get("file_path")
            .or_else(|| input.get("path"))
            .and_then(|v| v.as_str())
            .map(|s| {
                Path::new(s)
                    .file_name()
                    .map(|f| f.to_string_lossy().into_owned())
                    .unwrap_or_else(|| s.to_string())
            })
            .unwrap_or_default(),
        "Glob" | "Grep" => input
            .get("pattern")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        _ => String::new(),
    }
}

/// Parses ISO-8601 timestamp string into epoch milliseconds without heavy chrono dependency.
fn chrono_like_parse(iso: &str) -> Option<i64> {
    // Expected format: "2026-11-03T03:46:27.327Z" or "+09:00"
    let parts: Vec<&str> = iso.split('T').collect();
    if parts.len() != 2 {
        return None;
    }
    let date_parts: Vec<i64> = parts[0].split('-').filter_map(|s| s.parse().ok()).collect();
    if date_parts.len() != 3 {
        return None;
    }
    let y = date_parts[0];
    let m = date_parts[1];
    let d = date_parts[2];

    let mut time_str = parts[1].trim_end_matches('Z');
    let mut tz_offset_secs: i64 = 0;
    if let Some(pos) = time_str.rfind('+') {
        let (t, tz) = time_str.split_at(pos);
        time_str = t;
        let tz_parts: Vec<i64> = tz[1..].split(':').filter_map(|s| s.parse().ok()).collect();
        if !tz_parts.is_empty() {
            let tz_h = tz_parts[0];
            let tz_m = tz_parts.get(1).copied().unwrap_or(0);
            tz_offset_secs = tz_h * 3600 + tz_m * 60;
        }
    } else if let Some(pos) = time_str.rfind('-') {
        let (t, tz) = time_str.split_at(pos);
        time_str = t;
        let tz_parts: Vec<i64> = tz[1..].split(':').filter_map(|s| s.parse().ok()).collect();
        if !tz_parts.is_empty() {
            let tz_h = tz_parts[0];
            let tz_m = tz_parts.get(1).copied().unwrap_or(0);
            tz_offset_secs = -(tz_h * 3600 + tz_m * 60);
        }
    }

    let time_sub: Vec<&str> = time_str.split('.').collect();
    let time_parts: Vec<i64> = time_sub[0].split(':').filter_map(|s| s.parse().ok()).collect();
    if time_parts.len() != 3 {
        return None;
    }
    let hour = time_parts[0];
    let min = time_parts[1];
    let sec = time_parts[2];
    let ms = if time_sub.len() > 1 {
        let frac = time_sub[1];
        let padded = format!("{:<03}", &frac[..frac.len().min(3)]);
        padded.parse::<i64>().unwrap_or(0)
    } else {
        0
    };

    // Days since unix epoch (1970-01-01) approx algorithm
    let mut total_days = (y - 1970) * 365 + (y - 1969) / 4;
    let days_in_month = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let is_leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    for (idx, &dm) in days_in_month.iter().enumerate() {
        if (idx as i64 + 1) < m {
            total_days += dm;
            if idx == 1 && is_leap {
                total_days += 1;
            }
        }
    }
    total_days += d - 1;

    let total_secs = total_days * 86400 + hour * 3600 + min * 60 + sec - tz_offset_secs;
    Some(total_secs * 1000 + ms)
}

/// Spawns the background collector thread.
pub fn spawn_collector(
    db: Arc<Db>,
    projects_dir: Option<PathBuf>,
    interval: Duration,
    stop: Arc<AtomicBool>,
) -> JoinHandle<()> {
    std::thread::Builder::new()
        .name("memory-collector".into())
        .spawn(move || {
            let dir = projects_dir.unwrap_or_else(default_projects_dir);
            // Run initial sync
            let _ = sync_once(&db, &dir);

            while !stop.load(Ordering::Relaxed) {
                std::thread::sleep(interval);
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                let _ = sync_once(&db, &dir);
            }
        })
        .expect("failed to spawn memory collector thread")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_frontmatter() {
        let md = r#"---
name: my-spec
description: This is a test spec
metadata:
  type: project
  originSessionId: 00000000-0000-0000-0000-000000000042
---
# Actual Heading

Some body content here."#;

        let (title, desc, mem_type, origin, body) = parse_memory_markdown(md);
        assert_eq!(title, "my-spec");
        assert_eq!(desc, "This is a test spec");
        assert_eq!(mem_type, "project");
        assert_eq!(origin, Some("00000000-0000-0000-0000-000000000042".into()));
        assert!(body.contains("Some body content here."));
    }

    #[test]
    fn test_sync_once_with_mock_files() {
        let dir = tempfile::tempdir().unwrap();
        let proj_dir = dir.path().join("-Users-user-workspace-myproj");
        let mem_dir = proj_dir.join("memory");
        std::fs::create_dir_all(&mem_dir).unwrap();

        let mem_file = mem_dir.join("note.md");
        std::fs::write(&mem_file, "# Project Note\n\nImportant details.").unwrap();

        let transcript = proj_dir.join("00000000-0000-0000-0000-000000000099.jsonl");
        let lines = r#"{"type":"user","message":{"role":"user","content":"build something"},"timestamp":"2026-11-03T03:46:27.000Z","uuid":"00000000-0000-0000-0000-000000000001"}
{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"sure!"},{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"echo hello"}}]},"timestamp":"2026-11-03T03:46:28.000Z","uuid":"00000000-0000-0000-0000-000000000002"}
{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"hello\n"}]},"timestamp":"2026-11-03T03:46:29.000Z"}
"#;
        std::fs::write(&transcript, lines).unwrap();

        let db = Db::open_in_memory().unwrap();
        let count = sync_once(&db, dir.path()).unwrap();
        assert!(count >= 3);

        let mems = db.list_memories(None, None, 10).unwrap();
        assert_eq!(mems.len(), 1);
        assert_eq!(mems[0].title, "Project Note");
        assert_eq!(mems[0].project_name, "myproj");

        let convs = db.list_conversations(None, None, None, 10).unwrap();
        assert_eq!(convs.len(), 2);

        let execs = db.list_executions(None, None, None, 10).unwrap();
        assert_eq!(execs.len(), 1);
        assert_eq!(execs[0].tool_name, "Bash");
        assert_eq!(execs[0].command_hint, "echo hello");
        assert_eq!(execs[0].status, "success");
    }

    #[test]
    fn test_chrono_like_parse_timezones() {
        // UTC
        let t1 = chrono_like_parse("2026-11-03T12:00:00.000Z").unwrap();
        // +09:00 (Seoul) at 21:00 should equal 12:00 UTC
        let t2 = chrono_like_parse("2026-11-03T21:00:00.000+09:00").unwrap();
        assert_eq!(t1, t2);

        // -05:00 (EST) at 07:00 should equal 12:00 UTC
        let t3 = chrono_like_parse("2026-11-03T07:00:00.000-05:00").unwrap();
        assert_eq!(t1, t3);
    }
}
