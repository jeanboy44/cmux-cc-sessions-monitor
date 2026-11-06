//! CLI query tool for Claude Code memories, conversations, and execution history.

use anyhow::{bail, Context, Result};

use super::collector::{default_projects_dir, sync_once};
use super::db::{default_db_path, Db};
use super::timezone::Timezone;

pub fn run_cli(args: &[String]) -> Result<()> {
    let db_path = default_db_path();
    let db = Db::open(&db_path)?;
    run_cli_with_db(&db, args)
}

fn extract_tz(args: &[String]) -> (Timezone, Vec<String>) {
    let mut tz = None;
    let mut rest = Vec::new();
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        if arg == "--tz" || arg == "--timezone" {
            if let Some(val) = it.next() {
                tz = Some(Timezone::parse(val));
            }
        } else if let Some(val) = arg.strip_prefix("--tz=") {
            tz = Some(Timezone::parse(val));
        } else if let Some(val) = arg.strip_prefix("--timezone=") {
            tz = Some(Timezone::parse(val));
        } else {
            rest.push(arg.clone());
        }
    }
    let tz = tz.unwrap_or_else(|| {
        let (cfg, _) = crate::config::Config::load_checked(&crate::config::default_path());
        Timezone::parse(&cfg.timezone)
    });
    (tz, rest)
}

pub fn run_cli_with_db(db: &Db, args: &[String]) -> Result<()> {
    let (tz, filtered) = extract_tz(args);
    let subcmd = filtered.get(0).map(|s| s.as_str()).unwrap_or("list");
    let rest = filtered.get(1..).unwrap_or(&[]);

    match subcmd {
        "list" | "ls" => cmd_list(db, rest, tz),
        "show" | "get" | "cat" => cmd_show(db, rest, tz),
        "search" | "find" => cmd_search(db, rest, tz),
        "timeline" | "time" => cmd_timeline(db, rest, tz),
        "obs" | "observations" => cmd_observations(db, rest, tz),
        "exec" | "executions" | "tools" => cmd_exec(db, rest, tz),
        "conv" | "conversations" | "chat" => cmd_conv(db, rest, tz),
        "session" | "dump" | "bundle" | "read" | "export" => cmd_session(db, rest, tz),
        "sync" => cmd_sync(db, rest),
        "stats" | "info" => cmd_stats(db),
        "-h" | "--help" | "help" => {
            print_help();
            Ok(())
        }
        other => {
            // If the argument doesn't match a subcommand, treat it as a search query
            if !other.starts_with('-') {
                cmd_search(db, &filtered, tz)
            } else {
                eprintln!("알 수 없는 하위 명령어: {other}\n");
                print_help();
                bail!("unknown subcommand");
            }
        }
    }
}

fn print_help() {
    println!(
        r#"cmux-magigent memory — Claude 메모리, 실행이력, 대화 기록 조회 도구

사용법:
  cmux-magigent memory [명령어] [옵션]

명령어:
  list [검색어]              저장된 메모리 목록 조회 (기본값)
  show <ID|제목|파일명>      메모리 상세 내용 출력
  search <검색어>            메모리, 대화, 실행이력 통합 검색
  timeline [옵션]            시간순 통합 타임라인 조회 (claude-mem 패턴)
  obs [옵션]                 구조화된 관찰(Observation) 항목 조회
  exec [옵션]                도구 실행 이력(Bash 명령어 등) 조회
  conv [옵션]                과거 대화 내역(사용자 질문/어시스턴트 답변) 조회
  session <세션ID> [옵션]    특정 세션의 모든 메모리/대화/실행이력 통합 번들 조회
  sync                       Claude 및 claude-mem 세션 데이터 즉시 동기화
  stats                      저장된 데이터 통계 및 DB 파일 경로 확인
  help                       도움말 출력

옵션:
  --project <이름>           특정 프로젝트로 필터링
  --limit <숫자>             출력 개수 제한 (기본값: 30)
  --session <ID>             특정 세션 ID로 필터링
  --tool <이름>              특정 도구(Bash, Edit, Read 등)로 필터링
  --json                     JSON 형식으로 출력
  -o, --output <파일경로>    출력 결과를 파일로 저장
  --tz <시간대>              표시 시간대 지정 (기본값: KST, 예: UTC, JST, EST, PST, +09:00)
"#
    );
}

