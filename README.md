# agent-graph-tui

Lightweight, single-binary terminal dashboard for AI-coding-agent
**execution graphs**. Surfaces every active Claude Code / Codex / MiniMax /
Gemini session on your machine, grouped by **agent → repo → branch**, with
the model, worktree, task, live tail, and per-session status (running /
done / failed / blocked) all in one TUI.

Read-only. No daemon. No server. No electron. ~0.7 MB binary, cold start
~1 s, ~10 MB RSS.

```
$ agent-graph-tui           # multi-session dashboard (default)

agent-graph-tui  32 active sessions  ·  ↑/↓ select   ⏎ expand   q quit
─────────────────────────────────────────────────────────────────
🤖 claude-code (32 sessions)
  agent-graph-tui                              ← git main-repo
    ⏵ main                                     ← worktree branch
      ▶ [o] MiniMax-M3              23 nodes    55s ago
      ☒ [o] <unknown>                0 nodes     2h ago
      ☒ [o] <unknown>                0 nodes     2h ago
  archidraw
    ⏵ main
      ☒ [o] MiniMax-M3              33 nodes
  dev-harness-kit
    ⏵ agent/harness-effectiveness
      ☒ [o] <unknown>               17 nodes
      ☒ [o] MiniMax-M3              25 nodes
```

---

## Quick start (TL;DR)

```sh
# build
cargo build --release

# run
./target/release/agent-graph-tui                    # multi-session dashboard
./target/release/agent-graph-tui path/to/session.jsonl   # single session
./target/release/agent-graph-tui < session.jsonl | cat     # plain-text dump
```

