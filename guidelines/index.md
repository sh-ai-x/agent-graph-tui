# Guidelines

Language / framework conventions for `agent-graph-tui`.

## Go
- **Version:** 1.26 (toolchain `go1.26.5 darwin/arm64`)
- **Module path:** `github.com/sh-ai-x/agent-graph-tui`
- **Formatting:** `gofmt -s -w .` (run before commit)
- **Linting:** `go vet ./...`
- **Testing:** stdlib `testing` + `go test ./...`. Coverage not enforced at bootstrap stage.
- **Errors:** wrap with `fmt.Errorf("...: %w", err)`; use `errors.Is` / `errors.As` at boundaries.
- **Naming:** exported = CamelCase + doc comment; unexported = camelCase; receivers 1–2 letters.

## Bubble Tea
- Models implement `Init() tea.Cmd`, `Update(tea.Msg) (tea.Model, tea.Cmd)`, `View() string`.
- `Update` is pure — side effects only inside returned `tea.Cmd`.
- Layout via `lipgloss.JoinHorizontal` / `JoinVertical`; styles centralized in package-level vars.
- `tea.WithAltScreen()` for full-screen TUIs.

## tmux
- All shell-outs via `exec.Command("tmux", ...)`. Never shell-string concatenate user input.
- Format strings: `#{session_name}|#{session_windows}|#{session_created_string}|#{session_attached}`.
- Treat "no server running" stderr as a friendly user error, not a crash.

## Files
- `main.go` — entry point, model wiring, key bindings.
- Split into `internal/sessions/`, `internal/tui/`, `internal/tmux/` once >300 LOC.