fn cmd_list(db: &Db, args: &[String], tz: Timezone) -> Result<()> {
    let mut project = None;
    let mut query = None;
    let mut limit = 50;

    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--project" | "-p" => project = it.next().cloned(),
            "--limit" | "-n" => {
                if let Some(n) = it.next().and_then(|s| s.parse().ok()) {
                    limit = n;
                }
            }
            s if !s.starts_with('-') => query = Some(s.to_string()),
            _ => {}
        }
    }

    let mems = db.list_memories(project.as_deref(), query.as_deref(), limit)?;
    if mems.is_empty() {
        println!("저장된 메모리가 없습니다. ('cmux-magigent memory sync'로 동기화 가능)");
        return Ok(());
    }

    let header_time = format!("수정 시각 ({})", tz.as_code());
    println!("{:<4} | {:<20} | {:<30} | {:<16}", "ID", "프로젝트", "제목 / 파일명", header_time);
    println!("{:-<4}-+-{:-<20}-+-{:-<30}-+-{:-<16}", "", "", "", "");

    for m in mems {
        let display_title = if !m.title.is_empty() {
            m.title.clone()
        } else {
            m.file_name.clone()
        };
        let short_proj = truncate_str(&m.project_name, 20);
        let short_title = truncate_str(&display_title, 30);
        let updated = tz.format_iso_utc_short(&m.updated_at);
        println!("{:<4} | {:<20} | {:<30} | {:<16}", m.id, short_proj, short_title, updated);
    }

    Ok(())
}

fn cmd_show(db: &Db, args: &[String], tz: Timezone) -> Result<()> {
    let target = match args.get(0) {
        Some(t) => t,
        None => {
            eprintln!("메모리 ID 또는 제목을 입력해주세요. (예: cmux-magigent memory show 1)");
            bail!("missing target");
        }
    };

    let mem = if let Ok(id) = target.parse::<i64>() {
        db.get_memory(id)?
    } else {
        db.find_memory(target)?
    };

    match mem {
        Some(m) => {
            println!("============================================================");
            println!(" ID:           {}", m.id);
            println!(" 제목:         {}", m.title);
            println!(" 프로젝트:     {}", m.project_name);
            println!(" 파일명:       {}", m.file_name);
            println!(" 분류:         {}", m.memory_type);
            if let Some(origin) = m.origin_session_id {
                println!(" 세션 ID:      {}", origin);
            }
            if !m.description.is_empty() {
                println!(" 요약:         {}", m.description);
            }
            println!(" 최종 수정:    {} ({})", tz.format_iso_utc(&m.updated_at), tz.as_code());
            println!("============================================================");
            println!("\n{}", m.content);
        }
        None => {
            println!("일치하는 메모리를 찾을 수 없습니다: {target}");
        }
    }
    Ok(())
}

fn cmd_search(db: &Db, args: &[String], _tz: Timezone) -> Result<()> {
    let query = match args.get(0) {
        Some(q) => q,
        None => {
            eprintln!("검색어를 입력해주세요. (예: cmux-magigent memory search ui)");
            bail!("missing query");
        }
    };

    println!("=== [메모리 검색 결과: '{query}'] ===");
    let mems = db.list_memories(None, Some(query), 10)?;
    if mems.is_empty() {
        println!("(일치하는 메모리 없음)");
    } else {
        for m in mems {
            println!("• [메모리 #{}] [{}] {} - {}", m.id, m.project_name, m.title, truncate_str(&m.description, 50));
        }
    }

    println!("\n=== [대화 기록 검색 결과: '{query}'] ===");
    let convs = db.list_conversations(None, None, Some(query), 5)?;
    if convs.is_empty() {
        println!("(일치하는 대화 없음)");
    } else {
        for c in convs {
            let role_badge = if c.role == "user" { "USER" } else { "CLAUDE" };
            println!("• [{}] [{}] {}", role_badge, c.project_name, truncate_str(&c.content.replace('\n', " "), 70));
        }
    }

    println!("\n=== [실행 이력 검색 결과: '{query}'] ===");
    let execs = db.list_executions(None, None, None, 50)?;
    let matching_execs: Vec<_> = execs
        .into_iter()
        .filter(|e| e.command_hint.contains(query) || e.input_json.contains(query))
        .take(5)
        .collect();

    if matching_execs.is_empty() {
        println!("(일치하는 실행 이력 없음)");
    } else {
        for e in matching_execs {
            println!("• [{}] [{}] {} ({})", e.tool_name, e.project_name, truncate_str(&e.command_hint, 60), e.status);
        }
    }

    Ok(())
}

