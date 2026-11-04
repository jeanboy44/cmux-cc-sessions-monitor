> Superseded/abandoned — replaced by the spirit UI (`../specs/2026-11-04-spirit-ui-design.md`).

# pixelfx 마법 UI Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 세션 카드를 pixelfx 히어로·더미·마법 연출(fire/water/ice/lightning/melee)로 교체하고, Claude 상태에 따라 랜덤으로 마법을 시전하는 장면을 보여 준다.

**Architecture:** pixelfx의 JS 코어(math/color/sim/painter/rig/act/chain)와 선택한 연출을 `src/fx/`에 Rust로 1:1 포팅한다(함수·옵션 이름은 snake_case로 대응). 포팅한 엔진은 RGBA 프레임을 만들고, `ui::scene`이 그것을 점자(2×4)로 ratatui 버퍼에 그린다. 카드는 장면 + 정보 줄로 구성하며, 상태→장면 매핑과 랜덤 시전은 `fx::caster`가 맡는다.

**Tech Stack:** Rust 2021 (rust-version 1.89), ratatui 0.30, crossterm 0.29, 새 크레이트 없음. 골든 프레임 생성에만 개발용 Node(상류 pixelfx 클론 필요, 최종 바이너리·CI에는 불필요).

**Spec:** `docs/superpowers/specs/2026-11-04-pixelfx-ui-design.md`. 상류 소스는 https://github.com/patch-gg/pixelfx (MIT)의 `src/core`, `src/fx/<category>`, `src/fx/moves`이며 각 태스크에서 "상류 `<path>`"로 가리킨다. 이 계획은 포팅 대상 파일을 명시하지만, 연출이 import하는 파일은 태스크 안에서 끝까지 따라가 모두 포팅한다.

## Global Constraints

- macOS 전용, Rust 2021, `rust-version = "1.89"`, 새 의존성 추가 금지(이미지/JS 런타임 포함).
- 1단계 렌더는 **원본 픽셀 1:1**, 점자 2×4 도트, 칸당 색 1개. 배율은 한 곳(`ui::scene::SCALE`)의 상수로 두어 나중에 바꿀 수 있게 한다.
- 장면은 히어로–더미 간격을 줄여 약 90px 폭(원본 `TX`≈190 → 약 70~80px)으로 쓴다. 그 값은 한 상수(`fx::SCENE_TX`)로 둔다.
- 알파/발광은 장면 전용 어두운 배경색과 합성한다. 터미널 배경은 읽지 않는다.
- 골든 비교는 비트 일치가 아니라 허용 오차(차이 픽셀 비율)로 한다. 정수 해시/RNG(`hash`, `Rng`)는 원본과 정확히 같아야 한다.
- 애니메이션 약 15fps, 장면이 화면에 보이는 카드만 계산한다.
- NOTICE에 patch-gg/pixelfx, Hugo Duprez(Sprite Fusion), Hollow Crown(rehan-remade) MIT 고지 포함.
- 개인정보 가드(`tests/no_personal_info.rs`) 통과: 코드·픽스처·문서에 홈 디렉터리 절대경로, 실제 UUID, 회사 문자열 금지. 골든 스크립트에 로컬 경로를 하드코딩하지 말고 환경변수 `PIXELFX_DIR`로 받는다.
- 커밋: 작성자 `jeanboy44 <noreply@users.noreply.github.com>` (-c 플래그), 메시지 끝에 `Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>`.
- 문서는 한국어(내부 문서), 소스 주석은 영어.

## Review Focus

1. 터미널이 장면 하나보다 좁거나 낮을 때: 패닉 없이 한 줄짜리 간단 카드로 바뀐다 (Task 5).
2. 영역 너비/높이가 0이거나 1×1일 때 점자 렌더러가 패닉하지 않는다 (Task 4).
3. 오래된 config(`view = "a"` 등)가 그대로 로드되고, 알 수 없는 테마 문자열은 기본 테마로 떨어질 뿐 파일을 덮어쓰거나 죽지 않는다 (Task 5).
4. cwd가 비어 있는 세션에서도 `t`가 동작하고(세션 id를 키로 사용) 설정이 저장된다 (Task 12).
5. Running 세션이 30개여도 화면 밖 카드는 시뮬레이션하지 않고 프레임 계산이 예산(카드당 수 ms) 안에 든다 (Task 12).
6. 시전 도중 상태가 바뀌어도(Running→NeedsInput 등) 장면이 깨지지 않고 현재 시전을 마친 뒤 전환한다 (Task 12).

