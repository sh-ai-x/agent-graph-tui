# Changelog

All notable changes to tmux-tui are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0] - 2026-08-11

### Added
- Initial scaffold: Go TUI for browsing and switching tmux sessions
- `session` data model + `parseSessionLine` (rejects empty / leading-dash /
  negative-window inputs)
- `loadSessions` / `listWindows` / `switchClient` / `killSession` tmux exec
  wrappers using `CombinedOutput` so stderr surfaces in error messages
- Bubble Tea `model` with `list.Model` (sessions) + `viewport.Model` (detail)
- lipgloss styles: title (#7D56F4 bold), help (#626262 dim), pane border
  (#3C3C3C), focused border (#7D56F4), error (#FF5555)
- Keys: `enter` switch (errors surfaced, not silent), `K` kill + reload,
  `n` quit, `q` / `ctrl+c` quit
- Pure `Update`: detail refresh gated by `lastDetail` index — no tmux exec
  per keystroke
- View renders `m.err` as a top-of-frame error line
- Unit tests for `parseSessionLine` (7 cases incl. pipe-in-name limitation)
- GitHub Actions CI: vet + build + race-test + gofmt on PR
- `/dev-kit:review` (3-dim) + `/dev-kit:security` (10-dim OWASP) on PR
- `/dev-kit:auto-fix-pr` bounded repair wave on review changes
- Pre-push hook blocking direct push to main
- README + .gitignore + bin/ exclusion

### Known Limitations (v1)
- Session names containing `|` are rejected by the parser; v2 should switch
  the format string to a separator tmux forbids (`:`) and update the parser
- tmux exec is synchronous in key handlers; v2 should use `tea.Cmd` to keep
  the UI thread responsive under slow tmux servers
- `main.go` is single-file (~264 lines); v2 should split into `internal/tmux/`
  and `internal/tui/` packages

### Acknowledgements
- Plan + phases in `phases/tmux-tui-scaffold/`
- Design record at `docs/proposals/agent-graph-tui/tmux-tui-scaffold.html`
- Built with [Bubble Tea](https://github.com/charmbracelet/bubbletea) +
  [Bubbles](https://github.com/charmbracelet/bubbles) +
  [Lip Gloss](https://github.com/charmbracelet/lipgloss)
