# AGENTS.md

Sub-agent / Codex hand-off contract for `agent-graph-tui`.

## Identity
- **Repo:** github.com/sh-ai-x/agent-graph-tui
- **Stack:** Go 1.26, Bubble Tea TUI
- **Build:** `go build -o bin/agent-graph-tui .`
- **Run:** `./bin/agent-graph-tui` (needs an active tmux server)

## Boundaries
- **MUST NOT** edit `main` directly. Use a worktree: `git worktree add -b <type>/<slug> .worktrees/<slug> origin/main`.
- **MUST NOT** modify `.dev-kit/.active-hooks.json` directly; it is the SSOT and managed by `lib/state_codec.py`.
- **MUST** run `go vet ./...` and `go build ./...` before declaring done.
- **MUST** add tests with any non-trivial change (`*_test.go` next to source).

## Conventions
- Imports grouped: stdlib / third-party / local (blank line between groups).
- Errors wrap with `%w`; use `errors.Is` / `errors.As` at boundaries.
- Bubble Tea models implement `Init()`, `Update(msg)`, `View()` — keep `Update` pure (no side effects outside `tea.Cmd`).
- All tmux shell-outs go through `exec.Command("tmux", ...)`; surface stderr when meaningful.

## Hand-off format
When finishing a sub-task, emit:
```
## done
- files: <changed paths>
- tests: <added paths> + `go test ./...` exit=0
- build: `go build ./...` exit=0
- next: <suggested follow-up or "none">
```
