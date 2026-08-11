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
