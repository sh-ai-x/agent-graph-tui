// Package main provides tmux-tui: a terminal UI for browsing and switching
// tmux sessions. See README.md for usage.
package main

import (
	"errors"
	"fmt"
	"os"
	"os/exec"
	"strconv"
	"strings"

	"github.com/charmbracelet/bubbles/list"
	"github.com/charmbracelet/bubbles/viewport"
	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/lipgloss"
)

type session struct {
	name     string
	windows  int
	created  string
	attached string
}

func (s session) Title() string       { return s.name }
func (s session) Description() string { return fmt.Sprintf("%d windows · %s", s.windows, s.created) }
func (s session) FilterValue() string { return s.name }

// parseSessionLine parses one pipe-delimited tmux session line of the form
// "name|windows|created|attached". Returns ok=false on malformed input.
func parseSessionLine(line string) (session, bool) {
	parts := strings.SplitN(line, "|", 4)
	if len(parts) != 4 {
		return session{}, false
	}
	if parts[0] == "" || parts[0][0] == '-' {
		return session{}, false
	}
	windows, err := strconv.Atoi(parts[1])
	if err != nil || windows < 0 {
		return session{}, false
	}
	return session{
		name:     parts[0],
		windows:  windows,
		created:  parts[2],
		attached: parts[3],
	}, true
}

// loadSessions shells out to `tmux list-sessions` and parses each line.
// Returns a user-facing error when no tmux server is running.
func loadSessions() ([]session, error) {
	out, err := exec.Command("tmux", "list-sessions", "-F",
		"#{session_name}|#{session_windows}|#{session_created_string}|#{session_attached}").CombinedOutput()
	if err != nil {
		var exitErr *exec.ExitError
		if errors.As(err, &exitErr) && exitErr.ExitCode() == 1 {
			return nil, fmt.Errorf("no tmux server running — start one with `tmux new -s name`")
		}
		return nil, fmt.Errorf("tmux list-sessions: %s: %w", strings.TrimSpace(string(out)), err)
	}
	var sessions []session
	for _, line := range strings.Split(strings.TrimSpace(string(out)), "\n") {
		if line == "" {
			continue
		}
		if s, ok := parseSessionLine(line); ok {
			sessions = append(sessions, s)
		}
	}
	return sessions, nil
}

// listWindows shells out to `tmux list-windows` for the given session.
// Returns placeholder text on error or when the session has no windows.
func listWindows(sessionName string) []string {
	out, err := exec.Command("tmux", "list-windows", "-t", sessionName, "-F",
		"#{window_index} #{window_name} (#{window_panes} panes)").CombinedOutput()
	if err != nil {
		return []string{fmt.Sprintf("(unable to read windows: %s)", strings.TrimSpace(string(out)))}
	}
	lines := strings.Split(strings.TrimSpace(string(out)), "\n")
	if len(lines) == 1 && lines[0] == "" {
		return []string{"(no windows)"}
	}
	return lines
}

// renderDetail formats a session's detail pane. listWindows is intentionally
// called here; callers must gate by selection change to avoid spawning tmux
// on every event.
func renderDetail(s session) string {
	var b strings.Builder
	fmt.Fprintf(&b, "Name:     %s\n", s.name)
	fmt.Fprintf(&b, "Windows:  %d\n", s.windows)
	fmt.Fprintf(&b, "Created:  %s\n", s.created)
	fmt.Fprintf(&b, "Attached: %s\n", s.attached)
	b.WriteString("\nWindows\n")
	for _, line := range listWindows(s.name) {
		b.WriteString("  • ")
		b.WriteString(line)
		b.WriteByte('\n')
	}
	return b.String()
}

type model struct {
	list       list.Model
	detail     viewport.Model
	sessions   []session
	width      int
	height     int
	err        error
	lastDetail int // index of the session whose detail was last rendered; -1 = none
}

var (
	titleStyle   = lipgloss.NewStyle().Bold(true).Foreground(lipgloss.Color("#7D56F4")).Padding(0, 1)
	helpStyle    = lipgloss.NewStyle().Foreground(lipgloss.Color("#626262")).Padding(0, 1)
	paneStyle    = lipgloss.NewStyle().Border(lipgloss.RoundedBorder()).BorderForeground(lipgloss.Color("#3C3C3C"))
	focusedStyle = lipgloss.NewStyle().Border(lipgloss.RoundedBorder()).BorderForeground(lipgloss.Color("#7D56F4"))
	errorStyle   = lipgloss.NewStyle().Foreground(lipgloss.Color("#FF5555")).Padding(0, 1)
)