---

## 파일 구조

- `src/fx/mod.rs` — 모듈 선언, `SCENE_TX`, 공용 타입 `Frame`(재export).
- `src/fx/math.rs`, `color.rs` — 상류 `src/core/math.js`, `color.js`.
- `src/fx/frame.rs`, `painter.rs` — 상류 `fx.js`(Frame·composite), `painter.js`, `shapes.js`.
- `src/fx/sim.rs` — 상류 `sim.js`, `particles.js`.
- `src/fx/actor.rs`, `rig.rs`, `poses.rs`, `act.rs`, `chain.rs` — 상류 `actor.js`, `rig.js`, `poses.js`, `act.js`, `chain.js`.
- `src/fx/effects/{arcane,explosions,impacts,fire,ice,lightning,melee,water}.rs` — 각 카테고리의 필요한 연출과 `_lib.js`.
- `src/fx/moves.rs` — 상류 `src/fx/moves/*` 중 채택한 것 + water 조합.
- `src/fx/theme.rs` — `Theme` enum(Fire/Water/Ice/Lightning/Melee), 테마별 시전 목록.
- `src/fx/caster.rs` — 카드별 상태 머신(상태→장면, 랜덤 시전).
- `src/ui/scene.rs` — `Frame` → 점자 위젯. `src/ui/card.rs` — 새 카드(장면+정보 줄), `tile.rs`·`mascot.rs`·A/B 뷰 코드는 제거.
- `tools/golden/dump.mjs` — 상류에서 골든 프레임/수치 덤프. `tests/golden/*` — 커밋된 소형 픽스처. `tests/golden.rs` 또는 각 모듈의 `#[cfg(test)]` — 비교 헬퍼.
- `NOTICE.md`.

---

### Task 1: 골든 도구와 math/color 포팅

**Files:**
- Create: `tools/golden/dump.mjs`, `tests/golden/math.json`, `src/fx/mod.rs`, `src/fx/math.rs`, `src/fx/color.rs`
- Modify: `src/main.rs`(모듈 선언), `.gitignore`는 건드리지 않음

**Interfaces:**
- Produces: `fx::math::{hash(&[f64]) -> u32, rand(&[f64]) -> f64, Rng{ new(u32), next()->f64, range(a,b)->f64, chance(p)->bool, ... }, noise(x,seed), noise2(x,y,seed), fbm(x,y,seed,octaves), clamp, lerp, smooth, seg, bump, ease::*, TAU}`, `fx::color::{rgb(r,g,b)->u32, hex(&str)->u32, mix(a,b,t)->u32, scale, fit(&[u32], usize)->Vec<u32>, at, blend, PAL, WHITE, INK}`. 색은 상류와 같은 `0xAABBGGRR` u32(rgb()가 `(255<<24)|(b<<16)|(g<<8)|r`)로 유지한다.
- Consumes: 없음.

