# agent-graph-tui

Lightweight terminal viewer for Claude Code / Codex agent execution graphs.

Read-only. No daemon. Renders a session JSONL as a live-tail, scrollable tree of
`user` / `assistant text` / `tool call` / `tool result` nodes.

```
$ agent-graph-tui ~/.claude/projects/foo/abc.jsonl

agent-graph-tui /Users/you/.claude/projects/foo/abc.jsonl
─────────────────────────────────────────────────────────────────
user        "fix the bug"
assistant   "looking at parser.rs..."
tool        Read ./src/parser.rs                ✓
    └─ result  <240 bytes>                       ✓
tool        Edit ./src/parser.rs                ✓
    └─ result  ok                                ✓
assistant   "ship it"

 3/6   ↑↓ navigate   q quit
```

## Why

[herdr](https://herdr.dev/) runs an `herdr-server` daemon + client + pty layer
to multiplex agents, which is overkill when you only want to *see* one. This
is a single Rust binary with no runtime deps: cold-starts in ~20 ms and lives
in ~3–5 MB.

## Install

```sh
cargo install --path .
```

Or:

```sh
git clone https://github.com/sh-ai-x/agent-graph-tui
cd agent-graph-tui
cargo build --release
./target/release/agent-graph-tui
```

## Usage

```sh
agent-graph-tui                     # auto-detect most recent ~/.claude session
agent-graph-tui session.jsonl       # explicit path
agent-graph-tui < pipe-of-jsonl     # plain-text fallback (non-TTY)
```

## Keys

| Key     | Action            |
|---------|-------------------|
| `j` / ↓ | next row          |
| `k` / ↑ | prev row          |
| `g`     | jump to top       |
| `G`     | jump to bottom    |
| `q` / ⎋ | quit              |

## Build

Requires Rust 1.75+ (edition 2021). Tested on macOS 14 with rustc 1.89.

## License

Apache-2.0
