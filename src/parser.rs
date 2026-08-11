//! Streaming parser for Claude Code / Codex session JSONL files.
//!
//! Each line is one event. Unknown shapes are kept as `Unknown` so a single
//! malformed line never aborts the run.

use std::fs::File;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Clone)]
pub enum Event {
    User {
        ts: String,
        text: String,
    },
    AssistantText {
        ts: String,
        text: String,
    },
    ToolCall {
        ts: String,
        id: String,
        name: String,
        input: String,
    },
    ToolResult {
        ts: String,
        tool_use_id: String,
        output: String,
        is_error: bool,
    },
    Unknown {
        ts: String,
        raw: String,
    },
}

#[derive(Deserialize)]
struct Raw {
    #[serde(default)]
    r#type: String,
    #[serde(default)]
    timestamp: String,
    #[serde(default)]
    message: Value,
}

/// Read-only view of a session jsonl that can re-scan on demand.
pub struct Session {
    pub path: PathBuf,
    pub events: Vec<Event>,
}

impl Session {
    pub fn open(path: &Path) -> std::io::Result<Self> {
        let mut s = Self {
            path: path.to_path_buf(),
            events: Vec::new(),
        };
        s.rescan_from(0)?;
        Ok(s)
    }

    /// Re-read the file from `offset` bytes and append any newly parsed events.
    /// Returns the new offset.
    pub fn rescan_from(&mut self, offset: u64) -> std::io::Result<u64> {
        let mut f = File::open(&self.path)?;
        f.seek(SeekFrom::Start(offset))?;
        let reader = BufReader::new(f);
        let mut len: u64 = 0;
        for line in reader.lines() {
            let Ok(l) = line else { break };
            len = len.saturating_add(l.len() as u64 + 1);
            if l.trim().is_empty() {
                continue;
            }
            match parse_line(&l) {
                Some(ev) => self.events.push(ev),
                None => self.events.push(Event::Unknown {
                    ts: String::new(),
                    raw: truncate(&l, 240),
                }),
            }
        }
        Ok(offset + len)
    }
}

fn parse_line(line: &str) -> Option<Event> {
    let raw: Raw = serde_json::from_str(line).ok()?;
    let ts = raw.timestamp;

    if raw.r#type == "user" {
        if let Some(text) = content_text(&raw.message) {
            return Some(Event::User { ts, text });
        }
        if let Some(blocks) = raw.message.get("content").and_then(|c| c.as_array()) {
            for b in blocks {
                if b.get("type").and_then(|t| t.as_str()) == Some("tool_result") {
                    let id = b
                        .get("tool_use_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let out = b
                        .get("content")
                        .map(|v| {
                            v.as_str()
                                .map(String::from)
                                .unwrap_or_else(|| v.to_string())
                        })
                        .unwrap_or_default();
                    let is_error = b.get("is_error").and_then(|v| v.as_bool()).unwrap_or(false);
                    return Some(Event::ToolResult {
                        ts: ts.clone(),
                        tool_use_id: id,
                        output: truncate(&out, 1024),
                        is_error,
                    });
                }
            }
        }
    }

    if raw.r#type == "assistant" {
        if let Some(blocks) = raw.message.get("content").and_then(|c| c.as_array()) {
            for b in blocks {
                match b.get("type").and_then(|t| t.as_str()) {
                    Some("text") => {
                        let text = b
                            .get("text")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        return Some(Event::AssistantText { ts: ts.clone(), text });
                    }
                    Some("tool_use") => {
                        let id = b
                            .get("id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let name = b
                            .get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("?")
                            .to_string();
                        let input = b
                            .get("input")
                            .map(|v| v.to_string())
                            .unwrap_or_default();
                        return Some(Event::ToolCall {
                            ts: ts.clone(),
                            id,
                            name,
                            input: truncate(&input, 512),
                        });
                    }
                    _ => {}
                }
            }
        }
        if let Some(text) = content_text(&raw.message) {
            return Some(Event::AssistantText { ts, text });
        }
    }

    None
}

fn content_text(msg: &Value) -> Option<String> {
    let c = msg.get("content")?;
    if let Some(s) = c.as_str() {
        return Some(s.to_string());
    }
    if let Some(arr) = c.as_array() {
        let mut buf = String::new();
        for b in arr {
            if let Some(s) = b.get("text").and_then(|t| t.as_str()) {
                if !buf.is_empty() {
                    buf.push('\n');
                }
                buf.push_str(s);
            }
        }
        if !buf.is_empty() {
            return Some(buf);
        }
    }
    None
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push('…');
    out
}
