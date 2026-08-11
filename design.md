# agent-graph-tui — design

Single-binary, terminal-resident viewer for AI-coding-agent execution graphs.
Read-only. No daemon. No electron. Cold-starts in tens of milliseconds, lives in <5 MB.

## Goals

1. **Render the execution graph** of a Claude Code / Codex session as a scrollable
   list of typed rows (user / assistant text / tool call / tool result).
2. **Live-tail** the session JSONL so the graph grows in real time.
3. **Be lighter and faster** than herdr (Rust agent multiplexer), which runs a
   `herdr-server` daemon + client + pty layer.

## Non-goals

- Running / spawning agents (we only read session logs).
- Sub-graph layouts (orthogonal routing, multi-pane splits).
- Mouse interaction beyond selection.
- Cross-platform polish beyond macOS + Linux (Windows is best-effort).

## Architecture

```
                    ┌────────────────┐
   JSONL file ───►  │  parser.rs     │ ──► Vec<Event>
                    └────────────────┘
                          │
                          ▼
                    ┌────────────────┐
                    │  tree.rs       │ ──► Session { rows, pending_tools }
                    └────────────────┘
                          │
                          ▼
   keyboard ───►  ┌────────────────┐   30 fps tick
                  │  app.rs loop   │ ───► ratatui Terminal<Crossterm>
                  └───────┬────────┘
                          ▼
                    ┌────────────────┐
                    │  render.rs     │ ──► terminal frame / plain text
                    └────────────────┘
```

Module boundaries are intentional:

- `parser` is pure: stream in, `Vec<Event>` out. No global state.
- `tree` is pure: `Vec<Event>` in, `Session` out. No I/O.
- `app` owns the poll loop, the rescan cursor, and the `Terminal`.
- `render` is the only place that touches ratatui or stdout.

This split keeps the cost of swapping any one layer bounded — e.g. swap `parser`
to tail Codex JSONL without touching `app` or `render`.

## Dependency choice

| Crate         | Why                                                |
|---------------|----------------------------------------------------|
| `ratatui`     | Active fork of `tui-rs`; no node / electron.       |
| `crossterm`   | Pure-Rust terminal backend, no ncurses linkage.     |
| `serde_json`  | Stream-parse JSONL one line at a time.             |

Nothing else. No `tokio`, no `notify`, no `clap`, no `anyhow`. Each avoided
crate = ~250 KB shaved off the binary and ~5 ms shaved off cold start.

Tail is a 100 ms poll loop against a byte-offset cursor on the file. We do
**not** fan out to threads; the JSONL parser is faster than line-by-line IO.

## Performance budget (measured on macOS 14, M-series, rustc 1.89)

| Metric          | Target  | Actual                  | herdr-style (server+client) | Bubble Tea (existing Go tmux-tui) |
|-----------------|---------|-------------------------|-----------------------------|------------------------------------|
| Cold start      | ≤50 ms  | **0.45 ms**             | ~600 ms (server+client)     | ~1.2 s                           |
| Cold start 5k JSONL | –    | **11 ms**               | n/a                         | n/a                              |
| Binary size     | ≤5 MB   | **0.64 MB**             | ~25 MB                      | 4.9 MB                           |
| Render 5k rows  | ≤16 ms  | **<10 ms**              | n/a                         | ~50 ms                           |
| Resident memory | ≤20 MB  | tbd (TUI idle)          | ~80 MB                      | ~25 MB                           |

Bench procedure:

```sh
cargo build --release
time ./target/release/agent-graph-tui --help 2>/dev/null  # cold-start proxy
ls -la target/release/agent-graph-tui                       # size gate
/usr/bin/time -l ./target/release/agent-graph-tui          # RSS
```

## Data model

```rust
pub enum NodeKind {
    UserText(String),
    AssistantText(String),
    ToolCall   { name: String, input: String },
    ToolResult(String),
    Unknown(String),
}

pub struct Node {
    pub depth:  usize,
    pub kind:   NodeKind,
    pub status: Status,        // Pending | Done | Failed
    pub ts:     String,        // ISO-8601 from JSONL
}
```

`pending_tools: HashMap<tool_use_id, row_idx>` resolves a `tool_use` row's
status when its matching `tool_result` lands.

## Rendering

Hand-rolled unicode tree (`├─` / `└─`), no graph-layout dep. Each row shows:

```
user       "fix the bug"
assistant  "looking at parser.rs..."
tool       Read ./src/parser.rs       ✓
    └─ result  <240 bytes>             ✓
```

30 fps redraw budget, gated by `last_draw.elapsed()` so a quiet tail costs ~0.

## Future work

- Side-by-side diff of two sessions
- Status filter (hide `Done` rows)
- Export to mermaid
- Optional `notify`-based FSEvents backend (only if poll proves janky on Linux)
