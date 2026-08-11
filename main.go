// Package main provides the entry point for tmux-tui.
//
// Step 0 stub: imports the three Bubble Tea deps to force resolution
// + go.sum population. Real model lands in step 2.
package main

import (
	"fmt"

	"github.com/charmbracelet/bubbles/list"
	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/lipgloss"
)

func main() {
	_ = list.Model{}
	_ = tea.Quit
	_ = lipgloss.NewStyle()
	fmt.Println("tmux-tui: scaffold stub (step 0)")
}