- [ ] **Step 1: `tools/golden/dump.mjs`를 만든다.** `PIXELFX_DIR`(상류 클론 경로, 없으면 에러)에서 `src/core/math.js`를 import해 고정 입력(여러 `hash` 인자 조합, 시드 7·42로 `Rng` 20개 출력, `noise/noise2/fbm` 격자 샘플)의 결과를 `tests/golden/math.json`으로 쓴다. 로컬 경로를 파일에 박지 않는다.
- [ ] **Step 2: 실패하는 테스트를 쓴다** — `fx::math::tests::matches_upstream`: `tests/golden/math.json`을 읽어 `hash`/`Rng` 출력은 **정확히 일치**, `noise*`/`fbm`은 `1e-9` 이내로 일치함을 assert. `fx::color::tests::{hex_rgb_roundtrip, mix_midpoint, fit_resamples_ramp_to_n}`도 상류 값으로 작성.
- [ ] **Step 3: 실행해 실패 확인.** Run: `cargo test fx:: --quiet` → 모듈 없음으로 FAIL.
- [ ] **Step 4: 위 Interfaces의 시그니처로 포팅한다.** 상류 `math.js`의 정수 연산(해시 믹싱, mulberry류 RNG)은 `u32` 래핑 연산으로 JS의 `Math.imul`/`>>>0` 동작을 정확히 재현한다.
- [ ] **Step 5: 테스트 통과 확인.** Run: `cargo test fx:: --quiet` → PASS.
- [ ] **Step 6: 커밋.** `feat(fx): golden tool, math and color ports`

### Task 2: Frame, Painter, composite

**Files:**
- Create: `src/fx/frame.rs`, `src/fx/painter.rs`, `src/fx/shapes.rs`, `tools/golden/frames.mjs`, `tests/golden/painter_*.bin`(소형)
- Test: 각 파일의 `#[cfg(test)]`

**Interfaces:**
- Consumes: Task 1의 `math`, `color`.
- Produces: `Frame { w: usize, h: usize, ox: i32, oy: i32, color: Vec<u32>, light: Vec<u32> }` with `Frame::new(w,h,ox,oy)`, `clear()`; `composite(frame: &Frame, bg: u32) -> Vec<u32>`(RGBA, 불투명 색 + 가산 발광을 bg 위에 합성); `Painter<'a>` — 상류 `painter.js`의 공개 메서드(점/선/원/사각형/`shade`/`blob` 등)를 같은 이름 snake_case로.

- [ ] **Step 1: 골든 덤프를 만든다.** `tools/golden/frames.mjs`: 상류 `Painter`로 고정 도형 몇 개(점, 선, 채운 원, `shade`)를 작은 프레임에 그려 `composite()` 결과를 `tests/golden/painter_*.bin`(헤더 w,h + RGBA 바이트)로 저장한다. 픽스처 총합 200KB 이하.
- [ ] **Step 2: 실패하는 테스트.** `painter::tests::shapes_match_upstream`은 같은 도형을 Rust Painter로 그려 픽스처와 비교한다(차이 픽셀 비율 ≤ 0.5%, 채널 차이 ≤ 2). `frame::tests::composite_adds_light_over_color`: 발광 레이어가 색 위에 가산됨을 확인.
- [ ] **Step 3: 구현** 후 **Step 4: 통과 확인.** Run: `cargo test fx::painter fx::frame --quiet`.
- [ ] **Step 5: 커밋.** `feat(fx): frame, painter and composite`

### Task 3: 시뮬레이션(입자) 포팅

**Files:**
- Create: `src/fx/sim.rs`, `src/fx/particles.rs`
- Test: `#[cfg(test)]`, 골든 픽스처 1~2개

**Interfaces:**
- Consumes: Task 1–2.
- Produces: 상류 `sim.js`/`particles.js`가 export하는 것 중 연출이 쓰는 것 전부 — `Particles`, `Bits`, `Lights`, `Blazes`, 상수 `GRAVITY, FIRE_RAMP, SMOKE_RAMP`, 헬퍼 `sparkle, flare, band, crystal` — 를 같은 이름(타입은 CamelCase 유지, 함수는 snake_case)으로.

- [ ] **Step 1: 골든을 만든다**: 시드 고정으로 `Particles`/`Bits`를 몇 프레임 구동해 `composite()`한 프레임 2장을 `tests/golden/sim_*.bin`에 저장.
- [ ] **Step 2: 실패하는 테스트** `sim::tests::particles_match_upstream`(차이 픽셀 비율 ≤ 3%)와 `Particles`가 수명이 다한 입자를 제거하는 단위 테스트.
- [ ] **Step 3: 포팅, Step 4: 통과 확인, Step 5: 커밋** `feat(fx): particle simulation port`

