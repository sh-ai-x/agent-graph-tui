//! Streaming parser for Claude Code / Codex session JSONL files.
//!
//! Each line is one JSON record. A single assistant turn may carry several
//! content blocks (`text` + `tool_use` + `tool_use` for parallel tool calls),
//! so [`parse_line`] returns `Vec<Event>` to surface them all. CRLF line
//! endings and truncate-then-rewrite (Claude Code session rotation) are
//! handled in [`Session::rescan_from`].

use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::Value;

/// Hard cap on the number of `Event`s retained in memory. Once exceeded,
/// the oldest entries are dropped (ring-buffer policy).
#[cfg(not(test))]
pub const MAX_EVENTS: usize = 50_000;
#[cfg(test)]
pub const MAX_EVENTS: usize = 16;

/// Hard cap on how many bytes we will read from a single jsonl file.
/// Caps a hostile / malformed / unterminated record so `read_until` can't
/// allocate arbitrarily.
const MAX_FILE_BYTES: u64 = 256 * 1024 * 1024;

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
    #[serde(default)]
    payload: Option<Value>,
}

/// Outcome of a rescan, telling the caller whether the event log was reset
/// (rotation or cap eviction) so it can rebuild its tree rather than doing
/// an incremental extend that would target evicted rows.
pub struct RescanOutcome {
    pub new_offset: u64,
    pub rebuilt: bool,
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
    /// Caps the in-memory event log at `MAX_EVENTS` (ring policy) and totals
    /// at `MAX_FILE_BYTES` (per-record allocation guard).
    pub fn rescan_from(&mut self, offset: u64) -> std::io::Result<RescanOutcome> {
        let file_size = std::fs::metadata(&self.path).map(|m| m.len()).unwrap_or(0);
        let seek_to = if file_size < offset { 0 } else { offset };

        let rotated = seek_to != offset;
        if rotated {
            // File was rotated/truncated: drop prior parse so a fresh start is
            // consistent with the truncated-and-rewritten contents.
            self.events.clear();
        }

        let mut f = File::open(&self.path)?;
        f.seek(SeekFrom::Start(seek_to))?;
        let f = f.take(MAX_FILE_BYTES);
        let mut reader = BufReader::new(f);

        let mut buf: Vec<u8> = Vec::new();
        let mut consumed: u64 = 0;

        loop {
            buf.clear();
            let n = reader.read_until(b'\n', &mut buf)?;
            if n == 0 {
                break;
            }
            consumed += n as u64;

            let line = std::str::from_utf8(&buf).unwrap_or("");
            let trimmed = line.trim_end_matches(|c| c == '\n' || c == '\r');
            if trimmed.is_empty() {
                continue;
            }

            let mut events = parse_line(trimmed);
            if events.is_empty() {
                events.push(Event::Unknown {
                    ts: String::new(),
                    raw: truncate(trimmed, 240),
                });
            }
            self.events.extend(events);
        }

        let cap_evicted = if self.events.len() > MAX_EVENTS {
            let excess = self.events.len() - MAX_EVENTS;
            self.events.drain(..excess);
            true
        } else {
            false
        };

        Ok(RescanOutcome {
            new_offset: seek_to + consumed,
            rebuilt: rotated || cap_evicted,
        })
    }
}

