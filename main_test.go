package main

import "testing"

func TestParseSessionLine_Valid(t *testing.T) {
	s, ok := parseSessionLine("work|3|Mon Aug 10 12:00:00 2026|1")
	if !ok {
		t.Fatal("expected ok=true for valid session line")
	}
	if s.name != "work" {
		t.Errorf("name = %q, want %q", s.name, "work")
	}
	if s.windows != 3 {
		t.Errorf("windows = %d, want %d", s.windows, 3)
	}
	if s.created != "Mon Aug 10 12:00:00 2026" {
		t.Errorf("created = %q, want full timestamp", s.created)
	}
	if s.attached != "1" {
		t.Errorf("attached = %q, want %q", s.attached, "1")
	}
}

func TestParseSessionLine_Malformed(t *testing.T) {
	cases := []string{
		"",
		"only-one-field",
		"two|fields",
		"three|fields|only",
		"|||",
	}
	for _, line := range cases {
		if _, ok := parseSessionLine(line); ok {
			t.Errorf("parseSessionLine(%q) returned ok=true, want false", line)
		}
	}
}

func TestSession_Title(t *testing.T) {
	s := session{name: "main", windows: 5, created: "today", attached: "0"}
	if got := s.Title(); got != "main" {
		t.Errorf("Title() = %q, want %q", got, "main")
	}
}

func TestSession_Description(t *testing.T) {
	s := session{name: "main", windows: 5, created: "today", attached: "0"}
	if got := s.Description(); got != "5 windows · today" {
		t.Errorf("Description() = %q, want %q", got, "5 windows · today")
	}
}

func TestSession_FilterValue(t *testing.T) {
	s := session{name: "main", windows: 5, created: "today", attached: "0"}
	if got := s.FilterValue(); got != "main" {
		t.Errorf("FilterValue() = %q, want %q", got, "main")
	}
}

func TestParseSessionLine_AttachedZero(t *testing.T) {
	s, ok := parseSessionLine("idle|2|Tue Aug 11 09:00:00 2026|0")
	if !ok {
		t.Fatal("expected ok=true")
	}
	if s.attached != "0" {
		t.Errorf("attached = %q, want %q", s.attached, "0")
	}
}
