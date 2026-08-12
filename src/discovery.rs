//! Discover active agent sessions across known hosts (Claude Code, Codex, …).
//!
//! For each session jsonl we report:
//!   - agent kind (path heuristic)
//!   - worktree (decoded from the path encoding Claude Code uses) — also
//!     queried via `git -C <worktree> branch --show-current` for the branch
//!   - task summary (first user message in the file)
//!   - last activity timestamp + size + node count proxy

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

use crate::tree;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentKind {
    ClaudeCode,
    Codex,
    MiniMax,
    Gemini,
    Unknown,
}

impl AgentKind {
    pub fn from_path(path: &Path) -> Self {
        let s = path.to_string_lossy().to_lowercase();
        if s.contains(".claude/projects") {
            Self::ClaudeCode
        } else if s.contains(".codex/sessions") || s.contains(".codex/session") {
            Self::Codex
        } else if s.contains(".minimax") {
            Self::MiniMax
        } else if s.contains(".gemini") {
            Self::Gemini
        } else {
            Self::Unknown
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude-code",
            Self::Codex => "codex",
            Self::MiniMax => "minimax",
            Self::Gemini => "gemini",
            Self::Unknown => "unknown",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Self::ClaudeCode => "🤖",
            Self::Codex => "⚒",
            Self::MiniMax => "✦",
            Self::Gemini => "◆",
            Self::Unknown => "?",
        }
    }

    /// Ratatui Color::Index for terminal color slots — agent-color-coded.
    pub fn color_index(self) -> usize {
        match self {
            Self::ClaudeCode => 14, // cyan
            Self::Codex => 13,      // magenta
            Self::MiniMax => 11,    // yellow
            Self::Gemini => 10,     // green
            Self::Unknown => 8,     // dark gray
        }
    }
}

#[derive(Debug, Clone)]
pub struct DiscoveredSession {
    pub path: PathBuf,
    pub agent: AgentKind,
    /// Display name of the model used in the first assistant message
    /// (e.g. `claude-opus-4-8`, `MiniMax-M3`). The agent kind is the
    /// runner; this is the actual LLM.
    pub model: Option<String>,
    /// Working directory the session was started from. Read from the
    /// top-level `cwd` field of any JSONL line in the file.
    pub cwd: Option<PathBuf>,
    /// Worktree name extracted from the project's `--worktrees-X` suffix,
    /// when applicable (Claude Code only).
    pub worktree_name: Option<String>,
    /// `git -C <cwd> rev-parse --abbrev-ref HEAD`.
    pub branch: Option<String>,
    /// `git -C <cwd> rev-parse --show-toplevel` → basename.
    pub repo_name: Option<String>,
    /// Quick status from the file's last event (no full parse).
    pub quick_status: tree::SessionStatus,
    /// First user message, trimmed.
    pub task: Option<String>,
    pub size_bytes: u64,
    pub modified: Option<SystemTime>,
    pub node_count_proxy: usize,
}

#[derive(Debug, Clone, Default)]
pub struct DiscoveryReport {
    pub sessions: Vec<DiscoveredSession>,
}

const SCAN_TIMEOUT_FILES_PER_DIR: usize = 1_000;
const SCAN_DEADLINE_MS: u64 = 1_500;

