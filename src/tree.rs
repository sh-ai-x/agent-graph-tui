//! Builds a renderable flat tree from a linear event log.
//!
//! Layout (one row per logical step):
//!
//! ```text
//! user       "hello"
//! assistant  "thinking..."
//! tool       Read ./x.rs        ✓
//!     └─ result  <file bytes>     ✓
//! assistant  "done"
//! ```

use std::collections::HashMap;

use ratatui::style::Color;

use crate::parser::Event;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Pending,
    Done,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionStatus {
    /// Some `tool_use` is awaiting a result. The agent is mid-task.
    Running,
    /// All `tool_use`s resolved cleanly — agent reached an end state.
    Done,
    /// At least one `tool_result` came back with `is_error=true`.
    Failed,
    /// Has pending work AND no file activity for ≥ BLOCKED_AFTER. The agent
    /// is probably waiting on user input or stalled.
    Blocked,
}

pub const BLOCKED_AFTER: std::time::Duration = std::time::Duration::from_secs(5 * 60);

impl SessionStatus {
    /// Filled/empty dot glyph for the dashboard. Filled = active work
    /// in flight, empty = nothing pending. `x` for failed (rendered with
    /// `Color::Red` so the user can tell apart from `o` done).
    pub fn glyph(self) -> &'static str {
        match self {
            Self::Running => "●",
            Self::Done => "○",
            Self::Failed => "✗",
            Self::Blocked => "?",
        }
    }

    /// CSS-style color hint for the glyph. Running = cyan to make the
    /// "currently active" session pop against the muted Done rows.
    pub fn color(self) -> Color {
        match self {
            Self::Running => Color::Cyan,
            Self::Done => Color::DarkGray,
            Self::Failed => Color::Red,
            Self::Blocked => Color::Yellow,
        }
    }
}

/// Compute the session-level status from the rows. Caller passes the
/// last-modified timestamp of the underlying file so we can flip
/// Running → Blocked when a session stalls.
pub fn session_status(
    rows: &[Node],
    last_modified: Option<std::time::SystemTime>,
) -> SessionStatus {
    let mut has_failed = false;
    let mut has_pending = false;
    for row in rows {
        match row.status {
            Status::Pending => has_pending = true,
            Status::Failed => has_failed = true,
            Status::Done => {}
        }
    }
    if has_failed {
        return SessionStatus::Failed;
    }
    if has_pending {
        if let Some(modified) = last_modified {
            if let Ok(elapsed) = modified.elapsed() {
                if elapsed >= BLOCKED_AFTER {
                    return SessionStatus::Blocked;
                }
            }
        }
        return SessionStatus::Running;
    }
    SessionStatus::Done
}

#[derive(Debug, Clone)]
pub enum NodeKind {
    UserText(String),
    AssistantText(String),
    ToolCall {
        name: String,
        input: String,
    },
    ToolResult(String),
    Unknown(String),
}

#[derive(Debug, Clone)]
pub struct Node {
    pub depth: usize,
    pub kind: NodeKind,
    pub status: Status,
    pub ts: String,
}

pub struct Session {
    pub rows: Vec<Node>,
    pending_tools: HashMap<String, usize>,
}

impl Session {
    pub fn new() -> Self {
        Self {
            rows: Vec::new(),
            pending_tools: HashMap::new(),
        }
    }

    pub fn build(parser: &crate::parser::Session) -> Self {
        let mut s = Self::new();
        s.extend_from(&parser.events);
        s
    }

    /// Append-only update: feed new events without rebuilding prior rows.
    /// Caller passes only the slice that wasn't seen before; pending tool
    /// resolution still uses `pending_tools` so old `tool_use` rows still
    /// get their status updated.
    pub fn extend_from(&mut self, events: &[Event]) {
        for ev in events {
            self.push(ev);
        }
    }

    /// Reset state — used when the parser detects file rotation.
    pub fn clear(&mut self) {
        self.rows.clear();
        self.pending_tools.clear();
    }

