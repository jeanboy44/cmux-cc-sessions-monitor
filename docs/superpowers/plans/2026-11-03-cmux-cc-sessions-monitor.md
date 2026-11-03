# cmux-cc-sessions-monitor Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** cmux 안에서 도는 모든 Claude Code 세션을 도트 마스코트 타일로 한 화면에 보여주고, vim 방식으로 이동해 Enter로 해당 워크스페이스·surface에 포커스를 옮기는 Rust TUI를 만든다.

**Architecture:** 데이터 소스(Claude 세션 파일, cmux hook 세션 파일, cmux CLI)를 각각 격리된 모듈로 읽고, `merge`가 하나의 `Session` 목록으로 합친다. 상태 판정은 우선순위 체인으로 한다. PR 조회는 백그라운드 스레드가 캐시를 채우고 UI는 캐시만 읽는다. UI는 `ratatui` 위에서 뷰 A/B를 같은 `Session` 목록으로 그린다.

**Tech Stack:** Rust(2021), ratatui, crossterm, serde/serde_json, toml, notify(실패 시 폴링), anyhow, dirs. macOS 전용.

**Spec:** `docs/superpowers/specs/2026-11-03-cmux-cc-sessions-monitor-design.md`

## Global Constraints

- cmux 0.65.1 기준. README에 "cmux 0.65 기준"을 명시한다.
- macOS 전용. 배포는 `cargo install --git`(소스 빌드)로 시작한다.
- 읽기와 포커스 이동만 한다. 세션에 입력을 보내지 않는다.
- cmux 밖에서 도는 Claude 세션(workspaceId/surfaceId 없음)은 목록에서 제외한다.
- 상태는 `Running`, `NeedsInput`, `Idle`, `Done` 네 가지와 판정 불가용 `Unknown`. 파일 형식은 느슨하게 파싱하고 모르는 필드·값이 있어도 죽지 않는다.
- 상태 판정 우선순위(스파이크 결과로 확정, `docs/spike-notes.md` 참고): (1) `~/.claude/sessions/<pid>.json`의 `status`(`busy`/`waiting`/`idle`, 즉시 반영됨, `waiting`이 입력 대기), (2) `~/.cmuxterm/claude-hook-sessions.json`의 `agentLifecycle`/`hookEventName`(입력 대기 기록이 약 6초 늦고 승인 후에도 `Stop`까지 낡은 `needsInput`이 남을 수 있어 보조 신호), (3) `cmux read-screen` 화면 패턴. `cmux list-status`는 워크스페이스 단위라 세션별 판정에 쓰지 않는다. 종료 후 세션 파일과 hook 항목은 약 1초 안에 사라진다.
- `Idle`은 살아 있고 프롬프트를 기다리는 상태, `Done`은 프로세스 종료. 종료된 세션은 기본 10분(설정 `done_retention_minutes`) 흐리게 유지 후 제거. 생존 확인은 `kill -0`.
- PR 조회: remote `origin`만, 열린 PR만, CI 결과 제외. Azure DevOps(`az repos pr list --source-branch`)를 먼저, GitHub(`gh pr list --head`)은 그다음. organization/project/repo는 remote URL 파싱. 이미 로그인된 `gh`/`az` 세션만 사용하고 토큰을 저장하지 않는다. 조회 간격 기본 45초, 화면 그리기는 조회를 절대 기다리지 않는다.
- 세션-브랜치 매칭: 세션 프로세스 cwd(`lsof`)를 `git worktree list`와 대조. 세션 cwd가 메인 저장소이면 하위 프로세스 cwd와 transcript 최근 도구 호출 경로로 추정하고 타일에 `~`를 붙인다. 추정은 설정으로 끌 수 있다. cmux `sidebar-state`의 `git_branch`는 쓰지 않는다.
- 설정 파일 `~/.config/cmux-cc-monitor/config.toml`. 항목: `view`(`a`|`b`|`auto`), 부가정보 on/off, `pr_poll_seconds`, `guess_worktree`, `done_retention_minutes`.
- v1 부가정보: 세션 이름, 상태, 한 줄 요약, git 브랜치, 경과시간, 마지막 활동 시각, PR 상태. 도구 호출·토큰·CPU/메모리는 v2.
- 마스코트는 Claude Code 마스코트를 복제하지 않은 독자 도트 캐릭터. 상태별 표정: Running 활기, NeedsInput 노랑+느낌표 깜빡임, Idle 졸기, Done 초록+체크.
- 디자인은 게임풍(사용자 요청, 스펙 "화면" 참고). 전체 UI가 대상: 상단 HUD(상태별 아이콘+숫자), 타일은 캐릭터 카드, 선택 커서는 깜빡이는 `▶`와 두꺼운 테두리, 설정 창은 게임 메뉴 창. 애니메이션은 12fps 단위, 프레임은 틱 번호만으로 결정되는 순수 함수(상태를 저장하지 않아 루프가 이음새 없이 반복됨). 참고: https://patch-gg.github.io/pixelfx/ (방식만 참고, 코드는 가져오지 않는다).
- 키: `h j k l`/방향키 이동, `Enter` 포커스, `i` 설정 창, `q` 종료.
- GitHub 공개 저장소: 코드·문서·테스트 픽스처에 개인 경로, 이메일, 사내 URL을 넣지 않는다. 저장소 생성·push는 사용자가 말하기 전에는 하지 않는다.
- 30초 이상 걸리는 명령(전체 빌드, 전체 테스트)은 Bash로 직접 돌리지 않고 cmux 가이드 스킬대로 cmux 별도 패인에서 실행하고 로그는 `tee /tmp/<name>.log`로 남긴다.

