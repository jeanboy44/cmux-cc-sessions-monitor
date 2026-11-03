# 관찰 스파이크: 입력 대기 신호

cmux 안에서 `claude`(v2.1.x)를 새로 띄우고, 기본 권한 모드(manual)에서 권한 승인이 필요한 요청(`touch x.txt`를 Bash 도구로 실행)을 보내 상태 전이를 약 1초 간격으로 기록했다. 관찰 대상은 `~/.cmuxterm/claude-hook-sessions.json`, `~/.claude/sessions/<pid>.json`, `cmux list-status`, `cmux read-screen` 네 가지다. 세션 ID와 경로는 모두 placeholder로 바꿨다. 관찰은 1회 실행 결과이므로 수치는 모두 근사값이다.

## 관찰 결과 표

`cmux list-status` 열은 워크스페이스 단위 값이라 세션별로 신뢰할 수 없다(같은 워크스페이스의 다른 Claude 세션 값이 섞인다). 판정에 쓰지 않는다.

| 상황 | `agentLifecycle` | `hookEventName` | `~/.claude/sessions` `status` | `cmux list-status` work 값 (세션별로 신뢰할 수 없음, 워크스페이스 단위) | `read-screen`에서 쓸 수 있는 문구 |
|---|---|---|---|---|---|
| 세션 시작 직후 (프롬프트 대기) | `idle` | `SessionStart` | `idle` | 신뢰 불가 (`waiting` 관찰) | `❯ Try "how do I log an error?"` 입력창, 하단 `manual mode on` |
| 작업 중 (프롬프트 제출 직후) | `running` | `UserPromptSubmit` | `busy` | 신뢰 불가 (`waiting`에서 `running`으로 변함) | 입력창이 비고 도구 호출 진행 표시 |
| 권한 승인 프롬프트 대기 (표시 후 수 초간) | `running` | `PreToolUse` | `waiting` | 신뢰 불가 (`waiting`/`running` 혼재) | `Do you want to proceed?` 와 `1. Yes` ... `4. No`, `Esc to cancel` |
| 권한 승인 프롬프트 대기 (수 초 이후) | `needsInput` | `Notification` | `waiting` | 신뢰 불가 (`waiting`/`running` 혼재) | 위와 동일 |
| 승인 직후 (도구 실행 중) | `needsInput` (잔존) | `Notification` (잔존) | `busy` | 신뢰 불가 (`running` 관찰) | 승인 프롬프트 사라짐 |
| 작업 완료 후 프롬프트 대기 (idle) | `idle` | `Stop` | `idle` | 신뢰 불가 (`waiting` 관찰, 상위 세션 오염 가능) | 빈 입력창 `❯` 와 하단 상태줄 |
| 세션 종료 (`/exit`) | 항목 제거 | 항목 제거 | 파일 삭제 | 신뢰 불가 | 셸 프롬프트로 복귀, `Resume this session with:` 출력 |

`runtimeStatus`는 모든 구간에서 `idle`로 고정이었다. 이 필드는 판정에 쓸 수 없다.

권한 대기 시 `claude-hook-sessions.json` 항목의 추가 필드는 `lastBody = "Claude needs your permission"`, `lastSubtitle = "Permission"`이다. 완료(`Stop`) 시에는 `lastBody`에 마지막 응답 요약, `lastSubtitle`에 `Completed in <dir>`이 들어간다.

## 결론

`needsInput`은 실제로 기록된다. 다만 권한 프롬프트가 뜬 직후가 아니라 Claude Code가 권한 알림(`Notification`)을 보낼 때 기록되어 약 5~7초(1회 관찰 약 6초) 늦고, 그 전에는 `agentLifecycle=running`/`PreToolUse`이다. 승인 후에는 `Stop`이 올 때까지 `needsInput`/`Notification`이 남는다(stale). 반면 `~/.claude/sessions/<pid>.json`의 `status`는 프롬프트 표시 후 약 1초 이내(샘플링 해상도 한계)에 `waiting`으로 바뀌고 승인 후 즉시 `busy`로 돌아오므로 가장 빠르고 정확한 신호다. 그래서 `sessions` `status`를 1순위, hook 파일을 2순위로 쓴다.

