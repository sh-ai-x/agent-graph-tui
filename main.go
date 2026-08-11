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
)

type session struct {
	name     string
	windows  int
	created  string
	attached string
}

// parseSessionLine parses one pipe-delimited tmux session line of the form
// "name|windows|created|attached". Returns ok=false on malformed input.
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

// loadSessions shells out to `tmux list-sessions` and parses each line.
// Returns a user-facing error when no tmux server is running.
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

// listWindows shells out to `tmux list-windows` for the given session.
// Returns placeholder text on error or when the session has no windows.
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

func main() {
	sessions, err := loadSessions()
	if err != nil {
		fmt.Fprintln(os.Stderr, "tmux-tui:", err)
		os.Exit(1)
	}
	fmt.Printf("loaded %d sessions\n", len(sessions))
	for _, s := range sessions {
		fmt.Printf("  %s: %d windows\n", s.name, s.windows)
	}
}