## Review Focus

- 깨졌거나 비어 있는 세션 JSON, 알 수 없는 `status`·`agentLifecycle` 값 → 크래시 없이 `Unknown`으로 표시(Task 2, 4).
- 세션이 0개 → 빈 화면 대신 안내 문구, 키 입력이 패닉하지 않음(Task 8).
- 아주 작은 터미널(예: 20x6)과 한글 등 전각 문자가 들어간 세션 이름 → 타일이 깨지거나 패닉하지 않고 잘림 처리(Task 7).
- `cmux`, `gh`, `az`가 없거나 로그인 안 됨, 응답이 느림 → TUI는 멈추지 않고 해당 기능만 비활성 표시(Task 3, 6).
- pid가 재사용되었거나 프로세스가 사라진 세션 → 새 세션으로 오인하지 않고 `Done`으로 처리 후 보존 시간 뒤 제거(Task 4).

---

### Task 0: 관찰 스파이크 (입력 대기 신호)

가장 큰 기술 위험이다. 이 태스크의 결과로 Task 4의 판정 규칙이 확정된다. 코드를 만들지 않는 수동 관찰이며 결과는 문서로 남긴다.

**Files:**
- Create: `docs/spike-notes.md`

**Interfaces:**
- Produces: `docs/spike-notes.md`에 아래 표를 채운다. Task 4가 이 표를 규칙의 근거로 읽는다. 열: 상황, `agentLifecycle`, `hookEventName`, `~/.claude/sessions` `status`, `cmux list-status` work 값, `read-screen`에서 쓸 수 있는 문구.

- [ ] **Step 1: cmux 안에서 `claude`를 새로 띄우고(래퍼가 hook을 주입한다) 다음 네 상황을 만든다** — 작업 중, 권한 승인 프롬프트 대기, 프롬프트 대기(idle), 세션 종료. 각 상황에서 `cat ~/.cmuxterm/claude-hook-sessions.json`(해당 sessionId 항목), `cat ~/.claude/sessions/<pid>.json`, `cmux list-status`, `cmux read-screen`을 1초 간격으로 캡처해 상태 전이를 기록한다.
- [ ] **Step 2: `docs/spike-notes.md`에 위 표와 결론을 쓴다** — `needsInput`이 실제로 기록되는지, 안 되면 `hookEventName`의 어떤 값이 대용인지, `read-screen`이 필요한 경우의 문구. 개인 경로·세션 ID는 제거한다.
- [ ] **Step 3: 결론에 따라 Global Constraints의 상태 판정 우선순위가 바뀌어야 하면 사용자에게 알리고 이 계획을 고친다**

---

### Task 1: 프로젝트 골격, 모델, 설정

**Files:**
- Create: `Cargo.toml`, `.gitignore`, `src/main.rs`, `src/model.rs`, `src/config.rs`
- Test: `src/config.rs`(모듈 내 `#[cfg(test)]`)

**Interfaces:**
- Produces (`src/model.rs`):
  - `enum Status { Running, NeedsInput, Idle, Done, Unknown }`
  - `struct Pr { number: u64, title: String, state: PrState }`, `enum PrState { Draft, ReviewPending, Approved, Conflict, Unknown }`
  - `struct Session { id: String, name: String, status: Status, pid: u32, cwd: PathBuf, workspace_id: String, surface_id: String, started_at: Option<SystemTime>, updated_at: Option<SystemTime>, summary: Option<String>, branch: Option<String>, branch_guessed: bool, pr: Option<Pr> }`