/// Parse one JSONL record into zero or more events.
///
/// Returning `Vec<Event>` instead of `Option<Event>` lets a single assistant
/// turn emit one event per content block (Claude Code routinely produces
/// text + multiple parallel `tool_use` blocks in a single turn).
pub fn parse_line(line: &str) -> Vec<Event> {
    let raw: Raw = match serde_json::from_str(line) {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };
    let ts = raw.timestamp;
    let mut events: Vec<Event> = Vec::new();

    if raw.r#type == "user" {
        if let Some(text) = content_text(&raw.message) {
            events.push(Event::User {
                ts: ts.clone(),
                text,
            });
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
                    events.push(Event::ToolResult {
                        ts: ts.clone(),
                        tool_use_id: id,
                        output: truncate(&out, 1024),
                        is_error,
                    });
                }
            }
        }
        return events;
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
                        events.push(Event::AssistantText {
                            ts: ts.clone(),
                            text,
                        });
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
                        let input = b.get("input").map(|v| v.to_string()).unwrap_or_default();
                        events.push(Event::ToolCall {
                            ts: ts.clone(),
                            id,
                            name,
                            input: truncate(&input, 512),
                        });
                    }
                    _ => {}
                }
            }
            if !events.is_empty() {
                return events;
            }
        }
        if let Some(text) = content_text(&raw.message) {
            events.push(Event::AssistantText { ts, text });
        }
        return events;
    }

    // Codex envelope: top-level `type` is `"response_item"` and the inner
    // event is described by `payload.type`. Claude Code's `user`/`assistant`
    // shape is mutually exclusive with this branch — `parse_line` is called
    // per-record so the file can mix both hosts.
    if raw.r#type == "response_item" {
        if let Some(payload) = raw.payload.as_ref() {
            let payload_type = payload
                .get("type")
                .and_then(|t| t.as_str())
                .unwrap_or("");
            match payload_type {
                "message" => {
                    let text = codex_content_text(payload);
                    if !text.is_empty() {
                        events.push(Event::AssistantText {
                            ts: ts.clone(),
                            text,
                        });
                    }
                }
                "user_message" => {
                    let text = codex_content_text(payload);
                    if !text.is_empty() {
                        events.push(Event::User {
                            ts: ts.clone(),
                            text,
                        });
                    }
                }
                "custom_tool_call" | "function_call" => {
                    let id = codex_call_id(payload);
                    let name = payload
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("?")
                        .to_string();
                    let input = payload.get("input").map(|v| v.to_string()).unwrap_or_default();
                    events.push(Event::ToolCall {
                        ts: ts.clone(),
                        id,
                        name,
                        input: truncate(&input, 512),
                    });
                }
                "custom_tool_call_output" | "function_call_output" => {
                    let tool_use_id = codex_call_id(payload);
                    let output = codex_tool_output(payload);
                    let is_error = payload
                        .get("is_error")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    events.push(Event::ToolResult {
                        ts: ts.clone(),
                        tool_use_id,
                        output: truncate(&output, 1024),
                        is_error,
                    });
                }
                // reasoning / web_search_call / file_search_call / image_gen_call
                // are skipped — they don't map cleanly to our row taxonomy.
                _ => {}
            }
        }
        return events;
    }

    // event_msg (token_count / item_completed / turn_started / ...) and
    // session_meta are internal markers with no per-row representation.
    Vec::new()
}

/// Concatenate `output_text` / `input_text` / `text` blocks of a codex
/// `message` or `user_message` payload. Mirrors [`content_text`] for
/// Claude Code's flat string-or-array `content` shape but ignores
/// `reasoning` blocks (encrypted in codex's wire format).
fn codex_content_text(payload: &Value) -> String {
    let mut buf = String::new();
    if let Some(arr) = payload.get("content").and_then(|c| c.as_array()) {
        for b in arr {
            let btype = b.get("type").and_then(|t| t.as_str()).unwrap_or("");
            if btype == "output_text" || btype == "input_text" || btype == "text" {
                if let Some(text) = b.get("text").and_then(|t| t.as_str()) {
                    if !buf.is_empty() {
                        buf.push('\n');
                    }
                    buf.push_str(text);
                }
            }
        }
    }
    buf
}

/// Extract `output` from a `custom_tool_call_output` / `function_call_output`
/// payload. Codex emits it either as a plain string or as an array of
/// `{type: "input_text", text: "..."}` blocks (mirroring `message.content`).
fn codex_tool_output(payload: &Value) -> String {
    if let Some(s) = payload.get("output").and_then(|v| v.as_str()) {
        return s.to_string();
    }
    if let Some(arr) = payload.get("output").and_then(|v| v.as_array()) {
        let mut buf = String::new();
        for b in arr {
            if let Some(text) = b.get("text").and_then(|t| t.as_str()) {
                if !buf.is_empty() {
                    buf.push('\n');
                }
                buf.push_str(text);
            }
        }
        return buf;
    }
    String::new()
}