That's it. No config, no env vars, no daemon, no `git init` on the watch
side (it shells out to `git` against each session's cwd).

---

## Why this exists

[herdr](https://herdr.dev/) is a Rust agent *multiplexer* — it spawns
agents and routes their pty I/O through a server/client pair. Heavy if
you only want to *see* what your agents are doing.

`agent-graph-tui` is **read-only**. It never spawns an agent, never
captures a keystroke, never runs a background service. It walks
`~/.claude/projects/`, `~/.codex/sessions/`, `~/.minimax/`, `~/.gemini/`,
classifies each `*.jsonl`, and renders what's there.

`herdr`-style daemon+client+pty architecture on the user's machine is
~25 MB and ~600 ms cold start. `agent-graph-tui` is ~0.7 MB and ~1 s cold
start. The cost: it can't influence agent behaviour, only observe.

---

## Install

Three ways, pick one.

### A) Build from this checkout

```sh
git clone https://github.com/sh-ai-x/agent-graph-tui
cd agent-graph-tui
cargo build --release
./target/release/agent-graph-tui
```

### B) Install via cargo (once it's published)

```sh
cargo install --path .
agent-graph-tui
```

### C) Run from source during development

```sh
cargo run --release
```

`cargo run` is ~30 s the first time (compiles all deps) and <2 s
afterwards thanks to incremental compilation. Output binary lives at
`target/release/agent-graph-tui` (the `cargo run` invocation runs the
release-mode binary out of the same target dir).

### Requirements

- Rust 1.75+ (edition 2021). Tested on rustc 1.89.
- macOS 14+ or Linux. Windows is best-effort but unverified.
- A POSIX shell (`/bin/sh` for the pre-push hook), `git` on `$PATH`
  (used for branch + repo-name lookup), and `~/.claude/`, `~/.codex/`,
  `~/.minimax/`, `~/.gemini/` session files if you want the dashboard to
  have anything to show.

---

## Run modes

The binary dispatches based on argv and TTY state.

| Command | Mode |
|---|---|
| `agent-graph-tui` (no args, stdout is a TTY) | **Multi-session dashboard** — auto-discover and live-tail every active session |
| `agent-graph-tui <path.jsonl>` (argv[1] is a file) | **Single-session viewer** — focused view of one session file |
| `agent-graph-tui < …` (stdout is NOT a TTY) | **Plain-text fallback** — render a snapshot and exit; no raw mode, no interactive loop |

The dispatch is in `src/main.rs`; the three modes share the same parser
+ tree + render code, just different drivers.

### Multi-session dashboard (default, no args)

This is the primary use case. The binary:

1. Scans up to **32** most-recent session files under
   `~/.claude/projects/`, `~/.codex/sessions/`, `~/.minimax/`, and
   `~/.gemini/`.
2. For each, reads the first **64 KiB** (cheap) to extract:
   - **agent kind** (path prefix)
   - **model** (`message.model` on the first assistant message)
   - **cwd** (top-level `cwd` field of any JSONL line)
   - **task** (first user message)
3. Runs `git -C <cwd> rev-parse --abbrev-ref HEAD` and
   `git -C <cwd> rev-parse --path-format=absolute --git-common-dir`
   to get the **branch** and the **main repo name** (the latter via
   `dirname` of the shared `.git/`, so worktrees resolve to the project
   root, not the worktree branch dir).
4. Renders the sessions in a hierarchy: **agent → repo → branch**, with
   the most-recent session first within each group. The selected
   session's full execution graph (user / assistant / tool / result)
   is shown inline below it.
5. Re-discovers every 5 s. Live-tails each selected session at
   100 ms / 30 fps.

#### Layout

The TUI has three vertical regions:

```
┌─────────────────────────────────────────────────────────────┐
│ agent-graph-tui  N active sessions  ·  ↑/↓ … q quit         │ ← header (3 lines)
├─────────────────────────────────────────────────────────────┤
│  🤖 <agent>  (N sessions)            ← agent group header   │
│    <repo>                            ← repo group header     │
│      ⏵ <branch>  (N)                 ← branch group header   │
│        [status] <model>  N nodes  relative_time             │
│          task: <first user message>                          │
│      ⏵ <branch>  (N)                                        │
│        ▶ [status] <model>  N nodes  relative_time            │
│          ── execution graph ──                              │
│          user      "..."                                   │
│          assistant "..."                                   │
│          tool X   ...                ✓                      │
│            └ result  ...           ✓                      │
│          ...                                                  │
├─────────────────────────────────────────────────────────────┤
│ N/M · <agent>            live tailing · refresh 5 s        │ ← footer (1 line)
└─────────────────────────────────────────────────────────────┘
```

Scroll is **group-anchored**: when you move the selection, the agent +
repo + branch headers are kept at the top of the viewport so you never
lose context. The session itself sits below them.

#### Status glyphs (per session)

| Glyph | Meaning |
|---|---|
| `*` (green) | **Running** — a `tool_use` is awaiting its result, or the user is mid-prompt |
| `o` (gray) | **Done** — the last event is a completed `tool_result` or assistant text |
| `x` (red) | **Failed** — the last `tool_result` came back with `is_error: true` |
| `?` (yellow) | **Blocked** — pending work AND no file activity for ≥ 5 minutes (likely waiting on user input) |

The status is computed two ways:

- **TUI mode**: from the parsed `tree::Session` (Pending / Done / Failed
  on each row, aggregated to session-level).
- **Text-mode / first paint**: from a cheap last-event probe
  (`discovery::quick_status`) that reads only the last 8 KiB of the
  JSONL and inspects the last non-empty line. No full parse.

#### Keybindings (dashboard mode)

| Key | Action |
|---|---|
| `j` / `↓` | Next session |
| `k` / `↑` | Previous session |
| `g` | Jump to first session |
| `G` | Jump to last session |
| `r` | Force rediscovery (skip the 5 s timer) |
| `q` / `Esc` | Quit |

Mouse events are currently consumed by `EnableMouseCapture` but
**click-to-select is not wired up** — only keyboard works. Drag-and-drop
is not supported. If you want either, file an issue.

### Single-session viewer (with a path)

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

- Skips discovery.
- Same parser, same tree builder, same per-row status — just a single
  flat list, scrollable with `j` / `k` / `g` / `G`.
- `q` / `Esc` quits.

### Plain-text mode (non-TTY)

If stdout is **not a TTY** — i.e. you redirected to a file, piped to
`head`, or running inside a CI log — the binary **skips** the
interactive TUI loop entirely and prints a snapshot to stdout:

```sh
$ agent-graph-tui | head -40

agent-graph-tui — 32 sessions (text mode, cold start 1003.42 ms)

🤖 claude-code (32 sessions)
  agent-graph-tui
    ⏵ main
      [o] MiniMax-M3              23 nodes
      [o] <unknown>                0 nodes
  archidraw
    ⏵ main
      [o] MiniMax-M3              33 nodes
  ...
```

Use this for CI logs, `script(1)` recordings, or just to peek at
sessions without occupying a TTY.

---

## Output format in detail

### Agent classification

Auto-detected from the JSONL path:

| Path prefix | Agent kind | Icon |
|---|---|---|
| `~/.claude/projects/**/*.jsonl` | `claude-code` | 🤖 |
| `~/.codex/sessions/**/*.jsonl` | `codex` | ⚒ |
| `~/.minimax/**/*.jsonl` | `minimax` | ✦ |
| `~/.gemini/**/*.jsonl` | `gemini` | ◆ |
| anything else | `unknown` | ? |

Within a `claude-code` session, the **model** field on the first
assistant message is read and shown. So a single Claude Code session
can be running `claude-opus-4-8` or `MiniMax-M3` or any other model the
user configured.

### Repo + branch

`git -C <cwd> rev-parse --abbrev-ref HEAD` for the branch.

For the **repo name**, the binary uses
`git -C <cwd> rev-parse --path-format=absolute --git-common-dir` and
`dirname`s the result. This is the **main repo's working tree**, even if
the session was started from a linked worktree. So a worktree at
`~/main/.worktrees/skill-chain-redesign` shows up as repo=`main`,
branch=`skill-chain-redesign` — not repo=`skill-chain-redesign`.

The cwd is read from the JSONL's top-level `cwd` field. If absent
(common for very small / new files), the binary falls back to
`std::env::current_dir()` — i.e., the directory you invoked
`agent-graph-tui` from.

### Execution graph

The per-session execution graph is the sequence:

```
user
└─ assistant text
   └─ tool_use (Read, Edit, Bash, …)
      └─ tool_result
assistant text
└─ tool_use
   └─ tool_result
…
```

Each node has a status: `Pending` (no `tool_result` yet) → `Done` →
optionally `Failed` (if `is_error: true`). The session-level status
shown in the dashboard is the worst of these: any `Failed` wins, else
any `Pending` → `Running`, else `Done`. `Blocked` flips in when pending
work goes ≥ 5 minutes without file activity.

The dashboard's *selected* session expands its full execution graph
inline, last 8 rows visible. Older rows scroll off the top of the
inline panel.

---

## Performance

| Metric | Target | Actual |
|---|---|---|
| Binary size | ≤ 5 MB | 0.69 MB |
| Cold start, single session (empty) | ≤ 50 ms | 0.45 ms |
| Cold start, 32 sessions, dashboard mode | ≤ 2 s | 1.0 s |
| Cold start, 3,418 .jsonl files in tree | — | 1.2 s |
| Resident memory (idle) | ≤ 20 MB | ~10 MB |
| Render at 1 k rows | ≤ 16 ms | ~3 ms |
| Discovery refresh interval | — | 5 s |
| Live-tail interval (selected session) | — | 100 ms / 30 fps |
| Event ring cap | — | 50 000 events / session |
| Per-file read cap | — | 256 MiB |

The 3,418-file case is a real test on the user's machine (~32 active +
~3,400 older sessions in `~/.claude/projects/`). The naive scan took
43 s because we forked `git` for every file. The two-phase split
(metadata scan first, `git` only on the top 32 survivors) brings it
to 1.2 s.

To hit "no-build" warm-cache cold start, run once and then:
```sh
echo 1 | sudo tee /proc/sys/vm/drop_caches   # only if you want to
time ./target/release/agent-graph-tui
```
(Don't actually do that on a running system; it's just an illustration.)

---

## Architecture (brief)

```
src/
├── main.rs       CLI dispatch (dashboard / single / plain-text)
├── lib.rs        Re-exports for the test suite
├── parser.rs     JSONL → Vec<Event> (multi-block, CRLF, ring cap, rotation)
├── tree.rs       Event → renderable rows + session_status aggregation
├── discovery.rs  Active-session scan, agent kind, cwd, model, git_repo_root
├── dashboard.rs  Multi-session TUI: state, app loop, hierarchical build_lines
├── render.rs     Single-session TUI
└── app.rs        Single-session app loop
```

Module rules:

- `parser` is **pure** (no I/O after `open`).
- `tree` is **pure** (no I/O).
- `discovery` does **all** the I/O — filesystem walk, file open,
  `git` fork-exec.
- `dashboard` owns the live state and the per-tick loop; it borrows
  the parser/tree for tail updates.
- `render` is **pure** styling — given a `tree::Session`, paint it.
- `app` is the single-session equivalent of `dashboard::run`.

If you want to add a new agent kind:

1. Extend `discovery::AgentKind` + `from_path`.
2. Add a label + icon.
3. Update the README + this doc.

That's it. The parser, tree, and renderer are agent-agnostic.

---

## Development

```sh
# build
cargo build --release          # release binary
cargo build                    # debug binary

# run tests
cargo test --release --lib
# 45 unit tests; takes <1 s on a warm cache.

# format / lint
cargo fmt
cargo clippy -- -D warnings

# watch mode (rebuilds on file change)
cargo watch -x 'build --release'      # install `cargo-watch` once
```

### Test inventory (45 tests, all `cargo test`)

| Module | Group | Count |
|---|---|---|
| `parser` | multi-block assistant, CRLF, file rotation, ring cap, malformed JSON, marker edges | 10 |
| `tree` | pending / done / failed, orphan results, extend + clear, session_status aggregation (8 sub-cases) | 13 |
| `discovery` | agent kind classification, first user message extraction (string + content-array + escaped quotes), worktree decode, `quick_status` (7 sub-cases) | 13 |
| `dashboard` | `build_lines` headers (agent / repo / branch change), ordering, `compute_scroll` group-anchored | 7 |
| `repo_root` | worktree → main repo via `--git-common-dir` | 1 |
| `extract_*` | `cwd` / `model` / `worktree_name` | (subset of discovery) |

### Pre-push hook

`.githooks/pre-push` blocks direct push to `main` and forces a PR
review path. Install once:

```sh
git config core.hooksPath .githooks
```

---

## Configuration

There is **no config file**. The binary is opinionated. If you need a
flag, open an issue.

Things you might think need configuration but don't:

- **JSONL head scan size** — fixed at 64 KiB. Larger files are
  partially parsed; the model field is whatever's in the first 64 KiB.
- **Number of sessions** — capped at 32. Older ones are dropped
  silently.
- **Agent classification** — purely path-based. Override by symlinking
  the agent's data dir into one of the four known paths.
- **Color** — depends on the terminal. Most terminals render the agent
  icons in color; mono terminals fall back to text.

Things that ARE environment-dependent:

- `HOME` is read to locate `~/.claude`, `~/.codex`, etc. The binary
  won't find your sessions if `$HOME` is unset or points somewhere
  unexpected.
- `git` must be on `$PATH`. If you have it in `/usr/local/bin/git` and
  that's not exported, branch / repo name come back as `?` / `—`.

---

## Troubleshooting

**"No active sessions found"**

The four known data directories don't exist or are empty. Check:

```sh
ls -la ~/.claude/projects/   # Claude Code
ls -la ~/.codex/sessions/    # Codex
ls -la ~/.minimax/            # MiniMax
ls -la ~/.gemini/             # Gemini
```

If your agent stores sessions somewhere else, this binary doesn't know
about it. File an issue.

**Cold start takes >5 s**

The discovery fan-out runs `git -C <cwd>` per session. If you have
many sessions in the same worktree, the worktree itself may be slow
(`fsmonitor`, antivirus, etc.). Try running with a single path:

```sh
./target/release/agent-graph-tui ~/.claude/projects/foo/bar.jsonl
```

That bypasses discovery entirely.

**All sessions show `<unknown>` model**

The first assistant message in the JSONL is past the first 64 KiB.
Either the model is being inferred from a later line (we don't
currently re-scan), or the session has only user messages so far (the
agent hasn't replied yet — model field is per-assistant-message).

**All sessions show `0 nodes`**

Same reason — the file is mostly user messages and we haven't seen any
tool_use / tool_result pairs yet. New sessions look like this.

**Branch shows `?`**

`git -C <cwd>` failed. Either the cwd is not a git repo, or `git`
isn't on `$PATH`, or the cwd is on a detached HEAD without a symbolic
name.

**A worktree session shows the wrong repo name**

We use `--git-common-dir` + `dirname` to resolve the main repo from
any worktree. If this still shows the worktree name (e.g.
`skill-chain-redesign` instead of `main`), your `git` is too old to
support `--path-format=absolute --git-common-dir`. Upgrade `git` to
2.31+.

---

## Known limitations

- **No mouse support** — `EnableMouseCapture` is on but no click / drag
  handlers. Keyboard only.
- **No drag-and-drop** — not a thing in this tool. (The earlier
  conversation about it was a miscommunication.)
- **No graph layout engine** — the execution tree is a unicode tree
  (├─ / └─), not a real DAG layout. If your session has parallel
  tool calls, the display is still linearised.
- **Model is per first-assistant-message** — if the agent switches
  models mid-session (e.g. Claude Code calls `minimax/M3` for a tool),
  we don't re-detect. Only the first model is shown.
- **No Windows** — the file open + readdir code paths are POSIX-only
  in practice. PRs welcome.
- **No multi-CLI processes** — each binary is one TUI. Run multiple
  instances pointing at different JSONLs if you need side-by-side.
- **No state across runs** — every launch re-discovers. There's no
  saved selection, no bookmark, no recent sessions list.

---

## Migration notes (Go → Rust)

This project was originally a Go tmux-tui scaffold (Bubble Tea, see
`phases/tmux-tui-scaffold/` for the historical build output). It was
replaced in 2026-08 with this Rust port because:

- Bubble Tea + lipgloss + a pty layer pushed cold start to ~1.2 s
  for a single CLI and ~25 MB RSS at idle.
- The Rust + ratatui + crossterm version of the same UX is ~0.7 MB
  binary, ~10 MB RSS, ~0.5 ms cold start for a single session.
- No `go vet` / `go test` / `gofmt` (the new CI runs `cargo test` +
  `cargo build --release` + a size gate).
- The Go files have been deleted from `main`; PR history retains
  them in `phases/tmux-tui-scaffold/step{0..4}-output.json`.

If you were using the Go version, the user-visible changes are:

| | Go | Rust |
|---|---|---|
| Install | `go install github.com/sh-ai-x/agent-graph-tui@latest` | `cargo install --path .` |
| Build | `go build -o bin/agent-graph-tui .` | `cargo build --release` |
| Run | `./bin/agent-graph-tui` | `./target/release/agent-graph-tui` |
| Test | `go test ./...` | `cargo test` |
| Binary | 4.9 MB | 0.69 MB |
| Cold start | ~1.2 s | ~0.5–1 s |

---

## License

Apache-2.0