- Produces (`src/config.rs`):
  - `enum ViewMode { A, B, Auto }`
  - `struct Config { view: ViewMode, extras: Extras, pr_poll_seconds: u64, guess_worktree: bool, done_retention_minutes: u64 }`, `struct Extras { branch, elapsed, last_activity, summary, pr: bool }`
  - `Config::default() -> Config`(기본 켜짐: summary, branch, elapsed. 꺼짐: last_activity, pr), `Config::load(path: &Path) -> Config`(파일 없음·파싱 실패 시 기본값), `Config::save(&self, path: &Path) -> anyhow::Result<()>`, `fn default_path() -> PathBuf`(`~/.config/cmux-cc-monitor/config.toml`)

- [ ] **Step 1: 실패하는 테스트 작성** — `config_defaults_match_spec`(view=Auto, pr 꺼짐, summary·branch·elapsed 켜짐, `pr_poll_seconds==45`, `done_retention_minutes==10`), `config_roundtrip`(save 후 load가 같은 값), `config_load_missing_file_returns_default`, `config_load_garbage_returns_default`(`"{{{"` 파일).
- [ ] **Step 2: `cargo test config` 실행해 실패 확인**
- [ ] **Step 3: `git init`, `Cargo.toml`(의존성: ratatui, crossterm, serde(derive), serde_json, toml, anyhow, dirs, notify), `.gitignore`(`target/`, `.superpowers/`), 위 Interfaces의 타입과 함수 구현** — `src/main.rs`는 빈 `fn main() {}`.
- [ ] **Step 4: `cargo test config` 통과 확인**
- [ ] **Step 5: Commit** — `git add -A && git commit -m "chore: scaffold crate with model and config"`

---

### Task 2: 세션 파일 소스 (Claude, cmux hook)

**Files:**
- Create: `src/source/mod.rs`, `src/source/claude_sessions.rs`, `src/source/hook_sessions.rs`, `tests/fixtures/claude_session_busy.json`, `tests/fixtures/hook_sessions.json`
- Test: 각 파일 모듈 내 테스트

**Interfaces:**
- Produces:
  - `struct ClaudeSessionRecord { pid: u32, session_id: String, cwd: PathBuf, name: Option<String>, status: Option<String>, started_at_ms: Option<u64>, updated_at_ms: Option<u64> }`
  - `fn read_claude_sessions(dir: &Path) -> Vec<ClaudeSessionRecord>` — `dir` 아래 `*.json` 중 파싱되는 것만. 깨진 파일은 건너뛴다.
  - `struct HookRecord { session_id: String, pid: Option<u32>, workspace_id: String, surface_id: String, cwd: PathBuf, agent_lifecycle: Option<String>, hook_event_name: Option<String>, last_body: Option<String>, transcript_path: Option<PathBuf>, updated_at: Option<String> }`
  - `fn read_hook_sessions(path: &Path) -> Vec<HookRecord>` — 최상위 `sessions` 맵의 값들. 파일이 없거나 깨지면 빈 벡터.
- 픽스처는 개인 경로·실제 sessionId 없이 `/home/user/proj`, `00000000-...` 같은 가짜 값으로 만든다.

- [ ] **Step 1: 실패하는 테스트 작성** — `claude_sessions_parses_fixture`(status=="busy", pid 일치), `claude_sessions_skips_broken_file`(디렉터리에 `{`만 있는 파일을 같이 둠 → 나머지는 정상 반환), `claude_sessions_ignores_unknown_fields`, `hook_sessions_parses_fixture`(`agent_lifecycle==Some("running")`), `hook_sessions_missing_file_is_empty`, `hook_sessions_missing_optional_fields_ok`.
- [ ] **Step 2: `cargo test source` 실행해 실패 확인**
- [ ] **Step 3: 위 두 함수 구현** — serde 구조체는 모든 선택 필드에 `#[serde(default)]`를 두고 `deny_unknown_fields`를 쓰지 않는다. camelCase 필드명은 `#[serde(rename_all = "camelCase")]`.
- [ ] **Step 4: 통과 확인**
- [ ] **Step 5: Commit** — `git commit -m "feat: tolerant readers for claude and cmux hook session files"`

---

### Task 3: cmux CLI 래퍼

**Files:**
- Create: `src/source/cmux.rs`, `tests/fixtures/cmux_sessions_list.txt`
- Test: 모듈 내 테스트