/// Scan known agent session locations. Two phases:
///   1) cheap scan: enumerate + metadata + read first 8 KiB for task extraction.
///   2) `git_branch` enrichment on the most-recent MAX_SESSIONS only — git is
///      a fork+exec that's too expensive to run for files we'll never display.
pub fn discover() -> DiscoveryReport {
    let started = std::time::Instant::now();
    let deadline = started + std::time::Duration::from_millis(SCAN_DEADLINE_MS);

    let mut report = DiscoveryReport::default();
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        let claude = home.join(".claude").join("projects");
        scan_jsonl_recursive(&claude, &mut report.sessions, 3, deadline);
        if std::time::Instant::now() < deadline {
            let codex = home.join(".codex").join("sessions");
            scan_jsonl_recursive(&codex, &mut report.sessions, 4, deadline);
        }
        if std::time::Instant::now() < deadline {
            let minimax = home.join(".minimax");
            scan_jsonl_recursive(&minimax, &mut report.sessions, 2, deadline);
        }
        if std::time::Instant::now() < deadline {
            let gemini = home.join(".gemini");
            scan_jsonl_recursive(&gemini, &mut report.sessions, 2, deadline);
        }
    }

    report
        .sessions
        .sort_by(|a, b| b.modified.cmp(&a.modified));
    report.sessions.truncate(MAX_SESSIONS);

    // Phase 2: enrich the survivors with git-derived info (one git fork per
    // session). Uses the cwd that's recorded in the JSONL's top-level
    // `cwd` field first; falls back to the current working directory (the
    // directory the user invoked `agent-graph-tui` from) when the JSONL head
    // doesn't carry `cwd`. The encoded project path is never used as cwd.
    let fallback_cwd = std::env::current_dir().ok();
    for s in &mut report.sessions {
        let cwd = s.cwd.clone().or_else(|| fallback_cwd.clone());
        let Some(cwd) = cwd else { continue };
        if !cwd.exists() {
            continue;
        }
        if let Some(b) = git_branch(&cwd).filter(|b| !b.is_empty()) {
            s.branch = Some(b);
        }
        if let Some(root) = git_toplevel(&cwd) {
            s.repo_name = root
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .filter(|n| !n.is_empty());
        }
    }

    report
}

const MAX_SESSIONS: usize = 32;

fn scan_jsonl_recursive(
    dir: &Path,
    out: &mut Vec<DiscoveredSession>,
    depth: usize,
    deadline: std::time::Instant,
) {
    if std::time::Instant::now() >= deadline {
        return;
    }
    if depth == 0 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut count = 0usize;
    for entry in rd.flatten() {
        if std::time::Instant::now() >= deadline {
            return;
        }
        if count >= SCAN_TIMEOUT_FILES_PER_DIR {
            return;
        }
        let p = entry.path();
        if p.is_dir() {
            scan_jsonl_recursive(&p, out, depth - 1, deadline);
        } else if p.extension().and_then(|s| s.to_str()) == Some("jsonl") {
            count += 1;
            if let Some(s) = scan_one(&p) {
                out.push(s);
            }
        }
    }
}

fn scan_one(path: &Path) -> Option<DiscoveredSession> {
    let meta = std::fs::metadata(path).ok()?;
    let modified = meta.modified().ok();
    let size = meta.len();

    use std::io::Read;
    let mut f = std::fs::File::open(path).ok()?;
    // Read up to 64 KiB so we likely cover the first assistant message (model
    // field) AND the first user message (cwd field). 8 KiB was too small.
    let mut head = vec![0u8; 64 * 1024];
    let n = f.read(&mut head).unwrap_or(0);
    head.truncate(n);
    let head_str = std::str::from_utf8(&head).unwrap_or("");

    let agent = AgentKind::from_path(path);
    let model = extract_model(head_str);
    let cwd = extract_cwd(head_str);
    let worktree_name = extract_worktree_name(path);
    let task = first_user_message(head_str);

    let node_count_proxy = head_str.bytes().filter(|&b| b == b'\n').count();

    Some(DiscoveredSession {
        path: path.to_path_buf(),
        agent,
        model,
        cwd,
        worktree_name,
        branch: None,
        repo_name: None,
        quick_status: quick_status(path),
        task,
        size_bytes: size,
        modified,
        node_count_proxy,
    })
}

fn first_user_message(head: &str) -> Option<String> {
    for line in head.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let v: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => break,
        };
        // User message — could be plain string content or content array with
        // text/tool_result blocks. Extract the first text block, falling back
        // to a plain string content.
        let is_user = v.get("type").and_then(|t| t.as_str()) == Some("user")
            || v.get("message")
                .and_then(|m| m.get("role"))
                .and_then(|r| r.as_str())
                == Some("user");
        if !is_user {
            break;
        }
        let content = v.get("message").and_then(|m| m.get("content"))?;
        if let Some(s) = content.as_str() {
            return Some(unescape(s));
        }
        if let Some(arr) = content.as_array() {
            for b in arr {
                if b.get("type").and_then(|t| t.as_str()) == Some("text") {
                    if let Some(t) = b.get("text").and_then(|t| t.as_str()) {
                        return Some(unescape(t));
                    }
                }
            }
        }
        break;
    }
    None
}

