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

use crate::parser::Event;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Pending,
    Done,
    Failed,
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
    pub fn build(parser: &crate::parser::Session) -> Self {
        let mut s = Self {
            rows: Vec::new(),
            pending_tools: HashMap::new(),
        };
        for ev in &parser.events {
            s.push(ev);
        }
        s
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
}