**Interfaces:**
- Produces:
  - `struct CmuxSessionRef { session_id: String, workspace_id: String, surface_id: String, cwd: PathBuf, pid_exists: bool }`
  - `fn parse_sessions_list(output: &str) -> Vec<CmuxSessionRef>` — `cmux sessions list` 한 줄 형식(`claude <id> workspace=<uuid> surface=<uuid> cwd=<path> ... pid_exists=yes`)을 파싱. 모르는 줄은 건너뛴다.
  - `trait Cmux { fn sessions(&self) -> anyhow::Result<Vec<CmuxSessionRef>>; fn focus(&self, workspace_id: &str, surface_id: &str) -> anyhow::Result<()>; fn read_screen(&self, surface_id: &str) -> anyhow::Result<String>; }`
  - `struct CmuxCli;` — `Cmux`의 실제 구현. 각 호출에 2초 타임아웃. 바이너리가 없거나 실패하면 `Err`.
  - 테스트용 `struct FakeCmux`(호출 기록 + 미리 정한 응답)를 `#[cfg(test)]`로 제공.
- 포커스는 `cmux select-workspace --workspace <ws>` 다음 `cmux focus-panel --panel <surface> --workspace <ws>` 순서로 구현한다(스파이크에서 검증됨). 참고로 `cmux send-key`는 대화상자가 떠 있으면 `--force`가 필요하지만 우리는 키를 보내지 않는다.

- [ ] **Step 1: 실패하는 테스트 작성** — `parse_sessions_list_fixture`(3줄 → 3개, `pid_exists` 값 정확), `parse_ignores_garbage_lines`, `parse_handles_empty_output`.
- [ ] **Step 2: `cargo test cmux` 실행해 실패 확인**
- [ ] **Step 3: `parse_sessions_list`, `Cmux` 트레이트, `CmuxCli`, `FakeCmux` 구현** — 타임아웃은 자식 프로세스를 스레드에서 기다리고 `recv_timeout`으로 처리, 초과 시 kill.
- [ ] **Step 4: 통과 확인**
- [ ] **Step 5: 수동 확인** — cmux 안에서 `cargo run --example probe`(임시, 커밋하지 않음)로 `CmuxCli.sessions()`가 실제 세션을 돌려주는지 보고, `focus`가 다른 워크스페이스로 이동하는지 확인. 임시 파일은 삭제.
- [ ] **Step 6: Commit** — `git commit -m "feat: cmux cli wrapper with timeouts"`

---

### Task 4: 상태 판정과 세션 병합

**Files:**
- Create: `src/status.rs`, `src/merge.rs`
- Test: 각 모듈 내 테스트

**Interfaces:**
- Consumes: Task 1의 `Session`·`Status`, Task 2의 `ClaudeSessionRecord`·`HookRecord`, Task 3의 `Cmux`·`CmuxSessionRef`.
- Produces:
  - `fn resolve_status(hook: Option<&HookRecord>, claude: Option<&ClaudeSessionRecord>, alive: bool, screen: Option<&str>) -> Status` — `alive==false`면 `Done`. 그다음 우선순위 (1) claude status(`waiting`→NeedsInput, `busy`/`shell`→Running, `idle`→Idle) (2) hook(claude status가 없거나 알 수 없을 때만; `needsInput`은 `hookEventName`이 `Notification`이고 claude status가 `busy`로 돌아오지 않은 경우만 신뢰) (3) 화면 패턴. 규칙의 구체적 값은 `docs/spike-notes.md` 표를 따른다. 어떤 신호도 해석되지 않으면 `Unknown`.
  - `fn merge_sessions(claude: &[ClaudeSessionRecord], hooks: &[HookRecord], cmux: &[CmuxSessionRef], now: SystemTime, retention: Duration, prev: &[Session]) -> Vec<Session>` — sessionId로 조인. workspace/surface가 없는 세션은 제외. 종료된 세션은 `retention` 동안 유지 후 제거(이전 목록 `prev`에서 종료 시각 추적). pid 재사용 방지를 위해 같은 pid여도 sessionId가 다르면 별개 세션. 결과는 시작 시각 오름차순 정렬. `summary`는 hook의 `last_body`(없으면 `None`, 첫 줄만 사용), `name`은 Claude 세션 `name`(없으면 cwd의 마지막 디렉터리명), `updated_at`은 두 소스 중 더 최근 값으로 채운다. `merge_fills_summary_and_name_fallback` 테스트를 추가한다.