/// Resolve the call_id used to match a tool call with its output. Falls back
/// to the per-item `id` for envelope variants that don't include `call_id`
/// (none observed in the wild, but cheap insurance).
fn codex_call_id(payload: &Value) -> String {
    payload
        .get("call_id")
        .and_then(|v| v.as_str())
        .or_else(|| payload.get("id").and_then(|v| v.as_str()))
        .unwrap_or("")
        .to_string()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_user_text_into_one_event() {
        let line = r#"{"type":"user","message":{"role":"user","content":"hi"},"timestamp":"t0"}"#;
        let evs = parse_line(line);
        assert_eq!(evs.len(), 1);
        match &evs[0] {
            Event::User { ts, text } => {
                assert_eq!(ts, "t0");
                assert_eq!(text, "hi");
            }
            other => panic!("expected User, got {other:?}"),
        }
    }

    #[test]
    fn parses_assistant_with_multi_block_content_emits_one_event_per_block() {
        let line = r#"{"type":"assistant","message":{"role":"assistant","content":[
            {"type":"text","text":"thinking"},
            {"type":"tool_use","id":"toolu_1","name":"Read","input":{"path":"/x"}},
            {"type":"tool_use","id":"toolu_2","name":"Bash","input":{"cmd":"ls"}}
        ]},"timestamp":"t1"}"#;
        let evs = parse_line(line);
        assert_eq!(evs.len(), 3, "must emit one event per block");
        assert!(matches!(evs[0], Event::AssistantText { ref text, .. } if text == "thinking"));
        assert!(
            matches!(&evs[1], Event::ToolCall { id, name, .. } if id == "toolu_1" && name == "Read")
        );
        assert!(
            matches!(&evs[2], Event::ToolCall { id, name, .. } if id == "toolu_2" && name == "Bash")
        );
    }

    #[test]
    fn parses_user_with_tool_result_including_is_error() {
        let line = r#"{"type":"user","message":{"role":"user","content":[
            {"type":"tool_result","tool_use_id":"toolu_1","content":"boom","is_error":true}
        ]},"timestamp":"t2"}"#;
        let evs = parse_line(line);
        assert_eq!(evs.len(), 1);
        match &evs[0] {
            Event::ToolResult {
                tool_use_id,
                is_error,
                ..
            } => {
                assert_eq!(tool_use_id, "toolu_1");
                assert!(*is_error);
            }
            other => panic!("expected ToolResult, got {other:?}"),
        }
    }

    #[test]
    fn malformed_json_returns_empty_vec() {
        let evs = parse_line("not json");
        assert!(evs.is_empty());
    }

    #[test]
    fn rescan_with_crlf_line_endings_advances_offset_to_file_size() {
        let path = std::env::temp_dir().join("agent-graph-tui-test-crlf.jsonl");
        let _ = std::fs::remove_file(&path);
        let line1 = br#"{"type":"user","message":{"role":"user","content":"hi"},"timestamp":"t0"}"#;
        let line2 =
            br#"{"type":"user","message":{"role":"user","content":"there"},"timestamp":"t1"}"#;
        let mut body = Vec::new();
        body.extend_from_slice(line1);
        body.extend_from_slice(b"\r\n");
        body.extend_from_slice(line2);
        body.extend_from_slice(b"\r\n");
        std::fs::write(&path, &body).unwrap();

        // Build session directly to avoid double-rescan via Session::open().
        let mut s = Session {
            path: path.clone(),
            events: Vec::new(),
        };
        let outcome = s.rescan_from(0).unwrap();
        assert_eq!(s.events.len(), 2);
        assert!(!outcome.rebuilt, "fresh read should not flag rebuild");
        assert_eq!(
            outcome.new_offset,
            body.len() as u64,
            "offset must match raw file size (CRLF = 2 bytes/line, not 1)"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn rescan_after_file_rotation_resets_cursor() {
        let path = std::env::temp_dir().join("agent-graph-tui-test-rotate.jsonl");
        let _ = std::fs::remove_file(&path);
        // Initial file: single user message.
        std::fs::write(
            &path,
            b"{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"v1\"},\"timestamp\":\"t0\"}\n",
        )
        .unwrap();
        let mut s = Session::open(&path).unwrap();
        assert_eq!(s.events.len(), 1);

        // Simulate Claude Code rotation: truncate + rewrite with different content.
        let mut body = Vec::new();
        body.extend_from_slice(
            b"{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"v2\"},\"timestamp\":\"t1\"}\n",
        );
        body.extend_from_slice(
            b"{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"v3\"},\"timestamp\":\"t2\"}\n",
        );
        std::fs::write(&path, &body).unwrap();

        // Caller passes an offset that's past the post-rotation EOF; parser
        // must detect the shrinkage, reset to byte 0, and signal rebuild.
        let outcome = s.rescan_from(10_000).unwrap();
        assert!(outcome.rebuilt, "rotation must signal rebuild");
        assert_eq!(
            outcome.new_offset,
            body.len() as u64,
            "rotation must re-read from byte 0"
        );
        assert_eq!(s.events.len(), 2);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn rescan_ring_caps_events_at_max_and_signals_rebuild() {
        let path = std::env::temp_dir().join("agent-graph-tui-test-ring.jsonl");
        let _ = std::fs::remove_file(&path);
        // Write MAX_EVENTS + 5 lines.
        let mut body = Vec::new();
        for i in 0..(super::MAX_EVENTS + 5) {
            let line = format!(
                "{{\"type\":\"user\",\"message\":{{\"role\":\"user\",\"content\":\"msg {i}\"}},\"timestamp\":\"t{i}\"}}\n"
            );
            body.extend_from_slice(line.as_bytes());
        }
        std::fs::write(&path, &body).unwrap();

        // Build session at offset = body size minus the LAST 5 lines worth,
        // so the next rescan only sees the additional 5 events but already
        // had MAX_EVENTS in memory. To exercise cap eviction straightforwardly,
        // start at offset 0 and let the parser consume all events at once.
        let mut s = Session {
            path: path.clone(),
            events: Vec::new(),
        };
        let outcome = s.rescan_from(0).unwrap();
        assert!(
            outcome.rebuilt,
            "ring-cap eviction from initial parse still flags rebuild"
        );
        assert_eq!(s.events.len(), super::MAX_EVENTS, "ring cap applied");
        let _ = std::fs::remove_file(&path);
    }

    // Codex envelope — top-level `type: "response_item"` with `payload.type`
    // distinguishing the inner event. Codex writes assistant text as
    // `payload.type == "message"`, user text as `user_message`, and tool
    // calls/results as `custom_tool_call` / `custom_tool_call_output` (or
    // `function_call` / `function_call_output` in older sessions). The
    // matching key between call and output is `payload.call_id`.

    #[test]
    fn parses_codex_response_item_message_emits_assistant_text() {
        let line = r#"{"timestamp":"t0","ordinal":0,"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"hello"}]}}"#;
        let evs = parse_line(line);
        assert_eq!(evs.len(), 1);
        match &evs[0] {
            Event::AssistantText { ts, text } => {
                assert_eq!(ts, "t0");
                assert_eq!(text, "hello");
            }
            other => panic!("expected AssistantText, got {other:?}"),
        }
    }

    #[test]
    fn parses_codex_response_item_user_message_emits_user() {
        let line = r#"{"timestamp":"t0","ordinal":0,"type":"response_item","payload":{"type":"user_message","role":"user","content":[{"type":"input_text","text":"hi"}]}}"#;
        let evs = parse_line(line);
        assert_eq!(evs.len(), 1);
        match &evs[0] {
            Event::User { ts, text } => {
                assert_eq!(ts, "t0");
                assert_eq!(text, "hi");
            }
            other => panic!("expected User, got {other:?}"),
        }
    }

    #[test]
    fn parses_codex_custom_tool_call_emits_tool_call_with_call_id() {
        let line = r#"{"timestamp":"t0","ordinal":0,"type":"response_item","payload":{"type":"custom_tool_call","id":"ctc_xxx","call_id":"call_abc","name":"exec","input":"const x=1;","status":"completed"}}"#;
        let evs = parse_line(line);
        assert_eq!(evs.len(), 1);
        match &evs[0] {
            Event::ToolCall { id, name, input, .. } => {
                assert_eq!(id, "call_abc", "call_id is the matching key");
                assert_eq!(name, "exec");
                assert!(input.contains("const x=1"));
            }
            other => panic!("expected ToolCall, got {other:?}"),
        }
    }

    #[test]
    fn parses_codex_custom_tool_call_output_emits_tool_result_with_matching_call_id() {
        let line = r#"{"timestamp":"t0","ordinal":0,"type":"response_item","payload":{"type":"custom_tool_call_output","id":"ctco_xxx","call_id":"call_abc","output":"ok"}}"#;
        let evs = parse_line(line);
        assert_eq!(evs.len(), 1);
        match &evs[0] {
            Event::ToolResult {
                tool_use_id,
                output,
                is_error,
                ..
            } => {
                assert_eq!(tool_use_id, "call_abc");
                assert_eq!(output, "ok");
                assert!(!*is_error);
            }
            other => panic!("expected ToolResult, got {other:?}"),
        }
    }

    #[test]
    fn parses_codex_custom_tool_call_output_with_is_error_marks_failed() {
        let line = r#"{"timestamp":"t0","ordinal":0,"type":"response_item","payload":{"type":"custom_tool_call_output","call_id":"call_abc","output":"oops","is_error":true}}"#;
        let evs = parse_line(line);
        assert_eq!(evs.len(), 1);
        match &evs[0] {
            Event::ToolResult { is_error, .. } => assert!(*is_error),
            other => panic!("expected ToolResult, got {other:?}"),
        }
    }

    #[test]
    fn parses_codex_function_call_and_output_also_supported() {
        let call = r#"{"timestamp":"t0","type":"response_item","payload":{"type":"function_call","call_id":"call_f1","name":"Read","input":"{\"path\":\"/x\"}","status":"completed"}}"#;
        let out = r#"{"timestamp":"t1","type":"response_item","payload":{"type":"function_call_output","call_id":"call_f1","output":"file contents"}}"#;
        let call_evs = parse_line(call);
        let out_evs = parse_line(out);
        match &call_evs[0] {
            Event::ToolCall { id, name, .. } => {
                assert_eq!(id, "call_f1");
                assert_eq!(name, "Read");
            }
            other => panic!("expected ToolCall, got {other:?}"),
        }
        match &out_evs[0] {
            Event::ToolResult {
                tool_use_id,
                output,
                ..
            } => {
                assert_eq!(tool_use_id, "call_f1");
                assert_eq!(output, "file contents");
            }
            other => panic!("expected ToolResult, got {other:?}"),
        }
    }

    #[test]
    fn parses_codex_event_msg_token_count_emits_no_events() {
        let line = r#"{"timestamp":"t0","ordinal":0,"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{}}}}"#;
        let evs = parse_line(line);
        assert!(
            evs.is_empty(),
            "event_msg token_count is internal, no event should be emitted"
        );
    }

    #[test]
    fn parses_codex_session_meta_emits_no_events() {
        let line = r#"{"timestamp":"t0","ordinal":0,"type":"session_meta","payload":{"cwd":"/Users/x/proj","model_provider":"oai"}}"#;
        let evs = parse_line(line);
        assert!(
            evs.is_empty(),
            "session_meta is metadata, no event should be emitted"
        );
    }
}
