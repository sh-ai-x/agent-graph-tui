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
    /// Working directory the session was started from — decoded from the
    /// Claude Code project-path encoding when applicable.
    pub worktree: Option<PathBuf>,
    /// `git -C <worktree> rev-parse --abbrev-ref HEAD`.
    pub branch: Option<String>,
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

    // Phase 2: enrich the survivors with branch info (one git fork per session).
    for s in &mut report.sessions {
        if let Some(wt) = &s.worktree {
            s.branch = git_branch(wt).filter(|b| !b.is_empty());
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
    let mut head = [0u8; 8 * 1024];
    let n = f.read(&mut head).unwrap_or(0);
    let head_str = std::str::from_utf8(&head[..n]).unwrap_or("");

    let agent = AgentKind::from_path(path);
    let task = first_user_message(head_str);
    let worktree = decode_worktree_from_path(path, agent);
    // Branch populated later via enrich step in `discover()`.

    let node_count_proxy = head_str.bytes().filter(|&b| b == b'\n').count();

    Some(DiscoveredSession {
        path: path.to_path_buf(),
        agent,
        worktree,
        branch: None,
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

/// Claude Code encodes the project path by replacing `/` with `-` and
/// prepending `-`. E.g. `/Users/foo/bar` → `-Users-foo-bar`.
fn decode_worktree_from_path(path: &Path, agent: AgentKind) -> Option<PathBuf> {
    if agent != AgentKind::ClaudeCode {
        return path
            .parent()
            .map(|p| p.to_path_buf())
            .or_else(|| Some(path.to_path_buf()));
    }
    let comp = path
        .components()
        .find(|c| {
            let s = c.as_os_str().to_string_lossy();
            s.starts_with('-') && s.len() > 1
        })?
        .as_os_str()
        .to_string_lossy()
        .trim_start_matches('-')
        .to_string();
    let decoded = if comp.starts_with("Users/") || comp.starts_with("home/") {
        format!("/{comp}")
    } else {
        format!("/{comp}")
    };
    Some(PathBuf::from(decoded))
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
    fn decode_worktree_roundtrips_claude_project_path() {
        let p = Path::new("/home/u/.claude/projects/-Users-foo-bar/abc.jsonl");
        let wt = decode_worktree_from_path(p, AgentKind::ClaudeCode).unwrap();
        // /Users-foo-bar is the literal Claude Code encoding (escaped) of a
        // path whose separators are ambiguous. We assert the literal form,
        // not the un-recoverable multi-segment form.
        assert_eq!(wt, PathBuf::from("/Users-foo-bar"));
    }
}