- [ ] **Step 1: 실패하는 테스트 작성** — `status_dead_pid_is_done`, `status_claude_waiting_is_needs_input`, `status_stale_hook_needs_input_ignored_when_claude_busy`, `status_falls_back_to_claude_when_no_hook`, `status_unknown_values_become_unknown`(hook `agentLifecycle=="xyz"` + claude 없음), `status_screen_pattern_is_last_resort`, `merge_excludes_sessions_without_surface`, `merge_keeps_done_within_retention_then_drops`, `merge_pid_reuse_with_new_session_id_is_separate_session`.
- [ ] **Step 2: `cargo test status merge` 실행해 실패 확인**
- [ ] **Step 3: 두 함수 구현** — 판정 규칙은 `docs/spike-notes.md` 결론을 그대로 코드로 옮긴다. 규칙마다 위 테스트가 하나씩 대응해야 한다.
- [ ] **Step 4: 통과 확인**
- [ ] **Step 5: Commit** — `git commit -m "feat: status resolution and session merge"`

---

### Task 5: 워크트리·브랜치 매칭

**Files:**
- Create: `src/worktree.rs`
- Test: 모듈 내 테스트(임시 git 저장소와 `git worktree add`로 실제 워크트리 생성)

**Interfaces:**
- Consumes: Task 1의 `Session`.
- Produces:
  - `struct Worktree { path: PathBuf, branch: Option<String>, is_main: bool }`
  - `fn list_worktrees(repo_dir: &Path) -> Vec<Worktree>` — `git -C <dir> worktree list --porcelain` 파싱. 실패 시 빈 벡터.
  - `fn match_branch(cwd: &Path, worktrees: &[Worktree]) -> Option<(String, bool /*is_main*/)>` — cwd가 속한 워크트리의 가장 긴 접두 경로를 찾는다.
  - `fn guess_worktree(paths: &[PathBuf], worktrees: &[Worktree]) -> Option<String>` — 주어진 경로들(가장 최근 것이 앞)에서 메인이 아닌 워크트리에 속한 첫 항목의 브랜치.
  - `fn recent_tool_paths(transcript: &Path, max_bytes: u64) -> Vec<PathBuf>` — transcript 끝 `max_bytes`만 읽어 도구 호출 입력의 경로 문자열을 최신순으로 반환. 깨진 줄은 건너뛴다.
  - `fn child_cwds(pid: u32) -> Vec<PathBuf>` — `pgrep -P`와 `lsof`로 하위 프로세스 cwd.
  - `fn resolve_branch(session: &Session, guess: bool) -> (Option<String>, bool /*guessed*/)` — 기본은 `match_branch`. 메인 저장소 cwd이고 `guess`가 true일 때만 `child_cwds`, `recent_tool_paths`로 `guess_worktree`를 시도하고 성공하면 `guessed=true`.

- [ ] **Step 1: 실패하는 테스트 작성** — `match_branch_in_linked_worktree`, `match_branch_in_subdir_of_worktree`, `match_branch_main_checkout_is_main`, `guess_prefers_most_recent_non_main_path`, `guess_none_when_all_paths_in_main`, `recent_tool_paths_skips_broken_lines`, `resolve_branch_no_guess_when_disabled`, `list_worktrees_non_git_dir_is_empty`.
- [ ] **Step 2: `cargo test worktree` 실행해 실패 확인**
- [ ] **Step 3: 위 함수들 구현**
- [ ] **Step 4: 통과 확인**
- [ ] **Step 5: Commit** — `git commit -m "feat: worktree and branch matching with guessed fallback"`

---

### Task 6: PR 조회와 백그라운드 캐시

**Files:**
- Create: `src/pr.rs`
- Test: 모듈 내 테스트(실제 `az`/`gh` 호출 없이 가짜 `PrProvider`)

**Interfaces:**
- Consumes: Task 1의 `Pr`, `PrState`.
- Produces:
  - `trait PrProvider: Send + Sync { fn find(&self, repo_dir: &Path, branch: &str) -> anyhow::Result<Option<Pr>>; }`
  - `fn detect_remote(repo_dir: &Path) -> Option<Remote>`, `enum Remote { AzureDevOps { org: String, project: String, repo: String }, GitHub { owner: String, repo: String } }`, `fn parse_remote_url(url: &str) -> Option<Remote>` — `https://dev.azure.com/<org>/<project>/_git/<repo>`, `git@ssh.dev.azure.com:v3/<org>/<project>/<repo>`, `https://github.com/<o>/<r>(.git)`, `git@github.com:<o>/<r>.git` 지원.
  - `struct AzProvider; struct GhProvider;` — `PrProvider` 구현. 외부 명령은 5초 타임아웃. 명령 없음·미로그인은 `Err`.
  - `struct PrCache` — `fn get(&self, repo_dir: &Path, branch: &str) -> PrLookup`, `enum PrLookup { Unknown, None, Found(Pr), Unavailable(String) }`. `fn spawn_poller(cache: Arc<PrCache>, targets: Arc<Mutex<Vec<(PathBuf, String)>>>, interval: Duration, providers: …) -> JoinHandle<()>` — 백그라운드에서 주기적으로 조회해 캐시를 채운다. `get`은 잠금만 짧게 잡고 즉시 반환.
  - PR 상태 매핑: Azure DevOps의 draft·reviewers vote·mergeStatus, GitHub의 `isDraft`·`reviewDecision`·`mergeable`을 `PrState`로 변환하는 순수 함수 `fn az_state(json: &serde_json::Value) -> PrState`, `fn gh_state(json: &serde_json::Value) -> PrState`.