판정 규칙(번호 순서대로 평가해 처음 일치하는 규칙을 적용한다). 입력은 `pid`(생존 여부는 `kill -0 <pid>`), `S`(sessions `status`, 파일이 없으면 없음), `H`(hook 항목의 `agentLifecycle`/`hookEventName`, 항목이 없으면 없음)다. `S`가 `waiting`/`busy`/`shell`/`idle` 중 하나가 아니면 "인식 불가"로 보고 `S`가 없는 것과 같게 취급한다.

1. `pid`가 죽었다 → `Done`. (`S`와 `H`의 값에 관계없이 가장 먼저 평가한다.)
2. `pid`가 살아 있고 sessions 파일(레코드)도 hook 항목도 없다 → `Done`. sessions 레코드는 있지만 `S`가 인식 불가이거나 비어 있는 경우는 여기에 해당하지 않고 아래 규칙으로 넘어간다(hook도 없으면 규칙 9로 가며, 화면을 읽지 못하면 `Unknown`).
3. `S == waiting` → `NeedsInput`.
4. `S == busy` 또는 `S == shell` → `Running`. (`H`가 `needsInput`이어도 stale로 보고 무시한다. `S`와 `H`가 충돌하면 항상 `S`가 이긴다.)
5. `S == idle` → `Idle`. (`H`가 `running`이어도 `S`가 이긴다.)
6. 여기부터는 `S`가 없거나 인식 불가이고 `H`가 있다. `H.agentLifecycle == needsInput` 이고 `H.hookEventName == Notification` → `NeedsInput`.
7. `H.agentLifecycle == running` → `Running`.
8. `H.agentLifecycle == idle` → `Idle`.
9. 위에 모두 해당하지 않는다(`S`가 없거나 인식 불가이고 `H`도 인식 불가) → `read-screen` 마지막 수단: 화면에 `Do you want to proceed?`가 있으면 `NeedsInput`, 없으면 `Idle`.

경계 사례는 위 규칙으로 정의된다. `S` 파일은 없고 `H`만 있고 pid가 살아 있으면 규칙 6~8(hook 사용). `H`는 없고 `S`만 있으면 규칙 3~5(sessions 사용). sessions 레코드와 hook 항목이 둘 다 없으면 규칙 2(`Done`). sessions 레코드의 `status`만 인식 불가이고 hook이 없으면 규칙 9(화면이 없으면 `Unknown`)로 처리되어, 살아 있는 세션이 처음부터 숨겨지지 않는다. pid가 죽었으면 파일이 남아 있어도 규칙 1(`Done`). 종료 관찰에서는 `S` 파일과 `H` 항목이 거의 동시(약 1초 이내)에 사라졌으므로 한쪽만 먼저 사라진 순간은 규칙 6~8 또는 3~5로 처리된다. `read-screen`은 판정의 주 수단이 아니다.

## 추가 관찰

