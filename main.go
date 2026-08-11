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

func parseSessionLine(line string) (session, bool) {
	parts := strings.Split(line, "|")
	if len(parts) != 4 {
		return session{}, false
	}
	if parts[0] == "" {
		return session{}, false
	}
	windows, err := strconv.Atoi(parts[1])
	if err != nil {
		return session{}, false
	}
	return session{
		name:     parts[0],
		windows:  windows,
		created:  parts[2],
		attached: parts[3],
	}, true
}

func loadSessions() ([]session, error) {
	out, err := exec.Command("tmux", "list-sessions", "-F",
		"#{session_name}|#{session_windows}|#{session_created_string}|#{session_attached}").Output()
	if err != nil {
		var exitErr *exec.ExitError
		if errors.As(err, &exitErr) && strings.Contains(string(exitErr.Stderr), "no server running") {
			return nil, fmt.Errorf("no tmux server running — start one with `tmux new -s name`")
		}
		return nil, err
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

func listWindows(sessionName string) []string {
	out, err := exec.Command("tmux", "list-windows", "-t", sessionName, "-F",
		"#{window_index} #{window_name} (#{window_panes} panes)").Output()
	if err != nil {
		return []string{"(unable to read windows)"}
	}
	lines := strings.Split(strings.TrimSpace(string(out)), "\n")
	if len(lines) == 1 && lines[0] == "" {
		return []string{"(no windows)"}
	}
	return lines
}

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
	list     list.Model
	detail   viewport.Model
	sessions []session
	width    int
	height   int
	err      error
}

var (
	titleStyle   = lipgloss.NewStyle().Bold(true).Foreground(lipgloss.Color("#7D56F4")).Padding(0, 1)
	helpStyle    = lipgloss.NewStyle().Foreground(lipgloss.Color("#626262")).Padding(0, 1)
	paneStyle    = lipgloss.NewStyle().Border(lipgloss.RoundedBorder()).BorderForeground(lipgloss.Color("#3C3C3C"))
	focusedStyle = lipgloss.NewStyle().Border(lipgloss.RoundedBorder()).BorderForeground(lipgloss.Color("#7D56F4"))
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
	m := model{list: l, sessions: sessions, detail: viewport.New(40, 20)}
	if len(sessions) > 0 {
		m.detail.SetContent(renderDetail(sessions[0]))
	}
	return m, nil
}

func switchClient(name string) error {
	return exec.Command("tmux", "switch-client", "-t", name).Run()
}

func killSession(name string) error {
	return exec.Command("tmux", "kill-session", "-t", name).Run()
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
				_ = switchClient(sel.name)
				return m, tea.Quit
			}
		case "K":
			if sel, ok := m.list.SelectedItem().(session); ok {
				if err := killSession(sel.name); err == nil {
					m.err = nil
					if reloaded, err := loadSessions(); err == nil {
						m.sessions = reloaded
						items := make([]list.Item, len(reloaded))
						for i, s := range reloaded {
							items[i] = s
						}
						m.list.SetItems(items)
						if len(reloaded) > 0 {
							m.detail.SetContent(renderDetail(reloaded[0]))
						}
					}
				} else {
					m.err = err
				}
			}
			return m, nil
		case "n":
			return m, tea.Quit
		}
	}
	var cmd tea.Cmd
	m.list, cmd = m.list.Update(msg)
	if sel, ok := m.list.SelectedItem().(session); ok {
		m.detail.SetContent(renderDetail(sel))
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
	return lipgloss.JoinVertical(lipgloss.Left,
		titleStyle.Render("tmux-tui"),
		lipgloss.JoinHorizontal(lipgloss.Top, left, right),
		help,
	)
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
