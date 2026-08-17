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
    /// Short session id derived from the filename — first 8 hex chars of
    /// the UUID. Used as the dashboard's per-session label.
    pub sid: String,
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

    report.sessions.sort_by(|a, b| b.modified.cmp(&a.modified));
    report.sessions.truncate(MAX_SESSIONS);

    // Phase 2: enrich the survivors with git-derived info (one git fork per
    // session). Uses the cwd that's recorded in the JSONL's top-level
    // `cwd` field first; falls back to the current working directory (the
    // directory the user invoked `agent-graph-tui` from) when the JSONL head
    // doesn't carry `cwd`. The encoded project path is never used as cwd.
    //
    // Only run git for sessions modified within the last hour (assuming the
    // default `recent_only` filter is on). The recency check here is
    // duplicated from the dashboard filter so we don't take a dependency;
    // 32 forks + 32 disk reads was ~9 s on the user's machine.
    let fallback_cwd = std::env::current_dir().ok();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for s in &mut report.sessions {
        // Skip git for sessions older than the recency cutoff — they're
        // hidden by the default filter anyway, so a branch / repo name
        // on them would never be displayed.
        let is_recent = match s.modified {
            Some(m) => m
                .elapsed()
                .map(|d| d.as_secs() < 60 * 60) // 1 hour
                .unwrap_or(false),
            None => false,
        };
        if !is_recent {
            continue;
        }
        let cwd = s.cwd.clone().or_else(|| fallback_cwd.clone());
        let Some(cwd) = cwd else { continue };
        if !cwd.exists() {
            continue;
        }
        if let Some(b) = git_branch(&cwd).filter(|b| !b.is_empty()) {
            s.branch = Some(b);
        }
        if let Some(root) = git_repo_root(&cwd) {
            let key = root.to_string_lossy().to_string();
            if seen.insert(key) {
                s.repo_name = root
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .filter(|n| !n.is_empty());
            }
        }
    }

    report
}

#[cfg(test)]
mod repo_root_tests {
    use super::*;

    fn run(cmd: &str) {
        eprintln!("    run: {cmd}");
    }