- [ ] **Step 1: 실패하는 테스트 작성** — `parse_remote_url_azure_https`, `parse_remote_url_azure_ssh`, `parse_remote_url_github_https_and_ssh`, `parse_remote_url_unknown_is_none`, `az_state_draft`, `az_state_conflict`, `gh_state_approved`, `cache_get_is_nonblocking_while_provider_hangs`(provider가 3초 sleep하는 동안 `get`이 즉시 `Unknown` 반환), `cache_unavailable_when_provider_errors`.
- [ ] **Step 2: `cargo test pr` 실행해 실패 확인**
- [ ] **Step 3: 구현** — JSON은 `az repos pr list --source-branch <b> --status active --output json`, `gh pr list --head <b> --state open --json number,title,isDraft,reviewDecision,mergeable`의 출력. 필드 이름이 환경마다 다르면 `serde_json::Value`로 느슨하게 읽는다. 토큰은 읽거나 저장하지 않는다.
- [ ] **Step 4: 통과 확인**
- [ ] **Step 5: 수동 확인** — `az` 로그인 상태에서 실제 PR이 있는 브랜치로 `AzProvider.find`를 호출해 번호·상태가 맞는지 확인(임시 example, 커밋하지 않음).
- [ ] **Step 6: Commit** — `git commit -m "feat: pr providers and non-blocking cache"`

---

### Task 7: 마스코트와 타일 렌더링

**Files:**
- Create: `src/ui/mod.rs`, `src/ui/mascot.rs`, `src/ui/tile.rs`
- Test: 각 모듈 내 테스트(ratatui `TestBackend`)

**Interfaces:**
- Consumes: Task 1의 `Session`, `Status`, `Config`.
- Produces:
  - `fn sprite(status: Status, size: SpriteSize, tick: u64) -> Vec<Line<'static>>`, `enum SpriteSize { Small, Large }` — 하프블록(`▀▄█`)으로 그린 독자 도트 캐릭터. `tick`은 12fps 기준 틱 번호이고 프레임은 `tick`만으로 결정되는 순수 함수다(내부 상태 없음, 루프 길이로 나눈 나머지로 반복). Running은 4프레임 이상의 걷기/작업 루프, NeedsInput은 예비 동작 후 점프하며 느낌표가 깜빡이는 루프, Idle은 숨쉬기와 졸기, Done은 한 번 재생되는 "클리어" 포즈 후 마지막 프레임 유지(`fn sprite_oneshot_done(tick_since_done: u64)`로 구분하고 이후 고정).
  - `fn status_transition_fx(from: Status, to: Status, ticks_since_change: u64) -> TileFx`, `struct TileFx { shake: (i8, i8), flash: bool }` — 상태가 바뀐 직후 약 0.3초 동안 타일을 흔들고 번쩍이는 연출(NeedsInput 진입 시 가장 강함). 시간이 지나면 `TileFx::default()`.
  - `fn hud_gauge(value: f32, width: u16) -> String` — HP바 같은 블록 게이지(`█▓░`), 값은 0~1로 자르고 폭이 0이어도 패닉하지 않는다. 상태별 색: Running 주황+파란 불꽃, NeedsInput 노랑+느낌표, Idle 흐린 색+졸기, Done 초록+체크, Unknown 회색 물음표.
  - `fn render_tile(f: &mut Frame, area: Rect, s: &Session, cfg: &Config, selected: bool, tick: u64, fx: TileFx, view: ViewKind)`, `enum ViewKind { A, B }` — A는 마스코트 좌측, B는 상단. 선택 타일은 노란 테두리. 이름은 `unicode-width`로 폭을 계산해 잘라낸다. 추정 브랜치는 `~` 접두. `Done`은 흐리게. 켜진 부가정보만 표시.
  - `fn tile_size(view: ViewKind) -> (u16 /*w*/, u16 /*h*/)` — 열 수 계산에 쓰는 최소 타일 크기.