### Task 4: 히어로·더미 스프라이트와 점자 렌더러

**Files:**
- Create: `src/fx/actor.rs`, `src/ui/scene.rs`
- Modify: `src/ui/mod.rs`
- Test: `src/ui/scene.rs`의 `#[cfg(test)]`

**Interfaces:**
- Consumes: `Frame`, `composite`.
- Produces: `fx::actor::{HERO2: [&str; 38], DUMMY_PAL, HERO_PAL, draw_actor(&mut Frame, x, y, kind: ActorKind), actor_mask(..)}` — 상류 `actor.js`의 24×38 `HERO2`와 `DUMMY_PAL`을 문자열 그리드 그대로 옮긴다. `ui::scene::{SCALE: usize = 1, scene_cells(w_px: usize, h_px: usize) -> (u16, u16), render_braille(buf: &mut Buffer, area: Rect, rgba: &[u32], w_px: usize, h_px: usize, bg: u32)}`.

- [ ] **Step 1: 실패하는 테스트.**
  - `scene::tests::cells_for_native_hero`: `scene_cells(24, 38) == (12, 10)`.
  - `scene::tests::single_lit_pixel_sets_one_braille_dot`: 투명 프레임에서 (0,0)만 칠하면 첫 칸이 `⠁`.
  - `scene::tests::cell_color_is_average_of_lit_pixels`.
  - `scene::tests::zero_area_and_clipped_area_do_not_panic`: `area`가 0×0, 1×1, 프레임보다 작을 때(Review Focus 2).
  - `actor::tests::hero_and_dummy_are_24x38`: 각 행 길이 24, 38행.
- [ ] **Step 2: 실행해 FAIL 확인. Step 3: 구현** — 칸 안의 2×4 픽셀 중 배경과 다른(알파>0) 픽셀의 평균색을 fg로, 켜진 점 비트로 점자 코드포인트(U+2800+bits)를 만든다. **Step 4: PASS 확인.**
- [ ] **Step 5: 커밋.** `feat(ui): pixelfx actors and braille scene renderer`

### Task 5: 새 카드 레이아웃과 테마 설정 (Pochi·A/B 제거)

**Files:**
- Create: `src/ui/card.rs`, `src/fx/theme.rs`
- Modify: `src/ui/screen.rs`, `src/ui/grid.rs`, `src/ui/settings.rs`, `src/ui/mod.rs`, `src/config.rs`, `src/app.rs`
- Delete: `src/ui/tile.rs`, `src/ui/mascot.rs`(재사용하는 헬퍼 `fmt_duration`, `truncate_to_width`, `shake_rect` 등은 `card.rs`나 `style.rs`로 옮김)
- Test: 각 파일의 `#[cfg(test)]`

**Interfaces:**
- Consumes: Task 4.
- Produces: `fx::theme::Theme { Fire, Water, Ice, Lightning, Melee }`(serde lowercase, `Default = Fire`, `next()`), `Config.theme: Theme`, `Config.card_themes: BTreeMap<String, Theme>`(키는 cwd, 없으면 세션 id), `ui::card::{CARD_W, CARD_H, render_card(f, rect, session, cfg, selected, scene: Option<&Frame>), render_compact(f, rect, session, cfg, selected)}`, `ui::grid::card_cols(width: u16) -> usize`. 이 단계의 장면은 **정적**(히어로와 더미가 서 있는 `Frame`)이다.

- [ ] **Step 1: 실패하는 테스트.**
  - `config::tests::old_view_key_still_loads`: `view = "a"`가 든 toml이 에러 없이 로드되고 `theme`이 기본값.
  - `config::tests::unknown_theme_falls_back_to_default`: `theme = "bogus"` → 로드 실패로 기본 Config가 되되 `LOAD_FAILED` 안내가 나옴(파일을 덮어쓰지 않음, 기존 백업 동작 유지)(Review Focus 3).
  - `card::tests::card_shows_scene_and_info_lines`: 80×24 TestBackend에서 카드가 장면 영역과 이름/상태/브랜치 줄을 모두 그림.
  - `card::tests::narrow_terminal_uses_compact_card_without_panic`: 폭 30, 높이 8 등(Review Focus 1).
  - `grid::tests::card_cols_scales_with_width`: 폭 100에서 2열, 폭 60에서 1열.
  - 미리보기(`p`) 모드에서는 compact 카드를 쓴다는 테스트.
