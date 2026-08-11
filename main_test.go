package main

import "testing"

func TestParseSessionLine_Valid(t *testing.T) {
	line := "work|3|2026-08-11|0"
	got, ok := parseSessionLine(line)
	if !ok {
		t.Fatalf("expected ok=true for valid line, got false")
	}
	if got.name != "work" {
		t.Errorf("name = %q, want %q", got.name, "work")
	}
	if got.windows != 3 {
		t.Errorf("windows = %d, want %d", got.windows, 3)
	}
	if got.created != "2026-08-11" {
		t.Errorf("created = %q, want %q", got.created, "2026-08-11")
	}
	if got.attached != "0" {
		t.Errorf("attached = %q, want %q", got.attached, "0")
	}
}

func TestParseSessionLine_WrongArity(t *testing.T) {
	if _, ok := parseSessionLine("only|two"); ok {
		t.Errorf("expected ok=false for 2 parts, got true")
	}
	if _, ok := parseSessionLine("a|b|c|d|e"); ok {
		t.Errorf("expected ok=false for 5 parts, got true")
	}
}

func TestParseSessionLine_EmptyName(t *testing.T) {
	if _, ok := parseSessionLine("|3|2026-08-11|0"); ok {
		t.Errorf("expected ok=false for empty name, got true")
	}
}

func TestParseSessionLine_NonIntegerWindows(t *testing.T) {
	if _, ok := parseSessionLine("work|abc|2026-08-11|0"); ok {
		t.Errorf("expected ok=false for non-integer windows, got true")
	}
}

// Session names beginning with '-' could be parsed by tmux as a flag argument
// when re-passed via exec.Command ("tmux", "kill-session", "-t", name).
// Reject them at the parser boundary to avoid argument-injection.
func TestParseSessionLine_LeadingDashName(t *testing.T) {
	if _, ok := parseSessionLine("-evil|3|2026-08-11|0"); ok {
		t.Errorf("expected ok=false for leading-dash name, got true")
	}
}

func TestParseSessionLine_NegativeWindows(t *testing.T) {
	if _, ok := parseSessionLine("work|-3|2026-08-11|0"); ok {
		t.Errorf("expected ok=false for negative windows, got true")
	}
}

// tmux session names MAY contain '|'. The parser rejects them in this
// scaffold (uses SplitN with hard arity = 4). v2 should switch the format
// string to a separator tmux forbids (e.g. ":") and update the parser
// accordingly. Documented as a known limitation; the regression is locked
// in here so any future change forces an explicit decision.
func TestParseSessionLine_PipeInName_RejectedByDesign(t *testing.T) {
	if _, ok := parseSessionLine("foo|bar|2|2026-08-11|attached"); ok {
		t.Errorf("expected ok=false (scaffold limitation); v2 must switch to a tmux-forbidden separator")
	}
}