fn cmd_timeline(db: &Db, args: &[String], tz: Timezone) -> Result<()> {
    let mut project = None;
    let mut session = None;
    let mut limit = 30;

    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--project" | "-p" => project = it.next().cloned(),
            "--session" | "-s" => session = it.next().cloned(),
            "--limit" | "-n" => {
                if let Some(n) = it.next().and_then(|s| s.parse().ok()) {
                    limit = n;
                }
            }
            s if !s.starts_with('-') => project = Some(s.to_string()),
            _ => {}
        }
    }

    let items = db.get_timeline(project.as_deref(), session.as_deref(), limit)?;
    if items.is_empty() {
        println!("타임라인 항목이 없습니다.");
        return Ok(());
    }

    let header_time = format!("시각 ({})", tz.as_code());
    println!("{:<16} | {:<10} | {:<16} | {:<45}", header_time, "구분", "프로젝트", "내용");
    println!("{:-<16}-+-{:-<10}-+-{:-<16}-+-{:-<45}", "", "", "", "");

    for item in items {
        let time_str = tz.format_epoch_ms_short(item.timestamp_ms);
        let short_proj = truncate_str(&item.project, 16);
        let short_title = truncate_str(&item.title.replace('\n', " "), 45);
        println!("{:<16} | {:<10} | {:<16} | {:<45}", time_str, format!("[{}]", item.badge), short_proj, short_title);
    }
    Ok(())
}

fn cmd_observations(db: &Db, args: &[String], tz: Timezone) -> Result<()> {
    let mut project = None;
    let mut obs_type = None;
    let mut query = None;
    let mut limit = 30;

    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--project" | "-p" => project = it.next().cloned(),
            "--type" | "-t" => obs_type = it.next().cloned(),
            "--limit" | "-n" => {
                if let Some(n) = it.next().and_then(|s| s.parse().ok()) {
                    limit = n;
                }
            }
            s if !s.starts_with('-') => query = Some(s.to_string()),
            _ => {}
        }
    }

    let obss = db.list_observations(project.as_deref(), obs_type.as_deref(), query.as_deref(), limit)?;
    if obss.is_empty() {
        println!("관찰(Observation) 항목이 없습니다.");
        return Ok(());
    }

    let header_time = format!("기록 시각 ({})", tz.as_code());
    println!("{:<16} | {:<8} | {:<16} | {:<10} | {:<35}", header_time, "출처", "프로젝트", "유형", "제목");
    println!("{:-<16}-+-{:-<8}-+-{:-<16}-+-{:-<10}-+-{:-<35}", "", "", "", "", "");

    for obs in obss {
        let time_str = tz.format_epoch_ms_short(obs.created_at_epoch);
        let short_proj = truncate_str(&obs.project_name, 16);
        let short_title = truncate_str(&obs.title.replace('\n', " "), 35);
        println!("{:<16} | {:<8} | {:<16} | {:<10} | {:<35}", time_str, obs.source, short_proj, obs.obs_type, short_title);
    }
    Ok(())
}