- [ ] **Step 2: FAIL 확인. Step 3: 구현** — 카드 = 장면(점자 `scene_cells` 크기) + 정보 줄(기존 tile의 이름/상태 배지/branch/PR/요약 로직 재사용). 선택 카드는 `style::SELECTED` 파랑 테두리, 창 그룹 구분선·h/l/j/k 이동·미리보기(`p`)·HUD는 유지. 설정 화면에서 `view` 항목을 `기본 테마`로 교체. `ViewMode`는 config 하위호환을 위해 읽기만 하고 무시하거나 제거한다(필드가 없어도 알 수 없는 키는 무시됨).
- [ ] **Step 4: 전체 테스트 PASS 확인.** Run: `cargo test --quiet`
- [ ] **Step 5: 수동 확인.** `cargo build --release` 후 하단 패인(`jeanboy-cmux-guide` 스킬)에서 실행해 원본 크기 카드가 어떻게 보이는지 사용자에게 보여 준다(사용자의 "1. 원본 픽셀 크기로 보기").
- [ ] **Step 6: 커밋.** `feat(ui): pixelfx card layout with themes, drop Pochi and A/B views`

### Task 6: 리그·포즈·act·chain 엔진

**Files:**
- Create: `src/fx/rig.rs`, `src/fx/poses.rs`, `src/fx/act.rs`, `src/fx/chain.rs`, `tools/golden/act.mjs`, `tests/golden/act_*.bin`

**Interfaces:**
- Consumes: Task 1–4.
- Produces: 상류 `rig.js`(`skeleton`, `posed_parts`), `poses.js`(`READY`), `act.js`(`Pose`, `STAND`, `pose_at(track, t)`, `lerp_pose`, `Target`, `SKINS`, `create_act`, `act_bounds`, `act_span`), `chain.js`(`chain`, `shift_keys`, `shift_hits`). 모듈 단위 공개 API: `Move`(메타 `size, origin, duration, fps`, `act: Act`, `create: Chain`)와 `render_move(mv: &Move, t: f64, seed: u32, tx: f64) -> Frame`(히어로·더미·연출 합성).

- [ ] **Step 1: 골든** — 연출 없는 act(예: READY 포즈 정지 + 더미)의 t=0, t=1.0 프레임, 포즈 보간(`pose_at`) 샘플 수치 JSON.
- [ ] **Step 2: 실패하는 테스트** `act::tests::pose_at_matches_upstream`(수치 1e-9), `act::tests::ready_frame_matches_upstream`(허용오차 비교), `act::tests::target_flinches_on_hit`(hit 시각에 더미 오프셋/스킨 변화).
- [ ] **Step 3: 포팅, Step 4: PASS 확인, Step 5: 커밋** `feat(fx): rig, poses, act and chain engine`

### Task 7: fire 테마

**Files:**
- Create: `src/fx/effects/{arcane,explosions,fire}.rs`(필요한 연출과 `_lib`), `src/fx/moves.rs`(fire 부분), `tests/golden/move_fire_*.bin`

**Interfaces:**
- Consumes: Task 6.
- Produces: `fx::moves::fire() -> Vec<Move>` — 상류 `moves/fireball-throw`(+ `arcane/spell-charge`, `fire/fireball`, `explosions/small-explosion` 및 그 import 전부)와 `moves/`에서 fire 성격인 하나 더(`meteor-call` 또는 `rocket-launcher` 중 의존이 작은 쪽). `TX`는 `SCENE_TX`로 대체.