    fn push(&mut self, ev: &Event) {
        match ev {
            Event::User { ts, text } => self.rows.push(Node {
                depth: 0,
                kind: NodeKind::UserText(text.clone()),
                status: Status::Done,
                ts: ts.clone(),
            }),
            Event::AssistantText { ts, text } => self.rows.push(Node {
                depth: 0,
                kind: NodeKind::AssistantText(text.clone()),
                status: Status::Done,
                ts: ts.clone(),
            }),
            Event::ToolCall { ts, id, name, input } => {
                let idx = self.rows.len();
                self.rows.push(Node {
                    depth: 0,
                    kind: NodeKind::ToolCall {
                        name: name.clone(),
                        input: input.clone(),
                    },
                    status: Status::Pending,
                    ts: ts.clone(),
                });
                self.pending_tools.insert(id.clone(), idx);
            }
            Event::ToolResult { ts, tool_use_id, output, is_error } => {
                if let Some(&parent_idx) = self.pending_tools.get(tool_use_id) {
                    self.rows[parent_idx].status = if *is_error {
                        Status::Failed
                    } else {
                        Status::Done
                    };
                }
                self.rows.push(Node {
                    depth: 1,
                    kind: NodeKind::ToolResult(output.clone()),
                    status: if *is_error { Status::Failed } else { Status::Done },
                    ts: ts.clone(),
                });
            }
            Event::Unknown { ts, raw } => self.rows.push(Node {
                depth: 0,
                kind: NodeKind::Unknown(raw.clone()),
                status: Status::Pending,
                ts: ts.clone(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::{Event, Session as ParseSession};

    fn parser_with(events: Vec<Event>) -> ParseSession {
        ParseSession {
            path: std::path::PathBuf::from("/tmp/_unused.jsonl"),
            events,
        }
    }

    fn ev_text(t: &str) -> Event {
        Event::AssistantText {
            ts: String::new(),
            text: t.into(),
        }
    }
    fn ev_call(id: &str, name: &str) -> Event {
        Event::ToolCall {
            ts: String::new(),
            id: id.into(),
            name: name.into(),
            input: String::new(),
        }
    }
    fn ev_result(id: &str, err: bool) -> Event {
        Event::ToolResult {
            ts: String::new(),
            tool_use_id: id.into(),
            output: String::new(),
            is_error: err,
        }
    }

    #[test]
    fn tool_call_pending_until_matching_result() {
        let s = Session::build(&parser_with(vec![
            ev_call("t1", "Read"),
            ev_text("still pending"),
        ]));
        assert_eq!(s.rows.len(), 2);
        assert_eq!(s.rows[0].status, Status::Pending);
        assert_eq!(s.rows[1].status, Status::Done);
    }

    #[test]
    fn tool_call_marks_done_on_success_result() {
        let s = Session::build(&parser_with(vec![
            ev_call("t1", "Read"),
            ev_result("t1", false),
        ]));
        assert_eq!(s.rows[0].status, Status::Done);
        assert_eq!(s.rows[1].status, Status::Done);
        assert_eq!(s.rows[1].depth, 1, "result is indented under call");
    }

    #[test]
    fn tool_call_marks_failed_on_is_error_result() {
        let s = Session::build(&parser_with(vec![
            ev_call("t1", "Bash"),
            ev_result("t1", true),
        ]));
        assert_eq!(s.rows[0].status, Status::Failed);
    }

    #[test]
    fn orphan_tool_result_with_no_pending_call_still_renders() {
        let s = Session::build(&parser_with(vec![ev_result("unknown_id", false)]));
        // The result still appears as a row, just with no parent to update.
        assert_eq!(s.rows.len(), 1);
        assert_eq!(s.rows[0].depth, 1);
    }

    #[test]
    fn extend_appends_only_to_existing_state() {
        // Build from an initial event set; then extend with new events;
        // verify rows now include both old + new without losing prior status.
        let mut s = Session::build(&parser_with(vec![
            ev_call("t1", "Read"),
            ev_result("t1", false), // resolves to Done
        ]));
        assert_eq!(s.rows.len(), 2);
        assert_eq!(s.rows[0].status, Status::Done);

        s.extend_from(&[ev_text("post-result prose")]);
        assert_eq!(s.rows.len(), 3);
        assert_eq!(s.rows[2].status, Status::Done);
    }

    #[test]
    fn clear_resets_rows_and_pending_tools() {
        let mut s = Session::build(&parser_with(vec![
            ev_call("t1", "Read"),
            ev_result("t1", false),
        ]));
        s.clear();
        assert!(s.rows.is_empty());
        // After clear, a fresh resolve should still work (HashMap reset).
        s.extend_from(&[ev_call("t2", "Bash")]);
        assert_eq!(s.rows.len(), 1);
        assert_eq!(s.rows[0].status, Status::Pending);
    }

    // session_status — coarse-grained aggregation of per-row statuses.

    fn row_with_status(status: Status, depth: usize) -> Node {
        Node {
            depth,
            kind: NodeKind::AssistantText(String::new()),
            status,
            ts: String::new(),
        }
    }

    #[test]
    fn session_status_empty_rows_is_done() {
        assert_eq!(session_status(&[], None), SessionStatus::Done);
    }

    #[test]
    fn session_status_all_done_is_done() {
        let rows = vec![
            row_with_status(Status::Done, 0),
            row_with_status(Status::Done, 0),
        ];
        assert_eq!(session_status(&rows, None), SessionStatus::Done);
    }

    #[test]
    fn session_status_one_pending_is_running() {
        let rows = vec![
            row_with_status(Status::Done, 0),
            row_with_status(Status::Pending, 0),
        ];
        assert_eq!(session_status(&rows, None), SessionStatus::Running);
    }

    #[test]
    fn session_status_failed_takes_precedence_over_pending() {
        let rows = vec![
            row_with_status(Status::Pending, 0),
            row_with_status(Status::Failed, 0),
        ];
        assert_eq!(session_status(&rows, None), SessionStatus::Failed);
    }

    #[test]
    fn session_status_pending_with_old_mtime_is_blocked() {
        let rows = vec![row_with_status(Status::Pending, 0)];
        let old = std::time::SystemTime::now()
            - std::time::Duration::from_secs(BLOCKED_AFTER.as_secs() + 60);
        assert_eq!(session_status(&rows, Some(old)), SessionStatus::Blocked);
    }

    #[test]
    fn session_status_pending_with_recent_mtime_is_running() {
        let rows = vec![row_with_status(Status::Pending, 0)];
        let recent = std::time::SystemTime::now()
            - std::time::Duration::from_secs(5);
        assert_eq!(session_status(&rows, Some(recent)), SessionStatus::Running);
    }

    #[test]
    fn session_status_no_pending_with_failed_is_failed() {
        let rows = vec![
            row_with_status(Status::Done, 0),
            row_with_status(Status::Failed, 0),
        ];
        assert_eq!(session_status(&rows, None), SessionStatus::Failed);
    }

    #[test]
    fn session_status_glyphs_are_filled_or_empty_dot() {
        assert_eq!(SessionStatus::Running.glyph(), "●");
        assert_eq!(SessionStatus::Done.glyph(), "○");
        assert_eq!(SessionStatus::Failed.glyph(), "✗");
        assert_eq!(SessionStatus::Blocked.glyph(), "?");
    }
}