- 마스코트 도안은 공식 마스코트와 다른 실루엣으로 새로 그린다.

- [ ] **Step 1: 실패하는 테스트 작성** — `tile_view_a_shows_name_and_status`, `tile_view_b_shows_mascot_above_text`, `tile_truncates_wide_korean_name_without_panic`, `tile_tiny_area_does_not_panic`(20x6, 5x2, 0x0), `tile_hides_disabled_extras`, `tile_marks_guessed_branch_with_tilde`, `sprite_each_status_differs`(다섯 상태의 렌더 결과가 서로 다름), `sprite_needs_input_frames_differ`, `sprite_is_pure_function_of_tick`(같은 tick이면 항상 같은 결과, 루프 길이만큼 지나면 처음과 같음), `sprite_done_oneshot_holds_last_frame`, `transition_fx_decays_to_default_after_about_0_3s`, `hud_gauge_clamps_and_zero_width_ok`, `selected_tile_shows_blinking_cursor_arrow`(tick에 따라 `▶` 표시 여부가 바뀜). 상단 HUD(`render_hud(f, area, counts)`)가 상태별 개수를 아이콘+숫자로 그리는지 `hud_shows_counts_per_status`로 확인한다. 타일 테두리는 두꺼운 도트풍 문자로 그린다. 설정 창(Task 9)도 같은 테두리·커서 스타일(게임 메뉴 창)로 맞춘다.
- [ ] **Step 2: `cargo test ui` 실행해 실패 확인**
- [ ] **Step 3: 구현** — `Cargo.toml`에 `unicode-width` 추가. 영역이 타일 최소 크기보다 작으면 이름 한 줄만 그린다.
- [ ] **Step 4: 통과 확인**
- [ ] **Step 5: Commit** — `git commit -m "feat: dot mascot sprites and tile rendering"`

---

### Task 8: 앱 상태, 내비게이션, 메인 루프

**Files:**
- Create: `src/app.rs`, `src/ui/grid.rs`
- Modify: `src/main.rs`, `src/ui/mod.rs`
- Test: `src/app.rs`, `src/ui/grid.rs` 내 테스트

**Interfaces:**
- Consumes: Task 3 `Cmux`, Task 4 `merge_sessions`, Task 5 `resolve_branch`, Task 6 `PrCache`, Task 7 `render_tile`.
- Produces:
  - `struct App { sessions: Vec<Session>, selected: usize, cols: usize, view: ViewKind, config: Config, … }`
  - `enum Action { Move(Dir), Focus, OpenSettings, Quit, None }`, `enum Dir { Left, Down, Up, Right }`, `fn key_to_action(key: KeyEvent) -> Action` — `h j k l`와 방향키, Enter, `i`, `q`.
  - `fn App::apply_move(&mut self, dir: Dir)` — 격자(`cols`) 기준 이동, 가장자리에서 멈춤. 세션이 0개여도 패닉 없음. 세션 목록이 줄어 `selected`가 범위를 벗어나면 보정.
  - `fn App::focus_selected(&self, cmux: &dyn Cmux) -> anyhow::Result<()>` — 선택 세션의 `workspace_id`, `surface_id`로 `cmux.focus`.
  - `fn compute_cols(width: u16, view: ViewKind) -> usize`, `fn resolve_view(cfg_view: ViewMode, session_count: usize) -> ViewKind` — `Auto`는 세션이 7개 이상이면 A, 아니면 B(경계값은 설정으로 바꾸지 않는다).
  - `main.rs` — 터미널 초기화·복구(패닉 시에도 복구하는 훅 포함), 1초 주기로 데이터 소스를 새로 읽는 갱신 스레드(파일 감시는 `notify`, 실패하면 폴링), 12fps 틱 카운터(애니메이션은 이 `tick`만 넘긴다)와 상태 변화 시각 추적(세션별 `ticks_since_change`로 `status_transition_fx` 계산), 이벤트 루프. 세션이 없으면 "cmux에서 실행 중인 Claude 세션이 없습니다" 안내를 그린다. 상단 바에 세션 수와 상태별 개수, 하단에 키 도움말.