- [ ] **Step 1: 골든** — fireball-throw를 시드 1로 t=0.4(충전), t=1.2(비행), t=1.6(폭발) 3프레임 덤프(원본 TX 기준, 비교는 같은 TX로).
- [ ] **Step 2: 실패 테스트** `moves::tests::fireball_throw_matches_upstream`(차이 픽셀 비율 ≤ 5%, 폭발 같은 입자 비중이 크므로 프레임별 허용치를 둔다), `moves::tests::every_move_has_positive_duration_and_fits_frame`.
- [ ] **Step 3: 포팅(의존 연출 전부), Step 4: PASS, Step 5: 커밋** `feat(fx): fire theme`

### Task 8: ice 테마

상류 `moves/ice-lance-throw`(+ `arcane/spell-charge`, `ice/ice-lance`, `ice/snowflake-burst`와 `ice/_lib`)와 ice 성격의 move 하나 더(없으면 `ice/freeze`나 `ice/frost-nova` 연출을 fireball-throw와 같은 `chain` 구조로 조합). 파일: `src/fx/effects/ice.rs`, `src/fx/moves.rs`(ice 부분). 테스트·커밋 형식은 Task 7과 같고 골든은 `move_ice_*`. `fx::moves::ice() -> Vec<Move>`. 커밋 `feat(fx): ice theme`.

### Task 9: lightning 테마

상류 `moves/lightning-call`(+ `lightning/lightning-strike`와 `lightning/_lib`)와 lightning 성격의 연출 하나 더(`lightning/chain-lightning`·`thunderclap` 등을 chain으로 조합). `fx::moves::lightning() -> Vec<Move>`. 테스트: 골든 `move_lightning_*`, 번개 순간 더미가 `shock` 스킨을 받는지(`target` 테스트). 커밋 `feat(fx): lightning theme`.

### Task 10: melee 테마

상류 `moves/sword-combo`(+ `melee/sword-slash`, `double-slash`, `heavy-cleave`, `melee/_lib`)와 `moves/dash-strike`(+ `impacts/crit-hit`). `fx::moves::melee() -> Vec<Move>`. 테스트: 골든 `move_melee_*`, 히어로가 더미 쪽으로 이동(`x` 포즈)하는 move에서 장면 폭 `SCENE_TX` 안에 들어오는지. 커밋 `feat(fx): melee theme`.

### Task 11: water 테마(직접 조합)

**Files:** `src/fx/effects/water.rs`(`water/_lib`, `water-orb`, `water-jet`, `splash`, `wave-crash`), `src/fx/moves.rs`(water 부분).

**Interfaces:** `fx::moves::water() -> Vec<Move>` — 상류에 move가 없으므로 `fireball-throw`의 구조를 본뜬 두 가지: (a) 충전(`spell-charge`, `palette` = 물색) → `water-orb`가 더미까지 비행 → `splash`; (b) 충전 → `water-jet` → `wave-crash`. 히어로 포즈 트랙과 더미 `ice`/`flash` 스킨 선택은 제 판단으로 `fireball-throw`를 복사해 시각만 맞춘다.

- [ ] **Step 1: 실패 테스트** `moves::tests::water_moves_render_nonempty_frames_at_all_phases`(충전/비행/명중 시각에서 프레임에 불투명 픽셀이 있음), 골든은 만들지 않는다(원본에 없는 연출).
- [ ] **Step 2: 구현, Step 3: PASS, Step 4: 커밋** `feat(fx): water theme`

### Task 12: 시전 상태 머신과 UI 연결

**Files:**
- Create: `src/fx/caster.rs`
- Modify: `src/ui/screen.rs`, `src/app.rs`(키 `t`, 카드별 테마, 틱), `src/ui/settings.rs`(기본 테마 선택), `src/config.rs`(저장)

