# Magigent

[![ci](https://github.com/jeanboy44/cmux-cc-sessions-monitor/actions/workflows/ci.yml/badge.svg)](https://github.com/jeanboy44/cmux-cc-sessions-monitor/actions/workflows/ci.yml) ![license MIT](https://img.shields.io/badge/license-MIT-blue) ![platform macOS](https://img.shields.io/badge/platform-macOS-lightgrey)

A cmux extension that shows every Claude Code session running in cmux as a small card with a living spirit, so you can see at a glance which session is working, waiting for you, or done.

<!-- TODO: screenshot/GIF -->

## Features

- One 20x9 card per session: a small ghost-like spirit drawn with braille dots in 24-bit colour, the session name, a status badge with elapsed time, and the git branch (or PR).
- Each spirit stands for a Claude model: fable is water, opus is fire, sonnet is lightning, haiku is poison (green). You can change the element per model in settings, and unknown models use the `theme` element (default fire).
- The spirit's state is its animation: running (the body churns and casts its magic to the right: fire breath, water vortex, poison fog, lightning bolt), needs input (a bobbing `?`), idle (dim slow bob and `z`), done (sparkles), unknown (gray).
- While running, context usage sets the size of the magic: small under 30%, normal, big from 70%, and a red warning flicker above 90%.
- The card border blinks blue while the session has unread cmux notifications, for example when it finished.
- `Enter` jumps to the session's cmux workspace and pane. `p` shows a live preview of the selected session's terminal.
- Cards are grouped by cmux window. An optional PR chip shows the open pull request for the branch.

## Install

```
cargo install --git https://github.com/jeanboy44/cmux-cc-sessions-monitor
```

Then run `magigent` in a cmux pane. Requires a Rust toolchain (1.89 or newer).

## Usage

| Key | Action |
|---|---|
| `h` `j` `k` `l` / arrows | Move between cards (rows wrap) |
| `Enter` | Focus the selected session's cmux workspace and pane |
| `p` | Toggle preview (one-line cards plus the session's screen) |
| `i` | Open settings |
| `q` / `Ctrl-C` | Quit |

The selected card has a blue border. In settings, `j` `k` move, `Enter` or `Space` changes a value (saved immediately), and `Esc` closes.

## Configuration

The config file is `~/.config/magigent/config.toml` (if it does not exist yet, the old `~/.config/cmux-cc-monitor/config.toml` is read; saving always writes the new path and never touches the old file). It is optional, missing keys use the defaults, and the settings screen (`i`) writes it for you.

| Key | Default | Meaning |
|---|---|---|
| `theme` | `"fire"` | Element for sessions whose model is unknown (`fire`, `water`, `poison`, `lightning`) |
| `model_themes` | fable `water`, opus `fire`, sonnet `lightning`, haiku `poison` | Element per model family, e.g. `[model_themes]` with `opus = "water"` |
| `context_limit_tokens` | `200000` | Context window size used when usage is estimated from the transcript; for 1M-context models without the statusline file, raise it (e.g. `1000000`) |
| `ctx_fresh_seconds` | `300` | How long the statusline context file is trusted |
| `pr_poll_seconds` | `45` | PR lookup interval |
| `guess_worktree` | `true` | Guess the worktree of sessions started in the main repository |
| `done_retention_minutes` | `10` | How long finished sessions stay on screen |
| `[extras]` | `branch`, `elapsed`, `summary` on; `last_activity`, `pr` off | Extra info shown on cards |

## Context usage setup

Magigent reads context usage from `~/.cache/magigent/ctx/<session_id>.json`, containing `{"session_id":"...","used_percentage":NN}`. Claude Code's statusline script receives this data on stdin, so add a few lines to yours:

```sh
input=$(cat)
dir="$HOME/.cache/magigent/ctx"; mkdir -p "$dir"
pct=$(echo "$input" | jq -r '.context_window.used_percentage // empty')
if [ -n "$pct" ]; then
  echo "$input" | jq -c '{session_id: .session_id, used_percentage: .context_window.used_percentage}' \
    > "$dir/$(echo "$input" | jq -r .session_id).json"
fi
```

The file is written only once Claude Code reports a percentage, so no 0% is recorded before the first reply.

The old `~/.cache/cmux-cc-monitor/ctx/` is still read too (the freshest valid file wins), so a statusline script that already writes there keeps working. Without any file, Magigent falls back to the last usage recorded in the session transcript divided by `context_limit_tokens` (set it to `1000000` for 1M-context models).

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
- PR lookup (off by default) uses your existing `gh` or `az` login.

## Development

```
cargo build --locked
cargo test --locked
```

`tests/no_personal_info.rs` scans tracked text files for home-directory paths, real-looking UUIDs and extra private patterns you add via `CCMON_FORBIDDEN` or a gitignored `.personal-info-patterns` file.

## License

MIT, see [LICENSE](LICENSE). This is an unofficial project and is not affiliated with Anthropic or cmux. "Claude" and "Claude Code" are trademarks of Anthropic.