- [ ] **Step 1: 실패하는 테스트 작성** — `move_right_then_left_returns`, `move_stops_at_edges`, `move_down_in_grid_uses_cols`, `move_with_zero_sessions_does_not_panic`, `selected_clamped_when_sessions_shrink`, `key_to_action_vim_and_arrows`, `focus_selected_calls_cmux_with_ids`(`FakeCmux`로 호출 인자 검증), `focus_error_is_returned_not_panicked`, `resolve_view_auto_thresholds`, `compute_cols_never_zero`.
- [ ] **Step 2: `cargo test app grid` 실행해 실패 확인**
- [ ] **Step 3: 구현** — `main.rs`는 얇게 유지하고 로직은 `app.rs`에 둔다.
- [ ] **Step 4: 통과 확인**
- [ ] **Step 5: 수동 확인** — cmux 새 패인에서 `cargo run`을 실행(cmux 가이드대로 별도 패인, 로그 `tee /tmp/ccmon-run.log`). 실제 세션이 타일로 뜨고 hjkl 이동, Enter 포커스, `q` 종료 후 터미널이 정상 복구되는지 확인.
- [ ] **Step 6: Commit** — `git commit -m "feat: app loop, navigation and cmux focus"`

---

### Task 9: 설정 창과 설정 저장

**Files:**
- Create: `src/ui/settings.rs`
- Modify: `src/app.rs`, `src/ui/mod.rs`
- Test: `src/ui/settings.rs` 내 테스트

**Interfaces:**
- Consumes: Task 1 `Config`, Task 8 `App`.
- Produces:
  - `enum SettingsItem { View, ExtraBranch, ExtraElapsed, ExtraLastActivity, ExtraSummary, ExtraPr, GuessWorktree }`
  - `fn toggle(cfg: &mut Config, item: SettingsItem)` — View는 `A → B → Auto` 순환, 나머지는 on/off.
  - `fn render_settings(f: &mut Frame, area: Rect, cfg: &Config, cursor: usize)`.
  - `App`은 설정 모드에서 `j`/`k`로 항목 이동, `Enter`나 Space로 토글, `Esc`나 `i`로 닫기. 변경 즉시 `Config::save`. 저장 실패는 하단에 오류 문구로 표시하고 앱은 계속 동작. PR 항목을 켜면 PR 폴러가 시작되고, `gh`/`az`가 없으면 항목 옆에 "사용 불가" 표시.

- [ ] **Step 1: 실패하는 테스트 작성** — `toggle_view_cycles_a_b_auto`, `toggle_extra_flips_flag`, `settings_changes_persist_to_file`(임시 경로에 저장 후 `Config::load`로 확인), `settings_save_failure_does_not_panic`(읽기 전용 경로), `settings_render_tiny_area_does_not_panic`.
- [ ] **Step 2: `cargo test settings` 실행해 실패 확인**
- [ ] **Step 3: 구현**
- [ ] **Step 4: 통과 확인**
- [ ] **Step 5: Commit** — `git commit -m "feat: settings screen with persistence"`

---

### Task 10: 공개 준비 (README, 라이선스, 정리)

**Files:**
- Create: `README.md`, `LICENSE`
- Modify: `Cargo.toml`(메타데이터), 필요 시 코드의 하드코딩된 개인 경로 제거

**Interfaces:**
- Produces: 공개 저장소로 올릴 수 있는 상태. (저장소 생성·push는 하지 않는다.)

- [ ] **Step 1: 개인 정보 점검 테스트** — `tests/no_personal_info.rs`: 저장소의 추적 대상 파일(`target/`, `.git/` 제외)에서 홈 디렉터리 절대경로, 사내 계정명, 개인 사용자명(Cargo.toml의 repository 항목과 LICENSE 저작권 표기 줄 제외)를 찾아 하나라도 있으면 실패.
- [ ] **Step 2: `cargo test --test no_personal_info` 실행** — 실패하면 해당 파일에서 제거하고 반복해 통과시킨다.
- [ ] **Step 3: `README.md` 작성** — 한 문단 소개, 스크린샷 자리, 설치(`cargo install --git`), 사용법(키), 설정 파일 항목, "cmux 0.65 기준·macOS 전용", 상태 판정 방식과 한계(추정 매칭 `~`), PR 연동은 선택 사항이며 이미 로그인된 `gh`/`az`를 쓴다는 점. 문단 내 강제 줄바꿈 없이 문단은 한 줄로 쓴다.
- [ ] **Step 4: `LICENSE`(MIT, 저작권 표기에 GitHub 사용자명), `Cargo.toml`에 `repository`(공개 저장소 URL), `license = "MIT"`, `description`**
- [ ] **Step 5: 전체 검증** — cmux 별도 패인에서 `cargo build --release`와 `cargo test`(로그 `tee /tmp/ccmon-final.log`), `cargo clippy -- -D warnings`. 모두 통과해야 한다.
- [ ] **Step 6: Commit** — `git commit -m "docs: readme, license and personal-info guard"`