**Interfaces:**
- Consumes: Task 5–11.
- Produces: `fx::caster::Caster { new(seed_key: &str, now_tick: u64) }`, `Caster::frame(&mut self, status: Status, theme: Theme, tick: u64) -> &Frame`(상태→장면: Running=마법 반복 시전, NeedsInput=충전 자세로 대기·깜빡임, Idle=READY, Done=그을린 더미+승리 포즈 정적, Unknown=어둡게 정적), 카드별 `Caster`는 `App`이 세션 id→`Caster` 맵으로 들고 보이는 카드만 갱신. `App::cycle_theme_for_selected()`와 `t` 키 → `config.card_themes[key]` 저장(`key` = cwd, 비어 있으면 세션 id).

- [ ] **Step 1: 실패 테스트.**
  - `caster::tests::running_casts_repeatedly_with_gaps`: 시드 고정으로 일정 틱 동안 시전이 2회 이상 시작되고 사이에 대기 간격이 있음.
  - `caster::tests::seed_depends_on_key_and_cast_count`: 같은 키는 같은 시드열, 다른 키는 다른 시작 시점(스태거).
  - `caster::tests::status_change_finishes_current_cast_first`: Running 시전 중 NeedsInput으로 바뀌어도 현재 시전이 끝난 뒤 전환(Review Focus 6).
  - `caster::tests::needs_input_pose_differs_from_idle`.
  - `app::tests::t_key_cycles_theme_for_selected_and_persists`(cwd가 있는 경우와 빈 경우 둘 다, Review Focus 4).
  - `screen::tests::offscreen_cards_are_not_simulated`: 세션 30개, 보이는 카드 수만큼만 `Caster::frame`이 호출됨(호출 카운터) (Review Focus 5).
  - `caster::tests::frame_time_budget`: `#[ignore]` 벤치 — 장면 6개를 1프레임 계산하는 시간을 출력(예산 기준: 6개 합계 < 40ms, 초과 시 알림).
- [ ] **Step 2: FAIL 확인. Step 3: 구현** — 시전 선택은 `theme`의 move 목록에서 시드로 고르고, 간격은 시드 기반으로 약 1.5~4초(제 판단). `FPS`(`src/app.rs`의 `FRAME`)는 장면이 하나라도 보이면 약 15fps로, 없으면 기존 값으로.
- [ ] **Step 4: 전체 테스트 PASS 확인. Step 5: 하단 패인에서 실행해 사용자에게 보여 준다. Step 6: 커밋** `feat: status-driven casting scenes`

### Task 13: 정리·고지·문서

**Files:** `NOTICE.md`(신규), `README.md`, `Cargo.toml`(미사용 `notify` 의존이 있으면 제거), `docs/`

- [ ] **Step 1: `NOTICE.md`를 작성** — patch-gg/pixelfx(MIT), Hugo Duprez(Sprite Fusion), rehan-remade(Hollow Crown) 각각의 저작권 문구와 라이선스 전문 링크. 포팅 사실을 한 줄로 명시.
- [ ] **Step 2: README의 마스코트·뷰 설명을 새 UI(테마 5종, `t` 키, 설정)로 갱신.**
- [ ] **Step 3: 개인정보 가드 포함 전체 테스트와 빌드.** Run: `cargo test --quiet && cargo build --release` → PASS. 바이너리에 로컬 경로가 없는지 기존 배포 스크립트 방식(`--remap-path-prefix`)으로 확인.
- [ ] **Step 4: 커밋.** `docs: NOTICE and README for the pixelfx UI`

---

## Self-review 메모

- 스펙 커버리지: 테마 5종(T7–11), 상태별 장면(T12), `t`/설정(T5·12), 원본 1:1 크기 + 배율 상수(T4·5), 골든 허용오차(T1–3·6–10), NOTICE(T13), 간단 카드·미리보기(T5), 원본 연출 외 water 조합(T11).
- 타입 일관성: `Frame`/`Move`/`Theme`/`Caster` 이름과 위치가 태스크 간 동일.
- 리스크: 시뮬레이션 포팅(T3)과 rig/act(T6)가 가장 큼 — 이 둘이 끝나면 테마 포팅은 반복 작업이다. T5 직후(정적 장면)에 사용자가 크기를 보고 배율을 조정할 수 있다.