    #[test]
    fn git_repo_root_resolves_a_worktree_to_its_main_repo() {
        // Setup: a worktree of `main` at `.worktrees/skill-chain-redesign`.
        let dir =
            std::env::temp_dir().join(format!("agent-graph-tui-rroot-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let main_repo = dir.join("main");
        let wt = main_repo.join(".worktrees").join("skill-chain-redesign");
        std::fs::create_dir_all(&wt).unwrap();
        run(&format!("git init at {}", main_repo.display()));
        std::process::Command::new("git")
            .arg("-C")
            .arg(&main_repo)
            .args(["init", "--initial-branch=main"])
            .output()
            .unwrap();
        run(&format!(
            "git -C {} worktree add .worktrees/skill-chain-redesign -b skill-chain-redesign",
            main_repo.display()
        ));
        std::process::Command::new("git")
            .arg("-C")
            .arg(&main_repo)
            .args([
                "worktree",
                "add",
                ".worktrees/skill-chain-redesign",
                "-b",
                "skill-chain-redesign",
            ])
            .output()
            .unwrap();
        let resolved = git_repo_root(&wt).expect("should resolve");
        // We expect basename = `main`, NOT `skill-chain-redesign`.
        assert_eq!(
            resolved
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .as_deref(),
            Some("main"),
            "worktree toplevel should resolve to the main repo, got {:?}",
            resolved
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
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
    // Read up to 64 KiB so we likely cover the first assistant message
    // (model field) AND the first user message (task / cwd fields). 8 KiB
    // was too small — sessions with long preamble or large tool blocks
    // push the first assistant message past that.
    let mut head = vec![0u8; 64 * 1024];
    let n = f.read(&mut head).unwrap_or(0);
    head.truncate(n);
    let head_str = std::str::from_utf8(&head).unwrap_or("");

    let agent = AgentKind::from_path(path);
    let model = extract_model(path);
    let cwd = extract_cwd(head_str);
    let worktree_name = extract_worktree_name(path);
    let task = first_user_message(head_str);

    let node_count_proxy = head_str.bytes().filter(|&b| b == b'\n').count();

    Some(DiscoveredSession {
        path: path.to_path_buf(),
        sid: extract_sid(path),
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
        // to a plain string content. Also accept Codex's `response_item`
        // envelope whose payload carries role=user.
        let is_user = v.get("type").and_then(|t| t.as_str()) == Some("user")
            || v.get("message")
                .and_then(|m| m.get("role"))
                .and_then(|r| r.as_str())
                == Some("user")
            || (v.get("type").and_then(|t| t.as_str()) == Some("response_item")
                && v.get("payload")
                    .and_then(|p| p.get("type"))
                    .and_then(|t| t.as_str())
                    == Some("user_message"));
        if !is_user {
            break;
        }
        let content = v
            .get("message")
            .and_then(|m| m.get("content"))
            .or_else(|| v.get("payload").and_then(|p| p.get("content")))?;
        if let Some(s) = content.as_str() {
            return Some(unescape(s));
        }
        if let Some(arr) = content.as_array() {
            for b in arr {
                // Claude Code uses "text"; codex envelope uses
                // "input_text" / "output_text".
                let btype = b.get("type").and_then(|t| t.as_str()).unwrap_or("");
                if btype == "text" || btype == "input_text" || btype == "output_text" {
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

/// Derive a short, stable session id from the JSONL filename. For Codex's
/// `rollout-<timestamp>-<uuid>.jsonl` we strip the timestamp prefix and
/// return the first 8 hex chars of the uuid. For bare `<uuid>.jsonl`
/// (Claude Code / others) we just take the first 8 chars of the stem.
pub fn extract_sid(path: &Path) -> String {
    let stem = match path.file_stem().and_then(|s| s.to_str()) {
        Some(s) => s,
        None => return String::new(),
    };
    if let Some(rest) = stem.strip_prefix("rollout-") {
        // The timestamp portion is digits, dashes, and 'T'. The uuid
        // portion that follows contains hex letters. Walk forward until
        // we hit the first hex letter; step back one byte to include the
        // digit (or dash) that precedes it — that's the start of the uuid.
        for (i, c) in rest.char_indices() {
            if c.is_ascii_hexdigit() && !c.is_ascii_digit() {
                let start = if i > 0 { i - 1 } else { 0 };
                return rest[start..].chars().take(8).collect();
            }
        }
        // Fallback if the uuid portion isn't found.
        return rest.chars().take(8).collect();
    }
    stem.chars().take(8).collect()
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

/// Quick session-status check by reading the FILE'S LAST 64 KiB and walking
/// lines from the end. Avoids parsing the entire file. Returns:
///   - `Failed`   if the most recent `tool_result` has `is_error=true`.
///   - `Running`  if the most recent event is an assistant `tool_use`
///                (no matching `tool_result` yet) or a user message
///                (agent hasn't produced a response yet).
///   - `Done`     otherwise (last assistant event was a text reply).
pub fn quick_status(path: &Path) -> tree::SessionStatus {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut f) = std::fs::File::open(path) else {
        return tree::SessionStatus::Done;
    };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    if len == 0 {
        return tree::SessionStatus::Done;
    }
    let seek_to = len.saturating_sub(64 * 1024);
    let _ = f.seek(SeekFrom::Start(seek_to));
    let mut buf = Vec::with_capacity((len - seek_to) as usize);
    let _ = f.read_to_end(&mut buf);
    let text = String::from_utf8_lossy(&buf);
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    for line in lines.iter().rev() {
        let v: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let ty = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
        match ty {
            "assistant" => {
                let content = v.get("message").and_then(|m| m.get("content"));
                if let Some(arr) = content.and_then(|c| c.as_array()) {
                    for block in arr {
                        let btype = block.get("type").and_then(|t| t.as_str()).unwrap_or("");
                        // The agent is actively working. tool_use means
                        // it's blocked on a tool result; thinking /
                        // redacted_thinking mean it's still reasoning.
                        if btype == "tool_use"
                            || btype == "thinking"
                            || btype == "redacted_thinking"
                        {
                            return tree::SessionStatus::Running;
                        }
                        if btype == "text" {
                            // The agent's last block was a plain text
                            // reply — conversation is paused.
                            return tree::SessionStatus::Done;
                        }
                    }
                }
                return tree::SessionStatus::Done;
            }
            "user" => {
                // Any user event means the agent hasn't produced its
                // next response yet — either plain text (agent thinking)
                // or tool_result (agent processing the result).
                if let Some(arr) = v
                    .get("message")
                    .and_then(|m| m.get("content"))
                    .and_then(|c| c.as_array())
                {
                    for block in arr {
                        if block.get("is_error").and_then(|v| v.as_bool()) == Some(true) {
                            return tree::SessionStatus::Failed;
                        }
                    }
                }
                return tree::SessionStatus::Running;
            }
            "response_item" => {
                // Codex envelope. Inner event is keyed by `payload.type`:
                //   custom_tool_call   / function_call         — Running unless completed
                //   custom_tool_call_output / function_call_output with is_error — Failed
                //   custom_tool_call_output (no error)  — Done (matched call resolved)
                //   message                            — Done (assistant finished a turn)
                //   user_message                       — Running (agent hasn't replied)
                let payload = match v.get("payload") {
                    Some(p) => p,
                    None => continue,
                };
                let ptype = payload
                    .get("type")
                    .and_then(|t| t.as_str())
                    .unwrap_or("");
                match ptype {
                    "custom_tool_call" | "function_call" => {
                        let status = payload
                            .get("status")
                            .and_then(|t| t.as_str())
                            .unwrap_or("");
                        if status == "completed" {
                            // Tool finished executing; the matching output
                            // will be on the NEXT line (we walked backwards
                            // from EOF). Keep scanning back so we don't
                            // mis-report a stale completed call as Running.
                            continue;
                        }
                        return tree::SessionStatus::Running;
                    }
                    "custom_tool_call_output" | "function_call_output" => {
                        if payload
                            .get("is_error")
                            .and_then(|t| t.as_bool())
                            .unwrap_or(false)
                        {
                            return tree::SessionStatus::Failed;
                        }
                        // Clean output → keep scanning so a later
                        // pending tool call or user_message can flip us
                        // to Running.
                        continue;
                    }
                    "message" => {
                        return tree::SessionStatus::Done;
                    }
                    "user_message" => {
                        return tree::SessionStatus::Running;
                    }
                    _ => continue,
                }
            }
            // event_msg (token_count, item_completed, turn_started, …) and
            // session_meta carry no status information — keep scanning.
            _ => continue,
        }
    }
    tree::SessionStatus::Done
}

/// Walk a JSONL value looking for a model field at any of the
/// known locations (Claude Code: `message.model`; Codex envelope:
/// `payload.model`; some legacy: `response.model` / top-level `model`).
fn model_from_value(v: &serde_json::Value) -> Option<String> {
    let candidates = [
        v.get("message").and_then(|m| m.get("model")),
        v.get("payload").and_then(|p| p.get("model")),
        v.get("response").and_then(|r| r.get("model")),
        v.get("model"),
    ];
    candidates
        .into_iter()
        .flatten()
        .find_map(|m| m.as_str())
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty())
}

/// Walk up to `MODEL_SCAN_BYTES` of the file, line by line, looking for
/// the first JSONL line that has a model field. Most sessions put the
/// model in the first 8-20 KiB; 64 KiB is a safe upper bound.
const MODEL_SCAN_BYTES: u64 = 64 * 1024;

fn extract_model(path: &Path) -> Option<String> {
    use std::io::Read;
    let mut f = std::fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let read_up_to = len.min(MODEL_SCAN_BYTES);
    let mut buf = vec![0u8; read_up_to as usize];
    let _ = f.read(&mut buf).ok()?;
    let text = std::str::from_utf8(&buf).unwrap_or("");
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let v: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if let Some(m) = model_from_value(&v) {
            return Some(m);
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

fn git_repo_root(cwd: &Path) -> Option<PathBuf> {
    // `--show-toplevel` returns the cwd's worktree toplevel, which is the
    // worktree dir for a linked worktree, not the main repo. We want the
    // main repo's toplevel so the repo name is the project name (e.g. `main`),
    // not the worktree branch dir (e.g. `skill-chain-redesign`).
    //
    // `--git-common-dir` returns the shared `.git/` dir for all worktrees
    // of the same repo; `dirname` of that is the main repo's working tree.
    let out = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["rev-parse", "--path-format=absolute", "--git-common-dir"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let git_dir = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if git_dir.is_empty() {
        return None;
    }
    // `.git/` → dirname → main repo's working tree.
    let p = PathBuf::from(&git_dir);
    let parent = p.parent()?;
    Some(parent.to_path_buf())
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
    fn extract_sid_takes_first_eight_of_bare_uuid() {
        let p = Path::new("/h/3447e668-a07b-4f7c-b0e8-dafefaa48374.jsonl");
        assert_eq!(extract_sid(p), "3447e668");
    }

    #[test]
    fn extract_sid_parses_codex_rollout_uuid() {
        let p = Path::new(
            "/h/rollout-2025-09-17T19-05-35-9a6a7c2d-519d-4c07-b9ec-87b9ffd102f0.jsonl",
        );
        assert_eq!(extract_sid(p), "9a6a7c2d");
    }

    #[test]
    fn extract_sid_truncates_long_stem() {
        // Plain long stem with no prefix → first 8 chars.
        let p = Path::new("/h/abcdefghijklmnop.jsonl");
        assert_eq!(extract_sid(p), "abcdefgh");
    }

    // quick_status — exercises the last-event probe used by the text-mode
    // fallback so the dashboard status column matches what the TUI's tail
    // recompute would produce.

    fn write_temp_jsonl(name: &str, content: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "agent-graph-tui-test-{}-{}.jsonl",
            std::process::id(),
            name
        ));
        let _ = std::fs::remove_file(&path); // ensure no leftover from previous run
        std::fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn first_user_message_extracts_initial_prompt() {
        let head = r#"{"type":"user","message":{"role":"user","content":"hello world"}}
{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"hi"}]}}
"#;
        let m = super::first_user_message(head).unwrap();
        assert_eq!(m, "hello world");
    }

    #[test]
    fn first_user_message_handles_escaped_quotes() {
        let head = r#"{"type":"user","message":{"role":"user","content":"say \\\"hi\\\" to me"}}
"#;
        let m = super::first_user_message(head).unwrap();
        assert!(m.contains("hi"));
    }

    #[test]
    fn first_user_message_handles_content_array_text_block() {
        let head = r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"from array"}]}}
"#;
        let m = super::first_user_message(head).unwrap();
        assert_eq!(m, "from array");
    }

    #[test]
    fn first_user_message_handles_codex_envelope() {
        let head = r#"{"type":"response_item","payload":{"type":"user_message","content":[{"type":"input_text","text":"codex prompt"}]}}
"#;
        let m = super::first_user_message(head).unwrap();
        assert_eq!(m, "codex prompt");
    }

    #[test]
    fn first_user_message_returns_none_when_first_line_is_assistant() {
        let head = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"hi"}]}}
"#;
        assert!(super::first_user_message(head).is_none());
    }