fn unescape(s: &str) -> String {
    s.replace("\\n", " ")
        .replace("\\\"", "\"")
        .replace("\\t", " ")
        .trim()
        .to_string()
}

/// Pulls the cwd from the first JSONL line that has a top-level `cwd`
/// field. Claude Code writes this on every line in the session file.
fn extract_cwd(head: &str) -> Option<PathBuf> {
    for line in head.lines() {
        let v: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if let Some(cwd) = v.get("cwd").and_then(|c| c.as_str()) {
            if !cwd.is_empty() {
                return Some(PathBuf::from(cwd));
            }
        }
    }
    None
}

/// Quick session-status check by reading the FILE'S LAST event from the tail
/// of the JSONL. Avoids parsing the entire file. Returns:
///   - `Running` if the last event is a `tool_use` (no matching result yet).
///   - `Failed` if the last event is a `tool_result` with `is_error=true`.
///   - `Done`   otherwise (last event is a user message → either Blocked-on-user
///               or fully done; we don't distinguish here).
pub fn quick_status(path: &Path) -> tree::SessionStatus {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut f) = std::fs::File::open(path) else {
        return tree::SessionStatus::Done;
    };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    if len == 0 {
        return tree::SessionStatus::Done;
    }
    // Read the last 8 KiB; drop the partial last line.
    let seek_to = len.saturating_sub(8 * 1024);
    let _ = f.seek(SeekFrom::Start(seek_to));
    let mut buf = Vec::with_capacity((len - seek_to) as usize);
    let _ = f.read_to_end(&mut buf);
    let text = String::from_utf8_lossy(&buf);
    let last_line = text
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("");
    let v: serde_json::Value = match serde_json::from_str(last_line) {
        Ok(v) => v,
        Err(_) => return tree::SessionStatus::Done,
    };
    let ty = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
    if ty == "assistant" {
        // Has any tool_use without matching tool_result in the buffered tail?
        let content = v.get("message").and_then(|m| m.get("content"));
        if let Some(arr) = content.and_then(|c| c.as_array()) {
            for block in arr {
                if block.get("type").and_then(|t| t.as_str()) == Some("tool_use") {
                    return tree::SessionStatus::Running;
                }
            }
        }
        return tree::SessionStatus::Done;
    }
    if ty == "user" {
        let content = v.get("message").and_then(|m| m.get("content"));
        if let Some(arr) = content.and_then(|c| c.as_array()) {
            for block in arr {
                if block.get("is_error").and_then(|v| v.as_bool()) == Some(true) {
                    return tree::SessionStatus::Failed;
                }
                if block.get("type").and_then(|t| t.as_str()) == Some("text") {
                    return tree::SessionStatus::Running;
                }
            }
        }
        return tree::SessionStatus::Done;
    }
    tree::SessionStatus::Done
}

/// Pulls the model name from the first assistant message in the head.
/// Returns `None` if the field isn't present (older rows, error lines, etc.).
fn extract_model(head: &str) -> Option<String> {
    for line in head.lines() {
        if !line.contains("\"type\":\"assistant\"") {
            continue;
        }
        let v: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if let Some(m) = v
            .get("message")
            .and_then(|m| m.get("model"))
            .and_then(|m| m.as_str())
        {
            return Some(m.to_string());
        }
    }
    None
}

/// Pulls the worktree name out of the project dir's `--worktrees-X` suffix,
/// when the session was started inside a git worktree. Returns `None` for
/// sessions started in the main repo.
fn extract_worktree_name(path: &Path) -> Option<String> {
    let dir = path.parent()?.file_name()?.to_string_lossy();
    let stripped = dir.trim_start_matches('-');
    stripped
        .find("--worktrees-")
        .map(|i| stripped[i + "--worktrees-".len()..].to_string())
}