fn cmd_exec(db: &Db, args: &[String], tz: Timezone) -> Result<()> {
    let mut session = None;
    let mut project = None;
    let mut tool = None;
    let mut limit = 30;

    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--session" | "-s" => session = it.next().cloned(),
            "--project" | "-p" => project = it.next().cloned(),
            "--tool" | "-t" => tool = it.next().cloned(),
            "--limit" | "-n" => {
                if let Some(n) = it.next().and_then(|s| s.parse().ok()) {
                    limit = n;
                }
            }
            _ => {}
        }
    }

    let execs = db.list_executions(session.as_deref(), project.as_deref(), tool.as_deref(), limit)?;
    if execs.is_empty() {
        println!("실행 이력이 없습니다.");
        return Ok(());
    }

    let header_time = format!("실행 시각 ({})", tz.as_code());
    println!("{:<16} | {:<10} | {:<16} | {:<8} | {:<35}", header_time, "도구", "프로젝트", "상태", "명령어 / 인자");
    println!("{:-<16}-+-{:-<10}-+-{:-<16}-+-{:-<8}-+-{:-<35}", "", "", "", "", "");

    for e in execs {
        let time_str = tz.format_epoch_ms_short(e.timestamp_ms);
        let short_proj = truncate_str(&e.project_name, 16);
        let short_hint = truncate_str(&e.command_hint.replace('\n', " "), 35);
        println!("{:<16} | {:<10} | {:<16} | {:<8} | {:<35}", time_str, e.tool_name, short_proj, e.status, short_hint);
    }
    Ok(())
}

fn cmd_conv(db: &Db, args: &[String], tz: Timezone) -> Result<()> {
    let mut session = None;
    let mut project = None;
    let mut query = None;
    let mut limit = 20;

    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--session" | "-s" => session = it.next().cloned(),
            "--project" | "-p" => project = it.next().cloned(),
            "--query" | "-q" => query = it.next().cloned(),
            "--limit" | "-n" => {
                if let Some(n) = it.next().and_then(|s| s.parse().ok()) {
                    limit = n;
                }
            }
            _ => {}
        }
    }

    let convs = db.list_conversations(session.as_deref(), project.as_deref(), query.as_deref(), limit)?;
    if convs.is_empty() {
        println!("대화 기록이 없습니다.");
        return Ok(());
    }

    for c in convs {
        let time_str = tz.format_epoch_ms_short(c.timestamp_ms);
        let role_str = if c.role == "user" { "👤 사용자" } else { "🤖 Claude" };
        let model_str = c.model.map(|m| format!(" ({m})")).unwrap_or_default();
        let short_text = truncate_str(&c.content.replace('\n', " "), 80);
        println!("[{}] [{}] [{}] {}{}", time_str, c.project_name, role_str, short_text, model_str);
    }
    Ok(())
}

fn cmd_session(db: &Db, args: &[String], tz: Timezone) -> Result<()> {
    let mut session_id_opt = None;
    let mut as_json = false;
    let mut output_file: Option<String> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--json" => {
                as_json = true;
                i += 1;
            }
            "-o" | "--output" => {
                if i + 1 < args.len() {
                    output_file = Some(args[i + 1].clone());
                    i += 2;
                } else {
                    bail!("--output 옵션 뒤에 저장할 파일 경로를 입력해주세요.");
                }
            }
            arg if !arg.starts_with('-') && session_id_opt.is_none() => {
                session_id_opt = Some(arg.to_string());
                i += 1;
            }
            _ => {
                i += 1;
            }
        }
    }

    let session_id = match session_id_opt {
        Some(s) => s,
        None => {
            eprintln!("조회할 세션 ID 또는 접두어(예: 앞 8자리)를 지정해주세요.\n");
            eprintln!("사용법: cmux-magigent memory session <SESSION_ID> [--json] [-o <PATH>]\n");
            if let Ok(recent) = db.list_recent_session_ids(5) {
                if !recent.is_empty() {
                    eprintln!("최근 세션 목록:");
                    for (sid, proj, ts) in recent {
                        let time_str = tz.format_epoch_ms(ts);
                        eprintln!("  • {sid}  ({proj}, {time_str} {})", tz.as_code());
                    }
                }
            }
            bail!("missing session id");
        }
    };

    let packet = match db.get_session_packet(&session_id)? {
        Some(p) => p,
        None => {
            bail!("세션 ID 또는 접두어 '{session_id}'에 해당하는 데이터를 찾을 수 없습니다.");
        }
    };

    let content = if as_json {
        serde_json::to_string_pretty(&packet)?
    } else {
        packet.to_markdown(tz)
    };

    if let Some(out_path) = output_file {
        std::fs::write(&out_path, &content)
            .with_context(|| format!("파일 쓰기 실패: {out_path}"))?;
        println!("💾 세션 메모리 번들이 저장되었습니다: {out_path}");
    } else {
        use std::io::Write;
        if let Err(e) = std::io::stdout().write_all(content.as_bytes()) {
            if e.kind() != std::io::ErrorKind::BrokenPipe {
                return Err(e.into());
            }
        }
    }

    Ok(())
}

