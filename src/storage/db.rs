//! SQLite persistence for Claude Code memories, conversations, and tool executions.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use serde::Serialize;
use crate::storage::Timezone;

/// Normalizes epoch timestamp (either seconds or milliseconds) to milliseconds.
fn epoch_to_ms(epoch: i64) -> i64 {
    if epoch > 100_000_000_000 {
        epoch
    } else {
        epoch * 1000
    }
}

/// Default location of the SQLite database.
pub fn default_db_path() -> PathBuf {
    if let Ok(p) = std::env::var("CMUX_MAGIGENT_DB") {
        return PathBuf::from(p);
    }
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    home.join(".local").join("share").join("cmux-magigent").join("records.db")
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[allow(dead_code)]
pub struct MemoryRecord {
    pub id: i64,
    pub project_name: String,
    pub file_name: String,
    pub title: String,
    pub description: String,
    pub memory_type: String,
    pub origin_session_id: Option<String>,
    pub content: String,
    pub mtime_ms: i64,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[allow(dead_code)]
pub struct ConversationRecord {
    pub id: i64,
    pub session_id: String,
    pub project_name: String,
    pub role: String,
    pub uuid: Option<String>,
    pub parent_uuid: Option<String>,
    pub timestamp_ms: i64,
    pub model: Option<String>,
    pub content: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[allow(dead_code)]
pub struct ExecutionRecord {
    pub id: i64,
    pub session_id: String,
    pub project_name: String,
    pub tool_use_id: String,
    pub tool_name: String,
    pub input_json: String,
    pub command_hint: String,
    pub status: String,
    pub result_snippet: String,
    pub timestamp_ms: i64,
    pub created_at: String,
}

/// Structured observation inspired by thedotmack/claude-mem (decisions, changes, bugfixes, discoveries).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[allow(dead_code)]
pub struct ObservationRecord {
    pub id: i64,
    pub session_id: String,
    pub project_name: String,
    pub obs_type: String,
    pub title: String,
    pub subtitle: String,
    pub narrative: String,
    pub facts: String,
    pub concepts: String,
    pub files_read: String,
    pub files_modified: String,
    pub source: String,
    pub created_at_epoch: i64,
    pub created_at: String,
}

/// Chronological item for progressive disclosure timeline view (claude-mem 3-tier pattern).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[allow(dead_code)]
pub struct TimelineItem {
    pub timestamp_ms: i64,
    pub item_type: String,
    pub project: String,
    pub session_id: String,
    pub title: String,
    pub detail: String,
    pub badge: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct StorageStats {
    pub memory_count: usize,
    pub conversation_count: usize,
    pub execution_count: usize,
    pub observation_count: usize,
    pub project_count: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct SessionPacketStats {
    pub conversation_count: usize,
    pub user_message_count: usize,
    pub assistant_message_count: usize,
    pub execution_count: usize,
    pub tool_counts: Vec<(String, usize)>,
    pub observation_count: usize,
    pub memory_count: usize,
}

/// Consolidated package containing all memory, conversation, and execution data for a specific session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SessionPacket {
    pub session_id: String,
    pub project_name: String,
    pub started_at_ms: Option<i64>,
    pub ended_at_ms: Option<i64>,
    pub stats: SessionPacketStats,
    pub observations: Vec<ObservationRecord>,
    pub conversations: Vec<ConversationRecord>,
    pub executions: Vec<ExecutionRecord>,
    pub memories: Vec<MemoryRecord>,
}

impl SessionPacket {
    /// Formats the session packet into a clean, human-readable Markdown report.
    pub fn to_markdown(&self, tz: Timezone) -> String {
        let mut out = String::new();
        out.push_str(&format!("# Session Memory Report: {}\n\n", self.session_id));
        out.push_str(&format!("- **프로젝트**: {}\n", self.project_name));

        if let (Some(start), Some(end)) = (self.started_at_ms, self.ended_at_ms) {
            let start_str = tz.format_epoch_ms(start);
            let end_str = tz.format_epoch_ms(end);
            out.push_str(&format!("- **활동 시간**: {} ~ {} ({})\n", start_str, end_str, tz.as_code()));
        } else if let Some(start) = self.started_at_ms {
            out.push_str(&format!("- **시작 시각**: {} ({})\n", tz.format_epoch_ms(start), tz.as_code()));
        }

        out.push_str(&format!(
            "- **통계 요약**: 대화 {}건 (User {}, Assistant {}) · 도구 실행 {}건 · 관찰 {}건 · 메모리 {}건\n",
            self.stats.conversation_count,
            self.stats.user_message_count,
            self.stats.assistant_message_count,
            self.stats.execution_count,
            self.stats.observation_count,
            self.stats.memory_count,
        ));

        if !self.stats.tool_counts.is_empty() {
            let tools_summary: Vec<String> = self
                .stats
                .tool_counts
                .iter()
                .map(|(name, count)| format!("{name}: {count}"))
                .collect();
            out.push_str(&format!("- **사용 도구**: {}\n", tools_summary.join(", ")));
        }

        out.push_str("\n---\n\n");

        // 1. Observations
        if !self.observations.is_empty() {
            out.push_str("## 📌 관찰 및 핵심 기록 (Observations)\n\n");
            for obs in &self.observations {
                let time_str = if obs.created_at_epoch > 0 {
                    tz.format_epoch_ms(epoch_to_ms(obs.created_at_epoch))
                } else if !obs.created_at.is_empty() && obs.created_at != "-" {
                    tz.format_iso_utc(&obs.created_at)
                } else {
                    "-".to_string()
                };
                out.push_str(&format!("### [{}] {} ({})\n", obs.obs_type, obs.title, time_str));
                if !obs.subtitle.is_empty() {
                    out.push_str(&format!("*{}*\n\n", obs.subtitle));
                }
                if !obs.narrative.is_empty() {
                    out.push_str(&format!("{}\n\n", obs.narrative));
                }
                if obs.facts != "[]" && !obs.facts.is_empty() {
                    out.push_str(&format!("- **Facts**: {}\n", obs.facts));
                }
                if obs.files_modified != "[]" && !obs.files_modified.is_empty() {
                    out.push_str(&format!("- **Files Modified**: {}\n", obs.files_modified));
                }
                out.push('\n');
            }
            out.push_str("---\n\n");
        }

        // 2. Associated Memories
        if !self.memories.is_empty() {
            out.push_str("## 🧠 생성된 영구 메모리 (Memories)\n\n");
            for mem in &self.memories {
                let time_str = tz.format_epoch_ms(mem.mtime_ms);
                out.push_str(&format!("### {} [{}] ({})\n", mem.title, mem.memory_type, time_str));
                if !mem.description.is_empty() {
                    out.push_str(&format!("*{}*\n\n", mem.description));
                }
                out.push_str(&format!("```markdown\n{}\n```\n\n", mem.content));
            }
            out.push_str("---\n\n");
        }

        // 3. Executions (Tools)
        if !self.executions.is_empty() {
            out.push_str(&format!("## 🛠️ 도구 실행 이력 (Executions - {}건)\n\n", self.executions.len()));
            for exec in &self.executions {
                let time_str = tz.format_epoch_ms(exec.timestamp_ms);
                let hint = if !exec.command_hint.is_empty() {
                    &exec.command_hint
                } else {
                    &exec.tool_name
                };
                let status_icon = match exec.status.as_str() {
                    "success" => "✅",
                    "error" => "❌",
                    _ => "⏳",
                };
                out.push_str(&format!("- `{}` {} **{}** (`{}`) [{}]\n", time_str, status_icon, exec.tool_name, hint, exec.status));
                if !exec.result_snippet.is_empty() {
                    let chars: Vec<char> = exec.result_snippet.chars().collect();
                    let snippet = if chars.len() > 160 {
                        let prefix: String = chars[..157].iter().collect();
                        format!("{prefix}...")
                    } else {
                        exec.result_snippet.clone()
                    };
                    out.push_str(&format!("  > {}\n", snippet.replace('\n', " ")));
                }
            }
            out.push_str("\n---\n\n");
        }

        // 4. Conversations
        if !self.conversations.is_empty() {
            out.push_str(&format!("## 💬 대화 기록 (Conversations - {}턴)\n\n", self.conversations.len()));
            for conv in &self.conversations {
                let time_str = tz.format_epoch_ms(conv.timestamp_ms);
                let role_header = match conv.role.as_str() {
                    "user" => format!("### 👤 User ({time_str})"),
                    "assistant" => {
                        let model_info = conv.model.as_deref().map(|m| format!(" [{m}]")).unwrap_or_default();
                        format!("### 🤖 Assistant ({time_str}){model_info}")
                    }
                    _ => format!("### ⚙️ {} ({time_str})", conv.role),
                };
                out.push_str(&format!("{role_header}\n\n{}\n\n", conv.content));
            }
        }

        out
    }
}

pub struct Db {
    conn: Mutex<Connection>,
    pub path: PathBuf,
}

impl Db {
    /// Opens or creates the database at `path`, setting up tables and WAL mode.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let conn = Connection::open(path)
            .with_context(|| format!("failed to open sqlite db at {}", path.display()))?;

        // PRAGMAs inspired by thedotmack/claude-mem for high throughput and performance
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        let _: String = conn.query_row("PRAGMA journal_mode=WAL;", [], |r| r.get(0)).unwrap_or_default();
        conn.execute_batch(
            "PRAGMA synchronous = NORMAL; \
             PRAGMA foreign_keys = ON; \
             PRAGMA temp_store = memory; \
             PRAGMA mmap_size = 268435456; \
             PRAGMA cache_size = 10000;"
        )?;

        let db = Self {
            conn: Mutex::new(conn),
            path: path.to_path_buf(),
        };
        db.init_schema()?;
        Ok(db)
    }

    /// Opens an in-memory database.
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        let db = Self {
            conn: Mutex::new(conn),
            path: PathBuf::from(":memory:"),
        };
        db.init_schema()?;
        Ok(db)
    }

    fn init_schema(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS memories (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                project_name TEXT NOT NULL,
                file_name TEXT NOT NULL,
                title TEXT NOT NULL,
                description TEXT DEFAULT '',
                memory_type TEXT DEFAULT 'general',
                origin_session_id TEXT,
                content TEXT NOT NULL,
                mtime_ms INTEGER NOT NULL,
                created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
                updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
                UNIQUE(project_name, file_name)
            );

            CREATE TABLE IF NOT EXISTS conversations (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT NOT NULL,
                project_name TEXT NOT NULL,
                role TEXT NOT NULL,
                uuid TEXT UNIQUE,
                parent_uuid TEXT,
                timestamp_ms INTEGER NOT NULL,
                model TEXT,
                content TEXT NOT NULL,
                created_at DATETIME DEFAULT CURRENT_TIMESTAMP
            );

            CREATE TABLE IF NOT EXISTS executions (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT NOT NULL,
                project_name TEXT NOT NULL,
                tool_use_id TEXT UNIQUE,
                tool_name TEXT NOT NULL,
                input_json TEXT DEFAULT '',
                command_hint TEXT DEFAULT '',
                status TEXT DEFAULT 'running',
                result_snippet TEXT DEFAULT '',
                timestamp_ms INTEGER NOT NULL,
                created_at DATETIME DEFAULT CURRENT_TIMESTAMP
            );

            CREATE TABLE IF NOT EXISTS observations (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT NOT NULL,
                project_name TEXT NOT NULL,
                obs_type TEXT NOT NULL DEFAULT 'change',
                title TEXT NOT NULL,
                subtitle TEXT DEFAULT '',
                narrative TEXT DEFAULT '',
                facts TEXT DEFAULT '[]',
                concepts TEXT DEFAULT '[]',
                files_read TEXT DEFAULT '[]',
                files_modified TEXT DEFAULT '[]',
                source TEXT NOT NULL DEFAULT 'native',
                created_at_epoch INTEGER NOT NULL,
                created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
                UNIQUE(session_id, title, created_at_epoch)
            );

            CREATE TABLE IF NOT EXISTS sync_state (
                source_path TEXT PRIMARY KEY,
                last_offset INTEGER DEFAULT 0,
                last_mtime_ms INTEGER DEFAULT 0,
                updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
            );

            CREATE INDEX IF NOT EXISTS idx_memories_proj ON memories(project_name);
            CREATE INDEX IF NOT EXISTS idx_memories_title ON memories(title);
            CREATE INDEX IF NOT EXISTS idx_conv_session ON conversations(session_id);
            CREATE INDEX IF NOT EXISTS idx_conv_proj ON conversations(project_name);
            CREATE INDEX IF NOT EXISTS idx_exec_session ON executions(session_id);
            CREATE INDEX IF NOT EXISTS idx_exec_tool ON executions(tool_name);
            CREATE INDEX IF NOT EXISTS idx_obs_proj ON observations(project_name);
            CREATE INDEX IF NOT EXISTS idx_obs_type ON observations(obs_type);
            CREATE INDEX IF NOT EXISTS idx_obs_epoch ON observations(created_at_epoch DESC);
            "#,
        )?;
        Ok(())
    }

    /// Upserts a memory record.
    pub fn upsert_memory(
        &self,
        project_name: &str,
        file_name: &str,
        title: &str,
        description: &str,
        memory_type: &str,
        origin_session_id: Option<&str>,
        content: &str,
        mtime_ms: i64,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            r#"
            INSERT INTO memories (
                project_name, file_name, title, description, memory_type,
                origin_session_id, content, mtime_ms, updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, CURRENT_TIMESTAMP)
            ON CONFLICT(project_name, file_name) DO UPDATE SET
                title = excluded.title,
                description = excluded.description,
                memory_type = excluded.memory_type,
                origin_session_id = excluded.origin_session_id,
                content = excluded.content,
                mtime_ms = excluded.mtime_ms,
                updated_at = CURRENT_TIMESTAMP
            "#,
            params![
                project_name,
                file_name,
                title,
                description,
                memory_type,
                origin_session_id,
                content,
                mtime_ms
            ],
        )?;
        Ok(())
    }

    /// Inserts a conversation message if its uuid does not already exist.
    pub fn insert_conversation(
        &self,
        session_id: &str,
        project_name: &str,
        role: &str,
        uuid: Option<&str>,
        parent_uuid: Option<&str>,
        timestamp_ms: i64,
        model: Option<&str>,
        content: &str,
    ) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let rows = conn.execute(
            r#"
            INSERT OR IGNORE INTO conversations (
                session_id, project_name, role, uuid, parent_uuid,
                timestamp_ms, model, content
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
            "#,
            params![
                session_id,
                project_name,
                role,
                uuid,
                parent_uuid,
                timestamp_ms,
                model,
                content
            ],
        )?;
        Ok(rows > 0)
    }

    /// Upserts a tool execution record.
    pub fn upsert_execution(
        &self,
        session_id: &str,
        project_name: &str,
        tool_use_id: &str,
        tool_name: &str,
        input_json: &str,
        command_hint: &str,
        status: &str,
        result_snippet: &str,
        timestamp_ms: i64,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            r#"
            INSERT INTO executions (
                session_id, project_name, tool_use_id, tool_name,
                input_json, command_hint, status, result_snippet, timestamp_ms
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
            ON CONFLICT(tool_use_id) DO UPDATE SET
                status = CASE
                    WHEN executions.status IN ('success', 'error') AND excluded.status = 'running' THEN executions.status
                    ELSE excluded.status
                END,
                result_snippet = CASE WHEN excluded.result_snippet != '' THEN excluded.result_snippet ELSE executions.result_snippet END
            "#,
            params![
                session_id,
                project_name,
                tool_use_id,
                tool_name,
                input_json,
                command_hint,
                status,
                result_snippet,
                timestamp_ms
            ],
        )?;
        Ok(())
    }

    /// Updates execution result and status for a given `tool_use_id`.
    pub fn update_execution_result(&self, tool_use_id: &str, status: &str, result_snippet: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE executions SET status = ?1, result_snippet = ?2 WHERE tool_use_id = ?3",
            params![status, result_snippet, tool_use_id],
        )?;
        Ok(())
    }

    /// Retrieves sync state for a file path: (last_offset, last_mtime_ms).
    pub fn get_sync_state(&self, path: &str) -> Result<Option<(u64, i64)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT last_offset, last_mtime_ms FROM sync_state WHERE source_path = ?1")?;
        let mut rows = stmt.query([path])?;
        if let Some(r) = rows.next()? {
            let offset: i64 = r.get(0)?;
            let mtime: i64 = r.get(1)?;
            Ok(Some((offset as u64, mtime)))
        } else {
            Ok(None)
        }
    }

    /// Stores sync state for a file path.
    pub fn set_sync_state(&self, path: &str, last_offset: u64, last_mtime_ms: i64) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            r#"
            INSERT INTO sync_state (source_path, last_offset, last_mtime_ms, updated_at)
            VALUES (?1, ?2, ?3, CURRENT_TIMESTAMP)
            ON CONFLICT(source_path) DO UPDATE SET
                last_offset = excluded.last_offset,
                last_mtime_ms = excluded.last_mtime_ms,
                updated_at = CURRENT_TIMESTAMP
            "#,
            params![path, last_offset as i64, last_mtime_ms],
        )?;
        Ok(())
    }

    /// Lists memories, optionally filtered by project or query, sorted newest first.
    pub fn list_memories(
        &self,
        project_filter: Option<&str>,
        query: Option<&str>,
        limit: usize,
    ) -> Result<Vec<MemoryRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut sql = String::from(
            "SELECT id, project_name, file_name, title, description, memory_type, origin_session_id, content, mtime_ms, updated_at FROM memories WHERE 1=1"
        );
        let mut cond_params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

        if let Some(p) = project_filter.filter(|s| !s.is_empty()) {
            sql.push_str(" AND (project_name = ? OR project_name LIKE ?)");
            cond_params.push(Box::new(p.to_string()));
            cond_params.push(Box::new(format!("%{p}%")));
        }

        if let Some(q) = query.filter(|s| !s.trim().is_empty()) {
            let pat = format!("%{}%", q.trim());
            sql.push_str(" AND (title LIKE ? OR description LIKE ? OR content LIKE ?)");
            cond_params.push(Box::new(pat.clone()));
            cond_params.push(Box::new(pat.clone()));
            cond_params.push(Box::new(pat));
        }

        sql.push_str(" ORDER BY mtime_ms DESC LIMIT ?");
        cond_params.push(Box::new(limit as i64));

        let mut stmt = conn.prepare(&sql)?;
        let param_refs: Vec<&dyn rusqlite::ToSql> = cond_params.iter().map(|b| b.as_ref()).collect();
        let rows = stmt.query_map(&param_refs[..], |r| {
            Ok(MemoryRecord {
                id: r.get(0)?,
                project_name: r.get(1)?,
                file_name: r.get(2)?,
                title: r.get(3)?,
                description: r.get(4)?,
                memory_type: r.get(5)?,
                origin_session_id: r.get(6)?,
                content: r.get(7)?,
                mtime_ms: r.get(8)?,
                updated_at: r.get(9)?,
            })
        })?;

        let mut out = Vec::new();
        for item in rows {
            out.push(item?);
        }
        Ok(out)
    }

    /// Fetches a memory by id.
    pub fn get_memory(&self, id: i64) -> Result<Option<MemoryRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, project_name, file_name, title, description, memory_type, origin_session_id, content, mtime_ms, updated_at FROM memories WHERE id = ?1"
        )?;
        let mut rows = stmt.query([id])?;
        if let Some(r) = rows.next()? {
            Ok(Some(MemoryRecord {
                id: r.get(0)?,
                project_name: r.get(1)?,
                file_name: r.get(2)?,
                title: r.get(3)?,
                description: r.get(4)?,
                memory_type: r.get(5)?,
                origin_session_id: r.get(6)?,
                content: r.get(7)?,
                mtime_ms: r.get(8)?,
                updated_at: r.get(9)?,
            }))
        } else {
            Ok(None)
        }
    }

    /// Finds a memory by title or filename match.
    pub fn find_memory(&self, query: &str) -> Result<Option<MemoryRecord>> {
        let conn = self.conn.lock().unwrap();
        let pat = format!("%{query}%");
        let mut stmt = conn.prepare(
            r#"
            SELECT id, project_name, file_name, title, description, memory_type, origin_session_id, content, mtime_ms, updated_at
            FROM memories
            WHERE title = ?1 OR file_name = ?1 OR title LIKE ?2 OR file_name LIKE ?2
            ORDER BY mtime_ms DESC LIMIT 1
            "#,
        )?;
        let mut rows = stmt.query(params![query, pat])?;
        if let Some(r) = rows.next()? {
            Ok(Some(MemoryRecord {
                id: r.get(0)?,
                project_name: r.get(1)?,
                file_name: r.get(2)?,
                title: r.get(3)?,
                description: r.get(4)?,
                memory_type: r.get(5)?,
                origin_session_id: r.get(6)?,
                content: r.get(7)?,
                mtime_ms: r.get(8)?,
                updated_at: r.get(9)?,
            }))
        } else {
            Ok(None)
        }
    }

    /// Lists executions, newest first.
    pub fn list_executions(
        &self,
        session_id: Option<&str>,
        project_filter: Option<&str>,
        tool_filter: Option<&str>,
        limit: usize,
    ) -> Result<Vec<ExecutionRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut sql = String::from(
            "SELECT id, session_id, project_name, tool_use_id, tool_name, input_json, command_hint, status, result_snippet, timestamp_ms, created_at FROM executions WHERE 1=1"
        );
        let mut cond_params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

        if let Some(s) = session_id.filter(|s| !s.is_empty()) {
            sql.push_str(" AND session_id = ?");
            cond_params.push(Box::new(s.to_string()));
        }
        if let Some(p) = project_filter.filter(|s| !s.is_empty()) {
            sql.push_str(" AND (project_name = ? OR project_name LIKE ?)");
            cond_params.push(Box::new(p.to_string()));
            cond_params.push(Box::new(format!("%{p}%")));
        }
        if let Some(t) = tool_filter.filter(|s| !s.is_empty()) {
            sql.push_str(" AND tool_name = ?");
            cond_params.push(Box::new(t.to_string()));
        }

        sql.push_str(" ORDER BY timestamp_ms DESC LIMIT ?");
        cond_params.push(Box::new(limit as i64));

        let mut stmt = conn.prepare(&sql)?;
        let param_refs: Vec<&dyn rusqlite::ToSql> = cond_params.iter().map(|b| b.as_ref()).collect();
        let rows = stmt.query_map(&param_refs[..], |r| {
            Ok(ExecutionRecord {
                id: r.get(0)?,
                session_id: r.get(1)?,
                project_name: r.get(2)?,
                tool_use_id: r.get(3)?,
                tool_name: r.get(4)?,
                input_json: r.get(5)?,
                command_hint: r.get(6)?,
                status: r.get(7)?,
                result_snippet: r.get(8)?,
                timestamp_ms: r.get(9)?,
                created_at: r.get(10)?,
            })
        })?;

        let mut out = Vec::new();
        for item in rows {
            out.push(item?);
        }
        Ok(out)
    }

    /// Lists conversations, newest first.
    pub fn list_conversations(
        &self,
        session_id: Option<&str>,
        project_filter: Option<&str>,
        query: Option<&str>,
        limit: usize,
    ) -> Result<Vec<ConversationRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut sql = String::from(
            "SELECT id, session_id, project_name, role, uuid, parent_uuid, timestamp_ms, model, content, created_at FROM conversations WHERE 1=1"
        );
        let mut cond_params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

        if let Some(s) = session_id.filter(|s| !s.is_empty()) {
            sql.push_str(" AND session_id = ?");
            cond_params.push(Box::new(s.to_string()));
        }
        if let Some(p) = project_filter.filter(|s| !s.is_empty()) {
            sql.push_str(" AND (project_name = ? OR project_name LIKE ?)");
            cond_params.push(Box::new(p.to_string()));
            cond_params.push(Box::new(format!("%{p}%")));
        }
        if let Some(q) = query.filter(|s| !s.trim().is_empty()) {
            let pat = format!("%{}%", q.trim());
            sql.push_str(" AND content LIKE ?");
            cond_params.push(Box::new(pat));
        }

        sql.push_str(" ORDER BY timestamp_ms DESC LIMIT ?");
        cond_params.push(Box::new(limit as i64));

        let mut stmt = conn.prepare(&sql)?;
        let param_refs: Vec<&dyn rusqlite::ToSql> = cond_params.iter().map(|b| b.as_ref()).collect();
        let rows = stmt.query_map(&param_refs[..], |r| {
            Ok(ConversationRecord {
                id: r.get(0)?,
                session_id: r.get(1)?,
                project_name: r.get(2)?,
                role: r.get(3)?,
                uuid: r.get(4)?,
                parent_uuid: r.get(5)?,
                timestamp_ms: r.get(6)?,
                model: r.get(7)?,
                content: r.get(8)?,
                created_at: r.get(9)?,
            })
        })?;

        let mut out = Vec::new();
        for item in rows {
            out.push(item?);
        }
        Ok(out)
    }

    /// Upserts a structured observation (claude-mem style).
    pub fn upsert_observation(
        &self,
        session_id: &str,
        project_name: &str,
        obs_type: &str,
        title: &str,
        subtitle: &str,
        narrative: &str,
        facts: &str,
        concepts: &str,
        files_read: &str,
        files_modified: &str,
        source: &str,
        created_at_epoch: i64,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            r#"
            INSERT INTO observations (
                session_id, project_name, obs_type, title, subtitle,
                narrative, facts, concepts, files_read, files_modified,
                source, created_at_epoch
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
            ON CONFLICT(session_id, title, created_at_epoch) DO UPDATE SET
                subtitle = excluded.subtitle,
                narrative = excluded.narrative,
                facts = excluded.facts,
                concepts = excluded.concepts,
                files_read = excluded.files_read,
                files_modified = excluded.files_modified,
                source = excluded.source
            "#,
            params![
                session_id,
                project_name,
                obs_type,
                title,
                subtitle,
                narrative,
                facts,
                concepts,
                files_read,
                files_modified,
                source,
                created_at_epoch,
            ],
        )?;
        Ok(())
    }

    /// Lists structured observations, newest first.
    pub fn list_observations(
        &self,
        project_filter: Option<&str>,
        type_filter: Option<&str>,
        query: Option<&str>,
        limit: usize,
    ) -> Result<Vec<ObservationRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut sql = String::from(
            "SELECT id, session_id, project_name, obs_type, title, subtitle, narrative, facts, concepts, files_read, files_modified, source, created_at_epoch, created_at FROM observations WHERE 1=1"
        );
        let mut cond_params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

        if let Some(p) = project_filter.filter(|s| !s.is_empty()) {
            sql.push_str(" AND (project_name = ? OR project_name LIKE ?)");
            cond_params.push(Box::new(p.to_string()));
            cond_params.push(Box::new(format!("%{p}%")));
        }
        if let Some(t) = type_filter.filter(|s| !s.is_empty()) {
            sql.push_str(" AND obs_type = ?");
            cond_params.push(Box::new(t.to_string()));
        }
        if let Some(q) = query.filter(|s| !s.trim().is_empty()) {
            let pat = format!("%{}%", q.trim());
            sql.push_str(" AND (title LIKE ? OR subtitle LIKE ? OR narrative LIKE ? OR concepts LIKE ?)");
            cond_params.push(Box::new(pat.clone()));
            cond_params.push(Box::new(pat.clone()));
            cond_params.push(Box::new(pat.clone()));
            cond_params.push(Box::new(pat));
        }

        sql.push_str(" ORDER BY created_at_epoch DESC LIMIT ?");
        cond_params.push(Box::new(limit as i64));

        let mut stmt = conn.prepare(&sql)?;
        let param_refs: Vec<&dyn rusqlite::ToSql> = cond_params.iter().map(|b| b.as_ref()).collect();
        let rows = stmt.query_map(&param_refs[..], |r| {
            Ok(ObservationRecord {
                id: r.get(0)?,
                session_id: r.get(1)?,
                project_name: r.get(2)?,
                obs_type: r.get(3)?,
                title: r.get(4)?,
                subtitle: r.get(5)?,
                narrative: r.get(6)?,
                facts: r.get(7)?,
                concepts: r.get(8)?,
                files_read: r.get(9)?,
                files_modified: r.get(10)?,
                source: r.get(11)?,
                created_at_epoch: r.get(12)?,
                created_at: r.get(13)?,
            })
        })?;

        let mut out = Vec::new();
        for item in rows {
            out.push(item?);
        }
        Ok(out)
    }

    /// Chronological unified timeline of all events across projects and sessions.
    pub fn get_timeline(
        &self,
        project_filter: Option<&str>,
        session_filter: Option<&str>,
        limit: usize,
    ) -> Result<Vec<TimelineItem>> {
        let mut items = Vec::new();

        // 1. Observations
        let obss = self.list_observations(project_filter, None, None, limit)?;
        for obs in obss {
            if let Some(s) = session_filter {
                if obs.session_id != s {
                    continue;
                }
            }
            items.push(TimelineItem {
                timestamp_ms: epoch_to_ms(obs.created_at_epoch),
                item_type: "observation".into(),
                project: obs.project_name,
                session_id: obs.session_id,
                title: obs.title,
                detail: if !obs.narrative.is_empty() { obs.narrative } else { obs.subtitle },
                badge: obs.obs_type.to_uppercase(),
            });
        }

        // 2. User Prompts
        let convs = self.list_conversations(session_filter, project_filter, None, limit)?;
        for c in convs {
            if c.role == "user" {
                items.push(TimelineItem {
                    timestamp_ms: c.timestamp_ms,
                    item_type: "prompt".into(),
                    project: c.project_name,
                    session_id: c.session_id,
                    title: c.content.lines().next().unwrap_or("User prompt").to_string(),
                    detail: c.content,
                    badge: "PROMPT".into(),
                });
            }
        }

        // 3. Memories
        let mems = self.list_memories(project_filter, None, limit)?;
        for m in mems {
            items.push(TimelineItem {
                timestamp_ms: m.mtime_ms,
                item_type: "memory".into(),
                project: m.project_name,
                session_id: m.origin_session_id.unwrap_or_default(),
                title: m.title,
                detail: m.content,
                badge: "MEMORY".into(),
            });
        }

        items.sort_by_key(|b| std::cmp::Reverse(b.timestamp_ms));
        items.truncate(limit);
        Ok(items)
    }

    /// Overall statistics.
    pub fn get_stats(&self) -> Result<StorageStats> {
        let conn = self.conn.lock().unwrap();
        let m_count: i64 = conn.query_row("SELECT COUNT(*) FROM memories", [], |r| r.get(0))?;
        let c_count: i64 = conn.query_row("SELECT COUNT(*) FROM conversations", [], |r| r.get(0))?;
        let e_count: i64 = conn.query_row("SELECT COUNT(*) FROM executions", [], |r| r.get(0))?;
        let o_count: i64 = conn.query_row("SELECT COUNT(*) FROM observations", [], |r| r.get(0))?;
        let p_count: i64 = conn.query_row(
            "SELECT COUNT(DISTINCT project_name) FROM (SELECT project_name FROM memories UNION SELECT project_name FROM conversations UNION SELECT project_name FROM observations)",
            [],
            |r| r.get(0),
        )?;

        Ok(StorageStats {
            memory_count: m_count as usize,
            conversation_count: c_count as usize,
            execution_count: e_count as usize,
            observation_count: o_count as usize,
            project_count: p_count as usize,
        })
    }

    /// Resolves a session id from an exact or prefix match across conversations,
    /// executions, observations, and memories.
    pub fn resolve_session_id(&self, query: &str) -> Result<Option<String>> {
        let q = query.trim();
        if q.is_empty() {
            return Ok(None);
        }
        let conn = self.conn.lock().unwrap();

        // 1. Exact match check
        for table in &["conversations", "executions", "observations"] {
            let sql = format!("SELECT session_id FROM {} WHERE session_id = ? LIMIT 1", table);
            let mut stmt = conn.prepare(&sql)?;
            let mut rows = stmt.query(rusqlite::params![q])?;
            if let Some(row) = rows.next()? {
                return Ok(Some(row.get(0)?));
            }
        }
        {
            let mut stmt = conn.prepare("SELECT origin_session_id FROM memories WHERE origin_session_id = ? LIMIT 1")?;
            let mut rows = stmt.query(rusqlite::params![q])?;
            if let Some(row) = rows.next()? {
                if let Ok(id) = row.get::<_, String>(0) {
                    return Ok(Some(id));
                }
            }
        }

        // 2. Prefix match check
        let prefix = format!("{q}%");
        for table in &["conversations", "executions", "observations"] {
            let sql = format!("SELECT session_id FROM {} WHERE session_id LIKE ? ORDER BY id DESC LIMIT 1", table);
            let mut stmt = conn.prepare(&sql)?;
            let mut rows = stmt.query(rusqlite::params![prefix])?;
            if let Some(row) = rows.next()? {
                return Ok(Some(row.get(0)?));
            }
        }
        {
            let mut stmt = conn.prepare("SELECT origin_session_id FROM memories WHERE origin_session_id LIKE ? ORDER BY id DESC LIMIT 1")?;
            let mut rows = stmt.query(rusqlite::params![prefix])?;
            if let Some(row) = rows.next()? {
                if let Ok(id) = row.get::<_, String>(0) {
                    return Ok(Some(id));
                }
            }
        }

        Ok(None)
    }

    /// Fetches all memories, observations, conversations, and executions for a session in a single bundle.
    pub fn get_session_packet(&self, session_query: &str) -> Result<Option<SessionPacket>> {
        let canonical_id = match self.resolve_session_id(session_query)? {
            Some(id) => id,
            None => return Ok(None),
        };

        let conn = self.conn.lock().unwrap();

        // 1. Conversations
        let mut stmt_c = conn.prepare(
            "SELECT id, session_id, project_name, role, uuid, parent_uuid, timestamp_ms, model, content, created_at \
             FROM conversations WHERE session_id = ? ORDER BY timestamp_ms ASC, id ASC"
        )?;
        let conv_rows = stmt_c.query_map(rusqlite::params![canonical_id], |r| {
            Ok(ConversationRecord {
                id: r.get(0)?,
                session_id: r.get(1)?,
                project_name: r.get(2)?,
                role: r.get(3)?,
                uuid: r.get(4)?,
                parent_uuid: r.get(5)?,
                timestamp_ms: r.get(6)?,
                model: r.get(7)?,
                content: r.get(8)?,
                created_at: r.get(9)?,
            })
        })?;
        let mut conversations = Vec::new();
        for c in conv_rows {
            conversations.push(c?);
        }

        // 2. Executions
        let mut stmt_e = conn.prepare(
            "SELECT id, session_id, project_name, tool_use_id, tool_name, input_json, command_hint, status, result_snippet, timestamp_ms, created_at \
             FROM executions WHERE session_id = ? ORDER BY timestamp_ms ASC, id ASC"
        )?;
        let exec_rows = stmt_e.query_map(rusqlite::params![canonical_id], |r| {
            Ok(ExecutionRecord {
                id: r.get(0)?,
                session_id: r.get(1)?,
                project_name: r.get(2)?,
                tool_use_id: r.get(3)?,
                tool_name: r.get(4)?,
                input_json: r.get(5)?,
                command_hint: r.get(6)?,
                status: r.get(7)?,
                result_snippet: r.get(8)?,
                timestamp_ms: r.get(9)?,
                created_at: r.get(10)?,
            })
        })?;
        let mut executions = Vec::new();
        for e in exec_rows {
            executions.push(e?);
        }

        // 3. Observations
        let mut stmt_o = conn.prepare(
            "SELECT id, session_id, project_name, obs_type, title, subtitle, narrative, facts, concepts, files_read, files_modified, source, created_at_epoch, created_at \
             FROM observations WHERE session_id = ? ORDER BY created_at_epoch ASC, id ASC"
        )?;
        let obs_rows = stmt_o.query_map(rusqlite::params![canonical_id], |r| {
            Ok(ObservationRecord {
                id: r.get(0)?,
                session_id: r.get(1)?,
                project_name: r.get(2)?,
                obs_type: r.get(3)?,
                title: r.get(4)?,
                subtitle: r.get(5)?,
                narrative: r.get(6)?,
                facts: r.get(7)?,
                concepts: r.get(8)?,
                files_read: r.get(9)?,
                files_modified: r.get(10)?,
                source: r.get(11)?,
                created_at_epoch: r.get(12)?,
                created_at: r.get(13)?,
            })
        })?;
        let mut observations = Vec::new();
        for o in obs_rows {
            observations.push(o?);
        }

        // 4. Associated Memories
        let mut stmt_m = conn.prepare(
            "SELECT id, project_name, file_name, title, description, memory_type, origin_session_id, content, mtime_ms, updated_at \
             FROM memories WHERE origin_session_id = ? ORDER BY mtime_ms ASC"
        )?;
        let mem_rows = stmt_m.query_map(rusqlite::params![canonical_id], |r| {
            Ok(MemoryRecord {
                id: r.get(0)?,
                project_name: r.get(1)?,
                file_name: r.get(2)?,
                title: r.get(3)?,
                description: r.get(4)?,
                memory_type: r.get(5)?,
                origin_session_id: r.get(6)?,
                content: r.get(7)?,
                mtime_ms: r.get(8)?,
                updated_at: r.get(9)?,
            })
        })?;
        let mut memories = Vec::new();
        for m in mem_rows {
            memories.push(m?);
        }

        // Project name
        let project_name = conversations
            .first()
            .map(|c| c.project_name.clone())
            .or_else(|| executions.first().map(|e| e.project_name.clone()))
            .or_else(|| observations.first().map(|o| o.project_name.clone()))
            .or_else(|| memories.first().map(|m| m.project_name.clone()))
            .unwrap_or_else(|| "unknown".to_string());

        // Timestamps
        let mut min_ts: Option<i64> = None;
        let mut max_ts: Option<i64> = None;

        let mut update_ts = |ts: i64| {
            if ts > 0 {
                min_ts = Some(min_ts.map_or(ts, |m| m.min(ts)));
                max_ts = Some(max_ts.map_or(ts, |m| m.max(ts)));
            }
        };

        for c in &conversations {
            update_ts(c.timestamp_ms);
        }
        for e in &executions {
            update_ts(e.timestamp_ms);
        }
        for o in &observations {
            update_ts(epoch_to_ms(o.created_at_epoch));
        }

        // Stats calculation
        let user_message_count = conversations.iter().filter(|c| c.role == "user").count();
        let assistant_message_count = conversations.iter().filter(|c| c.role == "assistant").count();

        use std::collections::BTreeMap;
        let mut tool_map: BTreeMap<String, usize> = BTreeMap::new();
        for e in &executions {
            *tool_map.entry(e.tool_name.clone()).or_insert(0) += 1;
        }
        let mut tool_counts: Vec<(String, usize)> = tool_map.into_iter().collect();
        tool_counts.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

        let stats = SessionPacketStats {
            conversation_count: conversations.len(),
            user_message_count,
            assistant_message_count,
            execution_count: executions.len(),
            tool_counts,
            observation_count: observations.len(),
            memory_count: memories.len(),
        };

        Ok(Some(SessionPacket {
            session_id: canonical_id,
            project_name,
            started_at_ms: min_ts,
            ended_at_ms: max_ts,
            stats,
            observations,
            conversations,
            executions,
            memories,
        }))
    }

    /// Lists recent distinct session IDs with their project name and newest timestamp.
    pub fn list_recent_session_ids(&self, limit: usize) -> Result<Vec<(String, String, i64)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT session_id, project_name, MAX(ts) as max_ts FROM (
                SELECT session_id, project_name, timestamp_ms as ts FROM conversations
                UNION ALL
                SELECT session_id, project_name, timestamp_ms as ts FROM executions
                UNION ALL
                SELECT session_id, project_name, (CASE WHEN created_at_epoch > 100000000000 THEN created_at_epoch ELSE created_at_epoch * 1000 END) as ts FROM observations
            ) GROUP BY session_id ORDER BY max_ts DESC LIMIT ?"
        )?;
        let rows = stmt.query_map(rusqlite::params![limit as i64], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_memory_upsert_and_query() {
        let db = Db::open_in_memory().unwrap();
        db.upsert_memory(
            "test-project",
            "test-mem.md",
            "Test Memory Title",
            "A description",
            "project",
            Some("00000000-0000-0000-0000-000000000001"),
            "Body content of the memory",
            1000,
        )
        .unwrap();

        let list = db.list_memories(None, None, 10).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].title, "Test Memory Title");
        assert_eq!(list[0].project_name, "test-project");

        // Upsert update
        db.upsert_memory(
            "test-project",
            "test-mem.md",
            "Updated Title",
            "New desc",
            "project",
            None,
            "Updated body",
            2000,
        )
        .unwrap();

        let list2 = db.list_memories(None, None, 10).unwrap();
        assert_eq!(list2.len(), 1);
        assert_eq!(list2[0].title, "Updated Title");
        assert_eq!(list2[0].content, "Updated body");
    }

    #[test]
    fn test_conversations_and_executions() {
        let db = Db::open_in_memory().unwrap();
        let inserted = db
            .insert_conversation(
                "sess-1",
                "test-project",
                "user",
                Some("uuid-1"),
                None,
                1000,
                None,
                "Hello Claude",
            )
            .unwrap();
        assert!(inserted);

        // Duplicate uuid is ignored
        let inserted2 = db
            .insert_conversation(
                "sess-1",
                "test-project",
                "user",
                Some("uuid-1"),
                None,
                1000,
                None,
                "Hello Claude Again",
            )
            .unwrap();
        assert!(!inserted2);

        let convs = db.list_conversations(Some("sess-1"), None, None, 10).unwrap();
        assert_eq!(convs.len(), 1);
        assert_eq!(convs[0].content, "Hello Claude");

        // Execution
        db.upsert_execution(
            "sess-1",
            "test-project",
            "tool-1",
            "Bash",
            r#"{"command":"cargo test"}"#,
            "cargo test",
            "running",
            "",
            1100,
        )
        .unwrap();

        db.update_execution_result("tool-1", "success", "500 tests passed").unwrap();

        let execs = db.list_executions(Some("sess-1"), None, None, 10).unwrap();
        assert_eq!(execs.len(), 1);
        assert_eq!(execs[0].status, "success");
        assert_eq!(execs[0].result_snippet, "500 tests passed");

        // Re-upserting execution with "running" should NOT overwrite existing "success"
        db.upsert_execution(
            "sess-1",
            "test-project",
            "tool-1",
            "Bash",
            r#"{"command":"cargo test"}"#,
            "cargo test",
            "running",
            "",
            1100,
        )
        .unwrap();

        let execs2 = db.list_executions(Some("sess-1"), None, None, 10).unwrap();
        assert_eq!(execs2[0].status, "success");
        assert_eq!(execs2[0].result_snippet, "500 tests passed");
    }

    #[test]
    fn test_get_session_packet() {
        let db = Db::open_in_memory().unwrap();
        let sid = "aaaaaaaa-1111-2222-3333-444444444444";

        db.insert_conversation(
            sid,
            "project-x",
            "user",
            Some("aaaaaaaa-0001-0000-0000-000000000001"),
            None,
            1_700_000_000_000,
            None,
            "What is the meaning of life?",
        )
        .unwrap();

        db.insert_conversation(
            sid,
            "project-x",
            "assistant",
            Some("aaaaaaaa-0001-0000-0000-000000000002"),
            None,
            1_700_000_001_000,
            Some("claude-3-7-sonnet"),
            "42 is the answer.",
        )
        .unwrap();

        db.upsert_execution(
            sid,
            "project-x",
            "tool-x1",
            "Bash",
            r#"{"cmd":"echo 42"}"#,
            "echo 42",
            "success",
            "42\n",
            1_700_000_000_500,
        )
        .unwrap();

        db.upsert_observation(
            sid,
            "project-x",
            "discovery",
            "Found answer",
            "Key takeaway",
            "Discovered the number 42",
            r#"["42 is deep"]"#,
            r#"["universe"]"#,
            r#"["src/life.rs"]"#,
            r#"[]"#,
            "native",
            1_700_000_002,
        )
        .unwrap();

        // Exact match
        let pkt = db.get_session_packet(sid).unwrap().expect("packet should exist");
        assert_eq!(pkt.session_id, sid);
        assert_eq!(pkt.project_name, "project-x");
        assert_eq!(pkt.conversations.len(), 2);
        assert_eq!(pkt.executions.len(), 1);
        assert_eq!(pkt.observations.len(), 1);
        assert_eq!(pkt.stats.user_message_count, 1);
        assert_eq!(pkt.stats.assistant_message_count, 1);
        assert_eq!(pkt.stats.execution_count, 1);

        // Prefix match with first 8 characters
        let pkt_prefix = db.get_session_packet(&sid[..8]).unwrap().expect("prefix match should succeed");
        assert_eq!(pkt_prefix.session_id, sid);

        // Markdown generation
        let md = pkt.to_markdown(Timezone::Kst);
        assert!(md.contains("# Session Memory Report"));
        assert!(md.contains("Found answer"));
        assert!(md.contains("What is the meaning of life?"));
        assert!(md.contains("42 is the answer."));
        assert!(md.contains("echo 42"));

        // Non-existent session
        let none_pkt = db.get_session_packet("nonexistent").unwrap();
        assert!(none_pkt.is_none());

        // Recent session list
        let recent = db.list_recent_session_ids(5).unwrap();
        assert!(!recent.is_empty());
        assert_eq!(recent[0].0, sid);
        assert_eq!(recent[0].1, "project-x");
    }
}
