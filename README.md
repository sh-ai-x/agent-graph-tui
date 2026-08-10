# agent-graph-tui

Terminal UI for browsing and switching tmux sessions, built with [Bubble Tea](https://github.com/charmbracelet/bubbletea).

## Features

- Lists all running tmux sessions with window count and creation time
- Switches the active client to the selected session
- Kills sessions from the keyboard
- Renders window details in a side pane

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

| Key     | Action            |
|---------|-------------------|
| `enter` | switch to session |
| `K`     | kill session      |
| `n`     | new session       |
| `q`     | quit              |