    #[test]
    fn extract_model_picks_up_message_model_field() {
        let path = std::env::temp_dir().join(format!("agent-graph-tui-model-msg-{}.jsonl", std::process::id()));
        let _ = std::fs::remove_file(&path);
        std::fs::write(
            &path,
            b"{\"type\":\"assistant\",\"message\":{\"role\":\"assistant\",\"model\":\"claude-opus-4-8\",\"content\":[{\"type\":\"text\",\"text\":\"ok\"}]}}\n",
        )
        .unwrap();
        let m = super::extract_model(&path).unwrap();
        assert_eq!(m, "claude-opus-4-8");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn extract_model_picks_up_payload_model_field_for_codex() {
        let path = std::env::temp_dir().join(format!("agent-graph-tui-model-payload-{}.jsonl", std::process::id()));
        let _ = std::fs::remove_file(&path);
        std::fs::write(
            &path,
            b"{\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"assistant\",\"model\":\"gpt-5.4\",\"content\":[]}}\n",
        )
        .unwrap();
        let m = super::extract_model(&path).unwrap();
        assert_eq!(m, "gpt-5.4");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn extract_model_picks_up_top_level_model_field() {
        let path = std::env::temp_dir().join(format!("agent-graph-tui-model-top-{}.jsonl", std::process::id()));
        let _ = std::fs::remove_file(&path);
        std::fs::write(&path, b"{\"model\":\"minimax-m3\"}\n").unwrap();
        let m = super::extract_model(&path).unwrap();
        assert_eq!(m, "minimax-m3");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn extract_model_returns_none_when_no_model_field_anywhere() {
        let path = std::env::temp_dir().join(format!("agent-graph-tui-model-none-{}.jsonl", std::process::id()));
        let _ = std::fs::remove_file(&path);
        std::fs::write(&path, b"{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"hi\"}}\n").unwrap();
        assert!(super::extract_model(&path).is_none());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn extract_model_picks_up_model_in_later_lines_not_just_head() {
        let path = std::env::temp_dir().join(format!("agent-graph-tui-model-late-{}.jsonl", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let mut body = String::new();
        for _ in 0..50 {
            body.push_str("{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"padding\"}}\n");
        }
        body.push_str("{\"type\":\"assistant\",\"message\":{\"role\":\"assistant\",\"model\":\"claude-opus-4-8\",\"content\":[{\"type\":\"text\",\"text\":\"ok\"}]}}\n");
        std::fs::write(&path, body).unwrap();
        let m = super::extract_model(&path).unwrap();
        assert_eq!(m, "claude-opus-4-8");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn quick_status_assistant_text_is_done() {
        let path = write_temp_jsonl(
            "assistant_text",
            r#"{"type":"user","message":{"role":"user","content":"hi"}}
{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"ok"}]}}
"#,
        );
        let s = super::quick_status(&path);
        assert_eq!(s, tree::SessionStatus::Done);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn quick_status_assistant_thinking_block_is_running() {
        let path = write_temp_jsonl(
            "assistant_thinking",
            "{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"hi\"}}\n\
             {\"type\":\"assistant\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"thinking\",\"text\":\"...\"}]}}\n",
        );
        let s = super::quick_status(&path);
        assert_eq!(s, tree::SessionStatus::Running);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn quick_status_assistant_redacted_thinking_is_running() {
        let path = write_temp_jsonl(
            "assistant_redacted",
            "{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"hi\"}}\n\
             {\"type\":\"assistant\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"redacted_thinking\"}]}}\n",
        );
        let s = super::quick_status(&path);
        assert_eq!(s, tree::SessionStatus::Running);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn quick_status_assistant_tool_use_is_running() {
        let path = write_temp_jsonl(
            "assistant_tool_use",
            r#"{"type":"user","message":{"role":"user","content":"read x"}}
{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"t1","name":"Read","input":{}}]}}
"#,
        );
        let s = super::quick_status(&path);
        assert_eq!(s, tree::SessionStatus::Running);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn quick_status_user_text_is_running() {
        let path = write_temp_jsonl(
            "user_text",
            r#"{"type":"user","message":{"role":"user","content":"continue"}}
"#,
        );
        let s = super::quick_status(&path);
        assert_eq!(s, tree::SessionStatus::Running);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn quick_status_user_tool_result_is_error_is_failed() {
        let path = write_temp_jsonl(
            "user_tool_result_error",
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"t1","name":"Read","input":{}}]}}
{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"err","is_error":true}]}}
"#,
        );
        let s = super::quick_status(&path);
        assert_eq!(s, tree::SessionStatus::Failed);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn quick_status_empty_file_is_done() {
        let path = write_temp_jsonl("empty", "");
        let s = super::quick_status(&path);
        assert_eq!(s, tree::SessionStatus::Done);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn quick_status_garbage_file_is_done() {
        let path = write_temp_jsonl("garbage", "not even json");
        let s = super::quick_status(&path);
        assert_eq!(s, tree::SessionStatus::Done);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn quick_status_uses_last_non_empty_line_at_file_end() {
        let mut buf = String::new();
        for _ in 0..64 {
            buf.push_str(r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"ok"}]}}"#);
            buf.push('\n');
        }
        buf.push_str(r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"t1","name":"Read","input":{}}]}}"#);
        buf.push('\n');
        let path = write_temp_jsonl("buf_end", &buf);
        let s = super::quick_status(&path);
        assert_eq!(s, tree::SessionStatus::Running);
        let _ = std::fs::remove_file(&path);
    }

    // Codex envelope — `quick_status` walks the last 64 KiB of the JSONL
    // and looks at the final non-empty line. With codex's `response_item`
    // envelope, the relevant inner payload lives under `payload.type`.

    #[test]
    fn quick_status_codex_pending_tool_call_is_running() {
        let path = write_temp_jsonl(
            "codex_running",
            "{\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"ok\"]}}}\n\
             {\"type\":\"response_item\",\"payload\":{\"type\":\"custom_tool_call\",\"call_id\":\"call_x\",\"name\":\"exec\",\"input\":\"...\",\"status\":\"in_progress\"}}\n",
        );
        let s = super::quick_status(&path);
        assert_eq!(s, tree::SessionStatus::Running);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn quick_status_codex_tool_error_is_failed() {
        let path = write_temp_jsonl(
            "codex_failed",
            "{\"type\":\"response_item\",\"payload\":{\"type\":\"custom_tool_call\",\"call_id\":\"call_x\",\"name\":\"exec\",\"input\":\"...\",\"status\":\"completed\"}}\n\
             {\"type\":\"response_item\",\"payload\":{\"type\":\"custom_tool_call_output\",\"call_id\":\"call_x\",\"output\":\"oops\",\"is_error\":true}}\n",
        );
        let s = super::quick_status(&path);
        assert_eq!(s, tree::SessionStatus::Failed);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn quick_status_codex_assistant_message_is_done() {
        let path = write_temp_jsonl(
            "codex_done",
            "{\"type\":\"response_item\",\"payload\":{\"type\":\"custom_tool_call\",\"call_id\":\"call_x\",\"name\":\"exec\",\"input\":\"...\",\"status\":\"completed\"}}\n\
             {\"type\":\"response_item\",\"payload\":{\"type\":\"custom_tool_call_output\",\"call_id\":\"call_x\",\"output\":\"ok\"}}\n\
             {\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"all done\"}]}}\n",
        );
        let s = super::quick_status(&path);
        assert_eq!(s, tree::SessionStatus::Done);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn quick_status_codex_user_message_is_running() {
        let path = write_temp_jsonl(
            "codex_user_msg",
            "{\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"ok\"}]}}\n\
             {\"type\":\"response_item\",\"payload\":{\"type\":\"user_message\",\"role\":\"user\",\"content\":[{\"type\":\"input_text\",\"text\":\"continue\"}]}}\n",
        );
        let s = super::quick_status(&path);
        assert_eq!(s, tree::SessionStatus::Running);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn quick_status_codex_event_msg_is_ignored() {
        // Token-count event_msg at the very end shouldn't change the status
        // resolved from the response_item above it.
        let path = write_temp_jsonl(
            "codex_event_msg",
            "{\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"ok\"}]}}\n\
             {\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{}}}\n",
        );
        let s = super::quick_status(&path);
        assert_eq!(s, tree::SessionStatus::Done);
        let _ = std::fs::remove_file(&path);
    }
}
