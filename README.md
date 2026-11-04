# cmux-magigent

[![ci](https://github.com/jeanboy44/cmux-cc-sessions-monitor/actions/workflows/ci.yml/badge.svg)](https://github.com/jeanboy44/cmux-cc-sessions-monitor/actions/workflows/ci.yml) ![license MIT](https://img.shields.io/badge/license-MIT-blue) ![platform macOS](https://img.shields.io/badge/platform-macOS-lightgrey)

A cmux extension that shows every Claude Code session running in cmux as a small card with a living spirit, so you can see at a glance which session is working, waiting for you, or done.

<!-- TODO: screenshot/GIF -->

## Features

- One 20x10 card per session: a small ghost-like spirit drawn with braille dots in 24-bit colour, the session name, a status badge with elapsed time, the git branch (or PR) and a one-line summary. `v` switches to a large card, at least 32x14 and stretched to fill its grid cell, with a bigger spirit, a wide area for its magic and more text: the model family, branch and PR on their own rows, and the summary on up to three lines.
- Each spirit stands for a Claude model: fable is water, opus is fire, sonnet is lightning, haiku is poison (green). You can change the element per model in settings, and unknown models use the `theme` element (default fire).
- The spirit's state is its animation: running (the body churns and casts its magic to the right: fire breath from the mouth, and a water vortex, poison fog or lightning bolts centred in the room right of the body), needs input (a bobbing `?`), idle (dim slow bob and `z`), done (sparkles), unknown (gray).
- Context usage sets how hard the spirit works (relaxed, lively, struggling; see [Context usage setup](#context-usage-setup)) and, while running, the size of the magic: small under 30%, normal from 30%, big from 70%, and a red warning flicker above 90%.
- The card border blinks blue while the session has unread cmux notifications, for example when it finished.
- `Enter` (or `Space`) jumps to the session's cmux workspace and pane. `p` shows a live preview of the selected session's terminal.
- Cards are grouped by cmux window. An optional PR chip shows the session's own pull request, taken from the PR meta your hook writes (see Limitations below), not looked up from the branch, so two sessions on the same branch can show different PRs.
- Running Claude Code subagents show on the card: a `◇×N` marker on the standard card's badge row, one `◇ kind · description` row each on the large card, and one round bead each in a row along the bottom of the spirit's animation area, growing right from the body, in the colour of the subagent model's element (see [Subagents](#subagents)).

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
| `v` | Switch the card layout: standard (20x10) or large (32x14, stretched to fill the row); saved as `card_layout` |
| `p` | Toggle preview (a column of cards of the current layout plus the session's screen) |
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
| `card_layout` | `"standard"` | `"standard"`: 20x10 cards (spirit, name, badge, branch or PR, one-line summary); `"large"`: 32x14 cards (stretched to fill the row) with a bigger spirit and more text (model, branch, PR, summary on up to three lines). Any other value loads as `"standard"`. Also switched with `v` and in settings |
| `subagent_active_seconds` | `30` | A subagent counts as running while its transcript was written within this many seconds (see [Subagents](#subagents)) |
| `[extras]` | `branch`, `elapsed`, `summary` on; `last_activity`, `pr` off | Extra info shown on cards |

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

The usage drives three stages, on the same thresholds as the magic size:

- Relaxed (under 30%): the spirit barely moves, with a slow, tiny bob, calm churn, fewer ambient bits, and a gentler cast pose on only some casts.
- Lively (30% to 70%, or unknown): the normal motion.
- Struggling (70% and up): the body trembles and pants with a faster bob and a slight squash and stretch, pale sweat drops slide down its side, casts wind up longer and recoil harder, and an idle spirit slumps with heavy, slow breaths. Above 90% the body also flickers red.

The water vortex, poison fog and lightning bolts are centred in the magic area, the columns from just right of the body to the card's inner right edge (a stretched card, in the grid or the preview column, gives them its full width); the fire breath leaves the mouth, and at big magic it roars to the card's inner right edge. At normal magic the water vortex turns very slowly (about one turn every 8 s) and four thin lightning bolts strike at random spots a tick or two apart; at big magic one thick bolt strikes in the centre among three thin ones, and the poison fog of each cast settles into a skull that lingers for about half a second before it fades.

The card size picks the spirit size: the standard card (and the standard cards of the preview column) draws a small spirit, a round body of about 5x3 cells in a 7x4 grid, and the large card draws the bigger 11x4 spirit, whose magic gets an area of about 22 columns to its right. The small spirit has the same state cues, element looks, pose and stages; its eyes are two single white dots in proportion to its body, and its magic is drawn simpler and bolder so it reads at that size: a short bright flame, a solid disc with a clear highlight, dense round puffs (Big poison still settles into the skull, which fits the standard card), and the same bolts as the bigger spirit.

## Subagents

Claude Code keeps each session's subagents next to its transcript, in `~/.claude/projects/<project>/<session id>/subagents/`: per subagent an `agent-<id>.meta.json` (type, task description, model) and an `agent-<id>.jsonl` transcript that grows while it works. On every refresh (once a second) cmux-magigent lists that folder again for the sessions on screen and checks each subagent transcript's modification time, so a subagent that writes, starts or finishes shows on the next refresh; each meta file is parsed only again when it changes, and the subagent transcripts themselves are never read. At most the eight newest subagents of a session are kept.

A subagent counts as running when its meta does not say `finished: true` (honoured as soon as Claude Code writes it) and its transcript was written within `subagent_active_seconds` (default 30, the recent-activity window); a finished (Done) session has none running. Only running subagents are shown. Older Claude Code versions write no finished flag, so there a subagent that just finished may still show as running for up to the window (about 30 s).

- Standard card: `◇×N` at the end of the badge row (last activity makes room for it first). Nothing when none is running.
- Large card: below the summary, one row per running subagent, newest first, `◇ kind · description` with the model family on the right; at most three rows, the last ending in `+N` when more are running. The name, badge, branch, PR and summary rows always come first.
- One-line cards: `◇N` after the elapsed time.
- Spirit: one round bead per running subagent (at most five, as many as fit; the text counts the rest) in one row along the bottom of the animation area, starting one empty cell right of the body and growing to the right. Each bead is exactly one braille cell (`⠿`) with one empty cell between two beads, so N beads always read as N. Its colour is the element of the model the subagent actually runs on (`pickedEffortModel` from the meta, else the requested `model`, through the same `model_themes` mapping as the spirit; an unknown model takes the spirit's own element). The beads bob one sub-pixel behind the spirit, pulse while it runs, are dim while it is idle and are gone once it is done.

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