fn cmd_sync(db: &Db, _args: &[String]) -> Result<()> {
    let pdir = default_projects_dir();
    print!("Claude 세션 디렉토리 ({}) 동기화 중… ", pdir.display());
    let updated = sync_once(db, &pdir)?;
    println!("완료! (업데이트된 항목 수: {updated})");
    cmd_stats(db)?;
    Ok(())
}

fn cmd_stats(db: &Db) -> Result<()> {
    let stats = db.get_stats()?;
    println!("=== Claude Code 로컬 저장소 현황 ===");
    println!("DB 경로:      {}", db.path.display());
    println!("프로젝트 수:  {}", stats.project_count);
    println!("저장된 메모리: {} 개", stats.memory_count);
    println!("관찰/요약:    {} 개 (claude-mem 연동)", stats.observation_count);
    println!("대화 기록:    {} 개", stats.conversation_count);
    println!("도구 실행:    {} 회", stats.execution_count);
    Ok(())
}

fn truncate_str(s: &str, max_len: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max_len {
        s.to_string()
    } else {
        let truncated: String = chars.iter().take(max_len.saturating_sub(1)).collect();
        format!("{truncated}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cli_commands() {
        let db = Db::open_in_memory().unwrap();
        db.upsert_memory(
            "my-project",
            "arch.md",
            "Architecture Spec",
            "Summary of architecture",
            "project",
            Some("00000000-0000-0000-0000-000000000001"),
            "Detailed content of architecture",
            1000,
        )
        .unwrap();

        db.insert_conversation(
            "sess-1",
            "my-project",
            "user",
            Some("00000000-0000-0000-0000-000000000002"),
            None,
            1000,
            None,
            "how to build the project",
        )
        .unwrap();

        db.upsert_execution(
            "sess-1",
            "my-project",
            "tool-1",
            "Bash",
            r#"{"command":"cargo build"}"#,
            "cargo build",
            "success",
            "finished in 2s",
            1050,
        )
        .unwrap();

        // 0. Empty args defaults to list without panic
        assert!(run_cli_with_db(&db, &[]).is_ok());

        // 1. List
        assert!(run_cli_with_db(&db, &["list".into()]).is_ok());

        // 2. Show by ID
        assert!(run_cli_with_db(&db, &["show".into(), "1".into()]).is_ok());

        // 3. Show by title
        assert!(run_cli_with_db(&db, &["show".into(), "Architecture Spec".into()]).is_ok());

        // 4. Search
        assert!(run_cli_with_db(&db, &["search".into(), "architecture".into()]).is_ok());

        // 5. Exec
        assert!(run_cli_with_db(&db, &["exec".into()]).is_ok());

        // 6. Conv
        assert!(run_cli_with_db(&db, &["conv".into()]).is_ok());

        // 7. Stats
        assert!(run_cli_with_db(&db, &["stats".into()]).is_ok());

        // 8. Help
        assert!(run_cli_with_db(&db, &["help".into()]).is_ok());

        // 9. With timezone flag
        assert!(run_cli_with_db(&db, &["list".into(), "--tz".into(), "UTC".into()]).is_ok());
        assert!(run_cli_with_db(&db, &["timeline".into(), "--tz=KST".into()]).is_ok());

        // 10. Session command (valid, invalid, --json, and file output)
        assert!(run_cli_with_db(&db, &["session".into(), "sess-1".into()]).is_ok());
        assert!(run_cli_with_db(&db, &["session".into(), "sess-1".into(), "--json".into()]).is_ok());
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let tmp_path = tmp.path().to_str().unwrap().to_string();
        assert!(run_cli_with_db(&db, &["session".into(), "sess-1".into(), "-o".into(), tmp_path.clone()]).is_ok());
        let saved = std::fs::read_to_string(&tmp_path).unwrap();
        assert!(saved.contains("# Session Memory Report"));
        assert!(run_cli_with_db(&db, &["session".into(), "nonexistent".into()]).is_err());
        assert!(run_cli_with_db(&db, &["session".into()]).is_err());
    }
}
