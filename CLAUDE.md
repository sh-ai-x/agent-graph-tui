# CLAUDE.md

> Lazy-loading index for `agent-graph-tui`. Read on demand; nothing inlined.

## Project
- **Name:** agent-graph-tui
- **Type:** Go CLI / TUI (Bubble Tea)
- **Stack:** Go 1.26, charmbracelet/bubbletea + bubbles + lipgloss, tmux shell-out
- **Branch:** `chore/bootstrap` (this worktree); canonical branch = `main`

## Read these on demand
- `docs/CODEBASE-MAP.md` — tree, manifest, deps, conventions
- `iron-laws/index.md` — non-negotiable workflow rules (MUST-*, MUST-NOT-*)
- `guidelines/index.md` — language/framework conventions
- `hooks/index.md` — active hooks per stage (SSOT = `.dev-kit/.active-hooks.json`)

## Active hooks (bootstrap stage)
All hooks **OFF** during bootstrap. See `hooks/index.md` for per-stage matrix.

## Quick start
```sh
go mod init github.com/sh-ai-x/agent-graph-tui
go get github.com/charmbracelet/bubbletea@latest \
       github.com/charmbracelet/bubbles@latest \
       github.com/charmbracelet/lipgloss@latest
go build -o bin/agent-graph-tui .
./bin/agent-graph-tui   # requires `tmux new -s name` first
```

## Next stage
- `/dev-kit:build <first-feature>` — canonical plan → build loop
- `/dev-kit:ci-setup --force` — add CI templates (only if skipped at bootstrap)
