# cmux-magigent

[![ci](https://github.com/jeanboy44/cmux-cc-sessions-monitor/actions/workflows/ci.yml/badge.svg)](https://github.com/jeanboy44/cmux-cc-sessions-monitor/actions/workflows/ci.yml) ![license MIT](https://img.shields.io/badge/license-MIT-blue) ![platform macOS](https://img.shields.io/badge/platform-macOS-lightgrey)

A cmux extension that shows every Claude Code session running in cmux as a small card with a living spirit, so you can see at a glance which session is working, waiting for you, or done.

<!-- TODO: screenshot/GIF -->

## Features

- One 29x9 card per session, a golden rectangle on screen: a small ghost-like spirit drawn with braille dots in 24-bit colour on the left, and on the right the session name, a status badge with elapsed time, the git branch (or PR) and a summary on up to four lines. `v` switches to a large card, at least 46x10 and stretched to fill its grid cell, with a bigger spirit and more text: the model family, branch and PR on their own rows, and the summary on up to three lines.
- Cards sit on a fluid grid, 2 columns apart side by side and 1 row apart stacked (the same distance on screen, as a terminal cell is about twice as tall as wide). Standard cards keep their size and sit at the left, large cards share the spare width evenly, and a group heading sits directly on its cards.
- Each spirit stands for a Claude model: fable is water, opus is fire, sonnet is lightning, haiku is poison (green). You can change the element per model in settings, and unknown models use the `theme` element (default fire).
- The spirit's state is its animation: running (the body churns fast and leans in short bursts), needs input (a bobbing `?`), idle (dim slow bob and `z`), done (sparkles), unknown (gray).
- Context usage sets how hard the spirit works (relaxed, lively, struggling; see [Context usage setup](#context-usage-setup)), with a red warning flicker above 90%.
- The card border blinks blue while the session has unread cmux notifications, for example when it finished.
- `Enter` (or `Space`) jumps to the session's cmux workspace and pane. `p` shows a live preview of the selected session's terminal.
- Cards are grouped by cmux window. An optional PR chip shows the session's own pull request, taken from the PR meta your hook writes (see Limitations below), not looked up from the branch, so two sessions on the same branch can show different PRs.
- Running Claude Code subagents show on the card: a `◇×N` marker on the standard card's badge row, one `◇ kind · description` row each on the large card, and one square bead each in a grid under the spirit.
- Sessions in remote tabs (`cmux mosh`, `cmux ssh`) get a card too, marked `⇄` (see [Remote sessions](#remote-sessions)).
- Running tool calls show on the card: a `⚙N` marker after the subagent marker (foreground calls plus background Bash / Monitor tasks), and on the large card one `⚙ Bash · cargo test…  12s` row for the tool running now (see [Running tools](#running-tools)).

## Install

```
cargo install --git https://github.com/jeanboy44/cmux-cc-sessions-monitor
```

Then run `cmux-magigent` in a cmux pane. Requires a Rust toolchain (1.89 or newer).

## Usage

| Key | Action |
|---|---|
| `h` `j` `k` `l` / arrows | Move between cards (rows wrap) |
| `Enter` / `Space` | Focus the selected session's cmux workspace and pane |
| `v` | Switch the card layout: standard (29x9) or large (46x10, stretched to fill the row); saved as `card_layout` |
| `p` | Toggle preview (a column of cards of the current layout plus the session's screen) |
| `s` | Switch to **Status** tab (real-time Claude Code session tiles) |
| `m` | Switch to **Memory** tab (Claude memories, timeline, executions, conversations) |
| `i` | Open settings |
| `q` / `Ctrl-C` | Quit |

The selected card has a blue border. `◇×2` after the elapsed time (`◇2` on a one-line card) means two subagents of that session are running. In settings, `j` `k` move, `Enter` or `Space` changes a value (saved immediately), and `Esc` closes.

## Configuration

The config file is `~/.config/cmux-magigent/config.toml` (if it does not exist yet, the old `~/.config/cmux-cc-monitor/config.toml` is read; saving always writes the new path and never touches the old file). It is optional, missing keys use the defaults, and the settings screen (`i`) writes it for you.

| Key | Default | Meaning |
|---|---|---|
| `theme` | `"fire"` | Element for sessions whose model is unknown (`fire`, `water`, `poison`, `lightning`) |
| `model_themes` | fable `water`, opus `fire`, sonnet `lightning`, haiku `poison` | Element per model family, e.g. `[model_themes]` with `opus = "water"` |
| `context_limit_tokens` | `0` | Context window used only when a session has no statusline file and usage is estimated from its transcript. `0` = auto: up to 200k used tokens means a 200k window, more means 1M (the old default `200000` is also read as auto); any other value overrides it |
| `ctx_fresh_seconds` | `300` | How long the statusline context file counts as fresh: a fresh file is used as is; an older one is still used, scaled by how much the transcript has grown since it was last fresh |
| `pr_poll_seconds` | `45` | PR lookup interval |
| `guess_worktree` | `true` | Guess the worktree of sessions started in the main repository |
| `done_retention_minutes` | `10` | How long finished sessions stay on screen |
| `card_layout` | `"standard"` | `"standard"`: 29x9 cards (spirit, name, badge, branch or PR, summary on up to four lines); `"large"`: 46x10 cards (stretched to fill the row) with a bigger spirit and more text (model, branch, PR, summary on up to three lines). Any other value loads as `"standard"`. Also switched with `v` and in settings |
| `subagent_active_seconds` | `30` | A subagent counts as running while its transcript was written within this many seconds (see [Subagents](#subagents)) |
| `timezone` | `"KST"` | Timezone for timestamps in Memory and history viewer and CLI (`"KST"`, `"UTC"`, `"JST"`, `"EST"`, `"PST"` or `+09:00`) |
| `[extras]` | `branch`, `elapsed`, `summary` on; `last_activity`, `pr` off | Extra info shown on cards |

## Remote sessions

Tabs whose title starts with `[mosh]` or `[ssh]` are checked for Claude Code: their sessions run on the other machine, so the monitor sees only what cmux has here. It reads each such tab's screen about every 5 seconds (at most 3 per refresh) and uses the hook records cmux relays to `~/.cmuxterm/relay-hook-state/`.

A remote card has the status badge, the elapsed time and last summary (when the relay has them), notifications, jumping and the preview. It is named after the tab. There is no model, context usage, branch, PR, subagent or tool information, so the spirit uses the `theme` element and the normal motion. When the tab stops showing Claude Code for two reads in a row, the card turns DONE and leaves like any other finished session.

## Context usage setup

cmux-magigent reads context usage from `~/.cache/cmux-magigent/ctx/<session_id>.json`, containing `{"session_id":"...","used_percentage":NN}`. Claude Code's statusline script receives this data on stdin, so add a few lines to yours:

```sh
input=$(cat)
dir="$HOME/.cache/cmux-magigent/ctx"; mkdir -p "$dir"
pct=$(echo "$input" | jq -r '.context_window.used_percentage // empty')
if [ -n "$pct" ]; then
  echo "$input" | jq -c '{session_id: .session_id, used_percentage: .context_window.used_percentage}' \
    > "$dir/$(echo "$input" | jq -r .session_id).json"
fi
```

The file is written only once Claude Code reports a percentage, so no 0% is recorded before the first reply.

The old `~/.cache/cmux-cc-monitor/ctx/` is still read too (the freshest valid file wins), so a statusline script that already writes there keeps working. A file older than `ctx_fresh_seconds` is still used: once the transcript has grown since the file was last fresh, its value is scaled by that growth. Without any file, cmux-magigent falls back to the last usage recorded in the session transcript divided by the context window, which is inferred (200k, or 1M once more than 200k tokens are used) unless `context_limit_tokens` sets it.

The usage drives three stages:

- Relaxed (under 30%): the spirit barely moves, with a slow, tiny bob, calm churn, fewer ambient bits, and a gentler burst pose on only some bursts.
- Lively (30% to 70%, or unknown): the normal motion.
- Struggling (70% and up): the body trembles and pants with a faster bob and a slight squash and stretch, pale sweat drops slide down its side, bursts wind up longer and recoil harder, and an idle spirit slumps with heavy, slow breaths. Above 90% the body also flickers red.

The card size picks the spirit size: the standard card (and the standard cards of the preview column) draws a small spirit, a round body of about 5x3 cells in a 7x4 grid, and the large card draws the bigger 11x4 spirit. The small spirit has the same state cues, element looks, pose and stages; its eyes are two single white dots in proportion to its body.

## Subagents

Claude Code keeps each session's subagents next to its transcript, in `~/.claude/projects/<project>/<session id>/subagents/`: per subagent an `agent-<id>.meta.json` (type, task description, model) and an `agent-<id>.jsonl` transcript that grows while it works. On every refresh (once a second) cmux-magigent lists that folder again for the sessions on screen and checks each subagent transcript's modification time, so a subagent that writes, starts or finishes shows on the next refresh; each meta file is parsed only again when it changes, and the subagent transcripts themselves are never read. At most the eight newest subagents of a session are kept.

A subagent counts as running when its meta does not say `finished: true` (honoured as soon as Claude Code writes it) and its transcript was written within `subagent_active_seconds` (default 30, the recent-activity window); a finished (Done) session has none running. Only running subagents are shown. Older Claude Code versions write no finished flag, so there a subagent that just finished may still show as running for up to the window (about 30 s).

- Standard card: `◇×N` at the end of the badge row (last activity makes room for it first). Nothing when none is running.
- Large card: below the summary, one row per running subagent, newest first, `◇ kind · description` with the model family on the right; at most three rows, the last ending in `+N` when more are running. The name, badge, branch, PR and summary rows always come first.
- One-line cards: `◇N` after the elapsed time.
- Spirit: one square bead per running subagent in a grid under the spirit, four across, filled left to right and top to bottom: three rows on the standard card, four on the large card (the text counts the rest). Each bead is exactly one braille cell (a 2×2 block of dots, `⠶`), so N beads always read as N. Its colour is the element of the model the subagent actually runs on (`pickedEffortModel` from the meta, else the requested `model`, through the same `model_themes` mapping as the spirit; an unknown model takes the spirit's own element). The beads stay still and show the state in a soft shimmer: each twinkles on its own while the spirit runs, all pulse together when it needs input, they breathe slowly and dimly while it is idle, and they are gone once it is done.

## Running tools

cmux-magigent also shows which tool call a session is running right now and how many background tasks it has, read from the session transcript (`~/.claude/projects/<project>/<session id>.jsonl`). The first refresh reads the last 2 MiB of the transcript; later refreshes read only the lines appended since, so the cost per round is one `stat` per session plus the new bytes.

- Current tool: a main-chain `tool_use` block of an assistant reply counts as running until a `tool_result` with its id arrives; an `end_turn` reply, an interrupt or a new prompt clears them all. `Agent` / `Task` calls are left out (they are subagents, see above). The hint is the Bash `description` (else the first command line, at most 30 characters), the file name for Read / Edit / Write / NotebookEdit, the pattern for Grep / Glob, and nothing for other tools. The time is measured from the assistant record's timestamp. Parallel calls show the oldest one and `+N`.
- Background tasks: a main-chain tool result carrying `backgroundTaskId` (Bash run in the background, also when backgrounded later) or a Monitor `taskId` starts a task; a `<task-notification>` naming its task id (any status: completed, failed, killed, stopped) or a `TaskStop` / `KillShell` call naming it ends it.
- Standard and one-line cards: `⚙N` after the subagent marker (N = foreground calls plus background tasks); it is dropped first when the badge row is too narrow.
- Large card: one row after the summary, `⚙ Tool · hint` with the elapsed time, `+N` parallel calls and `bg×N` background tasks on the right, or `⚙ bg ×N` when only background tasks run. While it shows, the summary keeps two lines instead of three; running subagent rows follow it.

Detection limits, chosen to undercount rather than overcount:

- Claude Code's task output files (`/tmp/claude-<uid>/<project>/<session id>/tasks/<task id>.output`) carry no exit marker and stay after the task ends, so they cannot tell running from finished. They only extend a task's life: an open background task counts while it was launched within the last 30 minutes or its output file was written within the last 5 minutes. A silent task running longer than 30 minutes is therefore no longer counted. A Monitor never counts past its `timeoutMs`; a persistent Monitor counts until it is notified or stopped.
- The hook file's `hadPendingBackgroundWorkAtStop` is only a yes/no flag at the moment a turn stopped, so it is not used.
- A pending call of an Idle session older than 120 seconds is ignored (an interrupted or crashed call); a session waiting for permission (`INPUT`) keeps showing the call it asks about. Done sessions show nothing.
- Background tasks started by subagents belong to the subagent and are not counted, and a task launched before the first 2 MiB window is unknown until it ends.

## Memory & History Storage

cmux-magigent embeds an SQLite engine (with WAL mode and progressive disclosure indexing inspired by `claude-mem`) to collect and query Claude's long-term memory, execution history, and transcripts in real time.

Database location: `~/.local/share/cmux-magigent/records.db` (overrideable with `CMUX_MAGIGENT_DB`).

### TUI Viewer (`m`)

Press `m` in the session monitor to enter the interactive Memory & History explorer, or press `o` on any session card in the Status view to open its complete memory bundle:
- `Tab` / `BackTab` (or `Shift+Tab`): switch between tabs:
  1. **1. 메모리 (Memories)**: Project-level memories (`~/.claude/projects/<project>/memory/`), categorised with YAML frontmatter.
  2. **2. 타임라인 / 관찰 (Timeline / Observations)**: Chronological observation narrative (`change`, `discovery`, `execution`, `memory`).
  3. **3. 실행 이력 (Executions)**: Detailed tool and command executions with parameters and outputs.
  4. **4. 대화 기록 (Conversations)**: User/assistant dialogue history and context snippets.
- `Enter` / `o`: open the full consolidated Session Memory Report overlay for the selected item.
- `j` / `k` (or `↓` / `↑`): navigate list items (or scroll report).
- `d` / `u` (or `PageDown` / `PageUp`): scroll 10 lines in detail or session report.
- `/`: live filter query.
- `r`: trigger instant synchronization from disk.
- `s`: switch back to Status tab (`Status [s] | Memory [m]`).
- `Esc` / `q`: close overlay / return to session tiles.

### CLI Query Tool

Query past sessions and memories directly from your terminal:

```sh
# Show database statistics
cmux-magigent memory stats

# Dump complete session memory bundle (Markdown report)
cmux-magigent memory session <SESSION_ID>

# Dump session memory as JSON (for AI pipelines / prompt injection)
cmux-magigent memory session <SESSION_ID> --json

# Save session bundle directly to a file
cmux-magigent memory session <SESSION_ID> -o session_dump.md

# List recent memories across all projects
cmux-magigent memory list --limit 10

# Search memories and transcripts
cmux-magigent memory search "auth"

# View observation timeline
cmux-magigent memory timeline --limit 20

# View structured observations
cmux-magigent memory obs --type change

# Inspect recent tool executions
cmux-magigent memory exec --limit 10

# View recent conversation messages
cmux-magigent memory conv --limit 10

# Manually trigger disk sync
cmux-magigent memory sync
```

## How status is detected

1. The `status` in Claude's own session file (`~/.claude/sessions/<pid>.json`).
2. The cmux hook file (`~/.cmuxterm/claude-hook-sessions.json`).
3. Reading the terminal screen with `cmux read-screen` (only when the first two are unclear).

If the session's process is gone, the session is Done. When signals disagree, the earlier one in this list wins.

## Limitations

- macOS only, and it needs the `cmux` CLI on your `PATH`. Developed against cmux 0.65.
- Needs a terminal with 24-bit colour and a font with braille characters.
- It reads cmux and Claude Code internal files, which are not a public API and may change between versions.
- Read-only: it never sends input to sessions. The preview shows whatever is on the session's screen.
- The PR chip (off by default) reads the session's own `~/.claude/session-meta/<session id>.json`, `{"prs":[{"number":N,"branch":"…",…}, …]}` with the newest entry last (the number may be an integer or a string; other fields are ignored). The entry on the session's current branch wins, else the last one; a session without that file shows no PR. The state (review, approved, …) comes from the branch lookup through your existing `gh` or `az` login when it finds that same PR number, and is shown as `?` otherwise.

## Development

```
cargo build --locked
cargo test --locked
```

`tests/no_personal_info.rs` scans tracked text files for home-directory paths, real-looking UUIDs and extra private patterns you add via `CCMON_FORBIDDEN` or a gitignored `.personal-info-patterns` file.

## License

MIT, see [LICENSE](LICENSE). This is an unofficial project and is not affiliated with Anthropic or cmux. "Claude" and "Claude Code" are trademarks of Anthropic.