- `~/.claude/sessions/<pid>.json`의 `status` 전이는 hook 파일보다 항상 빠르거나 같았다. 갱신 시각은 `updatedAt`, `statusUpdatedAt`(밀리초)에 있다.
- `cmux list-status`는 워크스페이스 단위다. 같은 워크스페이스에 Claude 세션이 둘 이상 있으면(관찰 중 이 스파이크를 실행하는 상위 세션도 같은 워크스페이스였다) 값이 섞여 `waiting`과 `running`이 1초 단위로 깜빡였다. 권한 대기 때 `cmux.feed.attention:claude_code=입력 필요` 항목이 추가로 나타났고 승인 후 `실행 중`으로 바뀌었다가 사라졌다. `program_status`에는 `approve Bash: touch x.txt` 같은 문구가 권한 대기 때 표시됐다. 둘 다 워크스페이스 단위다.
- `/exit` 후 약 1초 이내에 `sessions/<pid>.json`이 삭제되고 hook 파일의 세션 항목도 제거됐다.
- 세션 파일에는 `pid`, `sessionId`, `cwd`, `status`, `updatedAt`, `kind`, `name` 등이 있다. hook 항목에는 `pid`, `cwd`, `surfaceId`, `workspaceId`, `transcriptPath`가 있어 cmux surface와 세션을 매핑할 수 있다.
- 새 디렉터리에서 처음 `claude`를 띄우면 "Is this a project you trust?" 프롬프트가 먼저 뜬다. 이 상태에서는 hook 항목이 아직 없다.
- 이 환경의 `claude`는 기본이 auto mode로 시작했다. `shift+tab`으로 manual mode로 바꿔야 권한 프롬프트가 나타났다.
- `cmux send-key`는 대상 surface가 질문/대화 상자 대기 상태이면 `nothing was sent` 오류를 내며 키를 보내지 않는다. 이때는 `--force`를 붙여야 한다.
- `cmux close-surface`는 1~2초 뒤 목록에서 사라지며, 닫힌 뒤 cmux 포커스가 임의의 다른 워크스페이스로 이동했다. 닫은 뒤에는 원래 포커스를 복원해야 한다.

## 관찰하지 못한 것

단일 실행(1회)이라 아래는 확인하지 못했다. Task 4 구현 시 가정으로만 두고 실제 사용 중 확인이 필요하다.

- auto mode에서의 권한 동작(프롬프트가 아예 뜨지 않는지, 어떤 신호가 남는지).
- `AskUserQuestion`, plan 승인 등 권한 프롬프트 이외의 대화 상자가 `waiting`/`needsInput`으로 기록되는지.
- `Esc`로 작업을 중단했을 때의 상태(`Stop`이 오는지, `busy`가 남는지).
- 서브에이전트 실행 중 상태.
- 반복 실행에 따른 지연(약 6초) 변동. 수치는 1회 값이다.
- `sessions` `status`의 `idle`/`busy`/`waiting` 이외 값(`shell` 등)이 실제로 나타나는 조건. `shell`은 Running으로 취급하자는 가정일 뿐 관찰한 값이 아니다.
- 1초 샘플링 해상도 때문에 1초 미만의 전이 순서와 정확한 지연.
- Esc 거절(`No` 선택)이나 `kill`로 비정상 종료했을 때 파일이 남는지.

## 포커스 이동 명령

워크스페이스와 surface(패널)를 함께 지정해 포커스를 옮기는 검증된 명령 순서다.

```
cmux select-workspace --workspace <workspace-ref>
cmux focus-panel --panel <surface-ref> --workspace <workspace-ref>
```

`cmux select-workspace`는 `cmux workspace select <workspace>`의 별칭이며, 둘 다 동작한다. `cmux focus-panel --panel <surface-ref>`는 해당 surface(탭)를 포커스한다. `--workspace`를 생략하면 `$CMUX_WORKSPACE_ID`가 기본이므로, 다른 워크스페이스의 surface는 반드시 `--workspace`를 줘야 한다. `cmux focus-pane --pane <pane-ref> [--workspace <ref>]`는 pane 단위 포커스이며 surface 단위 지정에는 `focus-panel`이 맞다.

검증: 다른 워크스페이스에 있는 상태에서 위 두 명령으로 `workspace:A`의 `surface:B`로 이동했고, `cmux identify`의 `focused.workspace_ref`/`surface_ref`가 대상 값으로 바뀐 것을 확인했다. 같은 두 명령에 원래 값을 넣어 포커스를 복원했고 `cmux identify`로 복원을 확인했다. 현재 포커스는 `cmux identify`의 `focused` 필드에서 읽는다. 참조(ref)는 `workspace:<n>`, `surface:<n>` 형태이며 UUID도 허용된다.

`select-workspace`가 cmux 앱 창 자체를 앞으로 가져오는지(다른 앱이 활성일 때)는 확인하지 않았다. 관찰은 cmux가 이미 활성인 상태에서만 했다. 앱 창이 비활성일 때의 동작은 `cmux focus-window --window <ref>`를 함께 써야 할 수 있으나 검증하지 않았다.