/// Best-effort: find the actual cwd that the session was started from, so
/// we can run `git -C <cwd>` to get branch info. The encoded project dir
/// (`~/.claude/projects/-Users-foo-bar--worktrees-X`) does NOT exist on disk,
/// so we have to reconstruct the cwd.
///
/// Strategy: if the worktree-name is known, look in `$HOME/.claude/...`
/// worktree metadata. Otherwise, take the parent of the file's directory
/// and walk up looking for a `.git` directory.
fn guess_real_cwd(jsonl_path: &Path, worktree_name: &Option<String>) -> Option<PathBuf> {
    // For now, return the parent of the jsonl file — this is the
    // `~/.claude/projects/<encoded>` directory; if git has metadata there
    // we can use it. Fall back to None.
    if let Some(wt) = worktree_name {
        // Common pattern: project cwd is the parent of the worktree dir.
        // We don't have the project root, so return None for now.
        let _ = (jsonl_path, wt);
    }
    None
}

fn git_branch(cwd: &Path) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn git_toplevel(cwd: &Path) -> Option<PathBuf> {
    let out = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(PathBuf::from(s))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_kind_from_path_classifies_known_hosts() {
        assert_eq!(
            AgentKind::from_path(Path::new("/home/u/.claude/projects/foo/abc.jsonl")),
            AgentKind::ClaudeCode
        );
        assert_eq!(
            AgentKind::from_path(Path::new("/home/u/.codex/sessions/2026-08-12/x.jsonl")),
            AgentKind::Codex
        );
        assert_eq!(
            AgentKind::from_path(Path::new("/home/u/.minimax/x.jsonl")),
            AgentKind::MiniMax
        );
        assert_eq!(
            AgentKind::from_path(Path::new("/home/u/.gemini/x.jsonl")),
            AgentKind::Gemini
        );
        assert_eq!(
            AgentKind::from_path(Path::new("/tmp/whatever/x.jsonl")),
            AgentKind::Unknown
        );
    }

    #[test]
    fn first_user_message_extracts_initial_prompt() {
        let head = "{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"fix the parser bug\"},\"timestamp\":\"t0\"}\n";
        let msg = first_user_message(head).unwrap();
        assert_eq!(msg, "fix the parser bug");
    }

    #[test]
    fn first_user_message_handles_escaped_quotes() {
        let head = "{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"run \\\"foo\\\" and check\"},\"timestamp\":\"t0\"}\n";
        let msg = first_user_message(head).unwrap();
        assert_eq!(msg, "run \"foo\" and check");
    }

    #[test]
    fn first_user_message_handles_content_array_text_block() {
        let head = "{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"text\",\"text\":\"hi\"}]}}\n";
        let msg = first_user_message(head).unwrap();
        assert_eq!(msg, "hi");
    }

    #[test]
    fn extract_worktree_name_parses_dash_double_worktrees() {
        let p = Path::new(
            "/home/u/.claude/projects/-Users-sanghee-dev-agent-graph-tui--worktrees-multi-session-dashboard/abc.jsonl",
        );
        let name = extract_worktree_name(p).unwrap();
        assert_eq!(name, "multi-session-dashboard");
    }

    #[test]
    fn extract_worktree_name_returns_none_for_bare_repo() {
        let p = Path::new("/home/u/.claude/projects/-Users-sanghee-dev-agent-graph-tui/abc.jsonl");
        let name = extract_worktree_name(p);
        assert!(name.is_none());
    }

    #[test]
    fn extract_model_picks_up_first_assistant_model_field() {
        let head = "{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"hi\"}}\n\
                    {\"type\":\"assistant\",\"message\":{\"id\":\"m1\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"claude-opus-4-8\",\"content\":[{\"type\":\"text\",\"text\":\"ok\"}]}}\n";
        let model = extract_model(head).unwrap();
        assert_eq!(model, "claude-opus-4-8");
    }

    #[test]
    fn extract_model_returns_none_when_no_assistant() {
        let head = "{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"hello\"}}\n";
        let model = extract_model(head);
        assert!(model.is_none());
    }
}
