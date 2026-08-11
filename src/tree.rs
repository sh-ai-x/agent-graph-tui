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
