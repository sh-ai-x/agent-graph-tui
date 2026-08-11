# tmux-tui

Terminal UI for browsing and switching tmux sessions, built with [Bubble Tea](https://github.com/charmbracelet/bubbletea).

## Features

- Lists all running tmux sessions with window count and creation time
- Switches the active client to the selected session
- Kills sessions from the keyboard
- Renders window details in a side pane

## Requirements

- Go 1.26+
- A running tmux server (`tmux new -s name` to start one)

## Install

```sh
go install github.com/sh-ai-x/agent-graph-tui@latest
```

Or build locally:

```sh
go build -o bin/agent-graph-tui .
./bin/agent-graph-tui
```

## Keys

| Key       | Action            |
|-----------|-------------------|
| `enter`   | switch to session |
| `K`       | kill session      |
| `n`       | new (run `tmux new -s name` outside the TUI) |
| `q`       | quit              |

## Development

```sh
go vet ./...          # static analysis
go build ./...        # compile
go test -race ./...   # unit tests with race detector
gofmt -l .            # formatting check (empty = OK)
```

## Architecture

Single-file scaffold (`main.go`):

- `session` struct + `parseSessionLine` + `loadSessions`/`listWindows` tmux exec wrappers
- Bubble Tea `model` (list + viewport) with lipgloss styling
- Pure `Update`; side-effects through key handlers only

See `phases/tmux-tui-scaffold/` for the step-by-step plan + verification logs.