func initialModel() (model, error) {
	sessions, err := loadSessions()
	if err != nil {
		return model{}, err
	}
	items := make([]list.Item, len(sessions))
	for i, s := range sessions {
		items[i] = s
	}
	l := list.New(items, list.NewDefaultDelegate(), 30, 20)
	l.Title = "tmux sessions"
	l.SetShowStatusBar(false)
	m := model{list: l, sessions: sessions, detail: viewport.New(40, 20), lastDetail: -1}
	return m, nil
}

func switchClient(name string) error {
	out, err := exec.Command("tmux", "switch-client", "-t", name).CombinedOutput()
	if err != nil {
		return fmt.Errorf("tmux switch-client: %s: %w", strings.TrimSpace(string(out)), err)
	}
	return nil
}

func killSession(name string) error {
	out, err := exec.Command("tmux", "kill-session", "-t", name).CombinedOutput()
	if err != nil {
		return fmt.Errorf("tmux kill-session: %s: %w", strings.TrimSpace(string(out)), err)
	}
	return nil
}

func (m model) Init() tea.Cmd { return nil }

func (m model) Update(msg tea.Msg) (tea.Model, tea.Cmd) {
	switch msg := msg.(type) {
	case tea.WindowSizeMsg:
		m.width, m.height = msg.Width, msg.Height
		leftW := m.width / 3
		if leftW < 24 {
			leftW = 24
		}
		rightW := m.width - leftW - 2
		if rightW < 0 {
			rightW = 0
		}
		if m.height < 4 {
			m.height = 4
		}
		m.list.SetSize(leftW, m.height-4)
		m.detail.Width = rightW
		m.detail.Height = m.height - 4
		return m, nil
	case tea.KeyMsg:
		switch msg.String() {
		case "q", "ctrl+c":
			return m, tea.Quit
		case "enter":
			if sel, ok := m.list.SelectedItem().(session); ok {
				if err := switchClient(sel.name); err != nil {
					m.err = err
					return m, nil
				}
				return m, tea.Quit
			}
		case "K":
			if sel, ok := m.list.SelectedItem().(session); ok {
				if err := killSession(sel.name); err != nil {
					m.err = err
					return m, nil
				}
				if reloaded, err := loadSessions(); err != nil {
					m.err = err
					return m, nil
				} else {
					m.sessions = reloaded
					items := make([]list.Item, len(reloaded))
					for i, s := range reloaded {
						items[i] = s
					}
					m.list.SetItems(items)
					if len(reloaded) > 0 {
						m.detail.SetContent(renderDetail(reloaded[0]))
						m.lastDetail = 0
					}
				}
			}
			return m, nil
		case "n":
			return m, tea.Quit
		}
	}
	var cmd tea.Cmd
	m.list, cmd = m.list.Update(msg)
	// Only refresh the detail pane when the selected index actually changes.
	// Calling renderDetail → listWindows on every event would spawn a tmux
	// subprocess per keystroke / mouse move / tick; refresh only on selection
	// change (fixes the Bubble-Tea-purity critical finding from /dev-kit:review).
	idx := m.list.Index()
	if idx >= 0 && idx != m.lastDetail {
		if idx < len(m.sessions) {
			m.detail.SetContent(renderDetail(m.sessions[idx]))
		}
		m.lastDetail = idx
	}
	return m, cmd
}

func (m model) View() string {
	if m.width == 0 {
		return "loading…"
	}
	left := focusedStyle.Render(m.list.View())
	right := focusedStyle.Render(m.detail.View())
	help := helpStyle.Render("enter switch · K kill · n new · q quit")
	body := lipgloss.JoinVertical(lipgloss.Left,
		titleStyle.Render("tmux-tui"),
		lipgloss.JoinHorizontal(lipgloss.Top, left, right),
		help,
	)
	if m.err != nil {
		body = errorStyle.Render("error: "+m.err.Error()) + "\n" + body
	}
	return body
}

func main() {
	m, err := initialModel()
	if err != nil {
		fmt.Fprintln(os.Stderr, "tmux-tui:", err)
		os.Exit(1)
	}
	p := tea.NewProgram(m, tea.WithAltScreen())
	if _, err := p.Run(); err != nil {
		fmt.Fprintln(os.Stderr, "tmux-tui:", err)
		os.Exit(1)
	}
}
