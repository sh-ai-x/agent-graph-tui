# agent-graph-tui

Lightweight, single-binary terminal dashboard for AI-coding-agent
execution graphs. Distinguishes **Claude Code / Codex / MiniMax / Gemini**
sessions, shows the worktree + branch + task each one is working on, and
draws the live execution graph for the focused session.

Read-only. No daemon. No electron. Cold-starts in ~1 s, lives in ~0.7 MB.

```
$ agent-graph-tui                     # multi-session dashboard (default)

agent-graph-tui  3 active sessions  ·  ↑/↓ select   ⏎ expand   q quit
──────────────────────────────────────────────────────────────────
  ▶ 🤖 claude-code  ~/repo/.worktrees/foo
    ⏵ feat/foo · 8 nodes · 12s ago
      └ task: fix parser bug
      ── execution graph ──
      user        "fix parser bug"
      assistant   "reading..."
      tool Read parser.rs                ✓
        └ result  4.2 KB                ✓
      tool Edit parser.rs                ✓
        └ result  ok                    ✓

  ☒ ⚒ codex        ~/.codex/sessions/2026-08-12/foo.jsonl
    ⏵ main · 12 nodes · 2m ago
      └ task: add login flow

  ☒ ✦ minimax      ~/work/another
    ⏵ feat/x · 4 nodes · 5m ago
      └ task: refactor parser
```

## Why

[herdr](https://herdr.dev/) is a Rust agent multiplexer. It runs an
`herdr-server` daemon + client + pty layer to multiplex agents — overkill
when you only want to *see* what's running. `agent-graph-tui` is a single
binary, ~0.7 MB, no runtime deps, no server.

## Install

```sh
cargo install --path .
```

Or build locally:

```sh
git clone https://github.com/sh-ai-x/agent-graph-tui
cd agent-graph-tui
cargo build --release
./target/release/agent-graph-tui
```

## Usage

| Command | Mode |
|---|---|
| `agent-graph-tui`                                       | **Multi-session dashboard** — discover and live-tail every active session |
| `agent-graph-tui <path.jsonl>`                          | **Single-session viewer** — focused view of one session file |
| `agent-graph-tui < …`                                   | Plain-text fallback for any non-TTY stdout (CI/logs) |

### Multi-session dashboard (default, no args)

Discoverer scans `~/.claude/projects/**/*.jsonl`,
`~/.codex/sessions/**/*.jsonl`, `~/.minimax/**/*.jsonl`, and
`~/.gemini/**/*.jsonl`, picks the 32 most-recent, and renders them sorted
with the most-recent first. Each session is auto-classified by path:

| Path prefix                                      | Agent kind  |
|--------------------------------------------------|-------------|
| `~/.claude/projects/**/*.jsonl`                   | 🤖 claude-code |
| `~/.codex/sessions/**/*.jsonl`                    | ⚒ codex        |
| `~/.minimax/**/*.jsonl`                          | ✦ minimax      |
| `~/.gemini/**/*.jsonl`                           | ◆ gemini       |
| anything else                                    | ? unknown     |

Per session row:

- **Agent icon** + **agent label** (color-coded)
- **Worktree** — decoded from the Claude Code project-path encoding, or
  the jsonl's parent dir for other agents
- **Branch** — `git -C <worktree> rev-parse --abbrev-ref HEAD` (cached)
- **Task** — first user message in the session
- **Node count** + **last-modified** timestamp

The focused session (▶) shows its execution graph inline:
**user → assistant text → tool call → tool result → assistant text → ...**
with status glyphs ✓ (done) · … (pending) · ✗ (failed).

### Single-session viewer (with a path)

When you pass a path, the binary skips discovery and renders one session
in a tall list view:

```
$ agent-graph-tui ~/.claude/projects/foo/abc.jsonl

agent-graph-tui /Users/you/.claude/projects/foo/abc.jsonl
─────────────────────────────────────────────────────────────────
10:00:00 user        "fix the bug"
10:00:01 assistant   "looking at parser.rs..."
10:00:02 tool        Read ./src/parser.rs                ✓
10:00:02     └─ result  <240 bytes>                       ✓
10:00:03 tool        Edit ./src/parser.rs                ✓
10:00:03     └─ result  ok                                ✓
10:00:04 assistant   "ship it"

 3/6   ↑↓ navigate   q quit
```

### Plain-text mode (piped / non-TTY)

When stdout is not a TTY, the binary prints a plain-text snapshot and
exits — no raw mode, no interactive rendering. Useful in CI logs:

```sh
agent-graph-tui | head -30
```

## Keys

### Dashboard mode

| Key     | Action                           |
|---------|----------------------------------|
| `j` / ↓ | Next session                     |
| `k` / ↑ | Previous session                 |
| `g`     | Jump to first session            |
| `G`     | Jump to last session             |
| `r`     | Force rediscovery                |
| `q` / ⎋ | Quit                              |

The focused session's execution graph auto-loads on selection and
live-tails at 100 ms / 30 fps; discovery reruns every 5 s.

### Single-session mode

| Key     | Action            |
|---------|-------------------|
| `j` / ↓ | next row          |
| `k` / ↑ | prev row          |
| `g`     | jump to top       |
| `G`     | jump to bottom    |
| `q` / ⎋ | quit              |

## What it does not do

- Spawn agents or capture input — read-only.
- Run on Windows. macOS + Linux only (uses `git` + `readdir`).
- Serialize state. Re-discovery on every launch.
- Render a true graph layout. The execution graph is a unicode tree
  (├─ / └─) — fast to render, no graph-layout dep.

## Performance

| Metric                              | Target  | Actual  |
|-------------------------------------|---------|---------|
| Cold start (text mode, 32 sessions) | ≤2 s    | ~1.1 s  |
| Cold start (single session, empty)  | ≤50 ms  | ~0.5 ms |
| Binary size                         | ≤5 MB   | 0.69 MB |
| Resident memory (idle)              | ≤20 MB  | ~10 MB  |
| Render at 1 k rows                  | ≤16 ms  | ~3 ms   |
| Cold start w/ 3,418 .jsonl files    | —       | 1.2 s   |

The 3,418 .jsonl cold-start case uses a two-phase discovery: cheap
metadata scan over all files, then a `git_branch` fork-exec only on the
top 32 survivors. Without that split, the first version took 43 s.

## Build

Requires Rust 1.75+ (edition 2021). Tested on macOS 14 with rustc 1.89.

## Tests

```sh
cargo test --release
```

18/18 unit tests passing:

- `parser::tests::*` — multi-block assistant, CRLF, file rotation, ring
  cap, malformed JSON, marker edges
- `tree::tests::*` — pending / done / failed, orphan results, extend + clear
- `discovery::tests::*` — agent kind classification, first user message
  extraction (string + content-array + escaped quotes), worktree decode

## License

Apache-2.0
