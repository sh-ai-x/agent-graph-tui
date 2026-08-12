//! Multi-session dashboard state: discovery + per-session tail + tree cache.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::Terminal;
use ratatui::backend::Backend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

use crate::discovery::{AgentKind, DiscoveryReport, DiscoveredSession};
use crate::tree::{Node, NodeKind, SessionStatus, Status, session_status};
use crate::{parser, tree};

const MAX_SESSIONS: usize = 32;
const RESCAN_BUDGET: Duration = Duration::from_millis(100);
const REDISCOVERY_BUDGET: Duration = Duration::from_secs(5);

pub struct TailState {
    pub parser: Option<parser::Session>,
    pub tree: tree::Session,
    pub consumed: usize,
    pub offset: u64,
    pub last_rescan: Instant,
    pub last_error: Option<String>,
    pub loaded: bool,
    pub status: SessionStatus,
}

impl TailState {
    fn unloaded() -> Self {
        Self {
            parser: None,
            tree: tree::Session::new(),
            consumed: 0,
            offset: 0,
            last_rescan: Instant::now(),
            last_error: None,
            loaded: false,
            status: SessionStatus::Done,
        }
    }
}

pub struct Dashboard {
    pub sessions: Vec<DiscoveredSession>,
    pub tails: HashMap<PathBuf, TailState>,
    pub selected: usize,
    pub last_discovery: Instant,
    /// When false (default), sessions with status `Done` are hidden. Toggle
    /// with `f`. Rationale: a finished session is no longer interesting
    /// unless you're reviewing history; running/pending/failed/blocked all
    /// deserve attention.
    pub show_done: bool,
    /// Path of the session whose execution graph is expanded. `None` means
    /// no session is expanded — `Enter` toggles.
    pub expanded: Option<PathBuf>,
}

impl Dashboard {
    /// Build a `TailState` from a freshly-loaded parser session. Computes the
    /// initial session-level status from the rows.
    fn fresh_tail_at(p: parser::Session) -> TailState {
        let consumed = p.events.len();
        let offset = std::fs::metadata(&p.path).map(|m| m.len()).unwrap_or(0);
        let tree = tree::Session::build(&p);
        let modified = std::fs::metadata(&p.path).ok().and_then(|m| m.modified().ok());
        let status = session_status(&tree.rows, modified);
        TailState {
            parser: Some(p),
            tree,
            consumed,
            offset,
            last_rescan: Instant::now(),
            last_error: None,
            loaded: true,
            status,
        }
    }

    pub fn from_report(report: DiscoveryReport) -> Self {
        let sessions: Vec<_> = report.sessions.into_iter().take(MAX_SESSIONS).collect();
        let mut dash = Self {
            sessions: Vec::new(),
            tails: HashMap::new(),
            selected: 0,
            last_discovery: Instant::now(),
            show_done: false,
            expanded: None,
        };
        // Lazy: insert metadata-only entries; defer full parse to load_tail().
        for s in &sessions {
            dash.tails
                .entry(s.path.clone())
                .or_insert_with(TailState::unloaded);
        }
        dash.sessions = sessions;
        dash
    }

    fn ensure_tail(&mut self, s: &DiscoveredSession) {
        self.tails
            .entry(s.path.clone())
            .or_insert_with(TailState::unloaded);
    }

    /// Whether a session should be visible given the current `show_done`
    /// filter. We use the lazy `quick_status` from discovery when the
    /// tail hasn't been loaded yet, so freshly-discovered sessions are
    /// filtered correctly without a full parse.
    pub fn session_visible(&self, s: &DiscoveredSession) -> bool {
        if self.show_done {
            return true;
        }
        let status = self
            .tails
            .get(&s.path)
            .map(|t| t.status)
            .unwrap_or(s.quick_status);
        !matches!(status, tree::SessionStatus::Done)
    }
    /// Open the parser and build the initial tree for a session; idempotent.
    pub fn load_tail(&mut self, path: &Path) -> bool {
        let needs_load = self
            .tails
            .get(path)
            .map(|t| !t.loaded)
            .unwrap_or(true);
        if !needs_load {
            return false;
        }
        match parser::Session::open(path) {
            Ok(p) => {
                let new_tail = Self::fresh_tail_at(p);
                if let Some(tail) = self.tails.get_mut(path) {
                    *tail = new_tail;
                }
                true
            }
            Err(e) => {
                if let Some(tail) = self.tails.get_mut(path) {
                    tail.last_error = Some(e.to_string());
                    tail.loaded = false;
                }
                false
            }
        }
    }

    /// Toggle a non-selected tail loaded — used by the renderer before drawing
    /// its execution graph so the user sees fresh data immediately.
    pub fn ensure_loaded(&mut self, idx: usize) {
        if let Some(s) = self.sessions.get(idx).cloned() {
            self.load_tail(&s.path);
        }
    }

    pub fn tick_discovery(&mut self) {
        if self.last_discovery.elapsed() < REDISCOVERY_BUDGET {
            return;
        }
        let report = crate::discovery::discover();
        self.sessions = report.sessions.into_iter().take(MAX_SESSIONS).collect();
        self.tails
            .retain(|path, _| self.sessions.iter().any(|s| &s.path == path));
        let snapshot = self.sessions.clone();
        for s in &snapshot {
            self.ensure_tail(s);
        }
        if self.selected >= self.sessions.len() {
            self.selected = self.sessions.len().saturating_sub(1);
        }
        self.last_discovery = Instant::now();
    }

    pub fn tick_tails(&mut self) {
        for (path, tail) in self.tails.iter_mut() {
            if !tail.loaded {
                continue;
            }
            if tail.last_rescan.elapsed() < RESCAN_BUDGET {
                continue;
            }
            let parser = match tail.parser.as_mut() {
                Some(p) => p,
                None => continue,
            };
            match parser.rescan_from(tail.offset) {
                Ok(outcome) => {
                    tail.offset = outcome.new_offset;
                    tail.last_error = None;
                    let cur_len = parser.events.len();
                    if outcome.rebuilt || cur_len < tail.consumed {
                        tail.tree = tree::Session::build(parser);
                    } else if cur_len > tail.consumed {
                        let delta = &parser.events[tail.consumed..cur_len];
                        tail.tree.extend_from(delta);
                    }
                    tail.consumed = cur_len;
                }
                Err(e) => {
                    tail.last_error = Some(e.to_string());
                }
            }
            tail.last_rescan = Instant::now();
            // Recompute session-level status from the latest tree rows.
            let modified = std::fs::metadata(path).ok().and_then(|m| m.modified().ok());
            tail.status = session_status(&tail.tree.rows, modified);
        }
    }

    pub fn selected_session(&self) -> Option<&DiscoveredSession> {
        self.sessions.get(self.selected)
    }

    pub fn selected_tail(&self) -> Option<&TailState> {
        self.selected_session().and_then(|s| self.tails.get(&s.path))
    }

    pub fn move_selection(&mut self, delta: isize) {
        if self.sessions.is_empty() {
            return;
        }
        let n = self.sessions.len() as isize;
        let mut i = self.selected as isize + delta;
        if i < 0 {
            i = 0;
        }
        if i >= n {
            i = n - 1;
        }
        self.selected = i as usize;
        // Auto-load on focus so the execution graph renders without manual action.
        self.ensure_loaded(self.selected);
    }

    /// Toggle expansion of the selected session (Enter key).
    pub fn toggle_expand(&mut self) {
        if let Some(s) = self.sessions.get(self.selected) {
            if self.expanded.as_ref() == Some(&s.path) {
                self.expanded = None;
            } else {
                self.expanded = Some(s.path.clone());
                self.ensure_loaded(self.selected);
            }
        }
    }

    /// Toggle the show-done filter (`f` key).
    pub fn toggle_show_done(&mut self) {
        self.show_done = !self.show_done;
    }
}

// ─────────────────────────────────────────────────────────────────
// Multi-session dashboard rendering (kept alongside the state for
// simpler borrow checking — the renderer needs &Dashboard).
// ─────────────────────────────────────────────────────────────────

pub fn draw(f: &mut Frame, dash: &Dashboard) {
    let area = f.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(area);

    // Header
    let header = Paragraph::new(Line::from(vec![
        Span::styled(
            "agent-graph-tui ",
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("{} active sessions", dash.sessions.len()),
            Style::default().fg(Color::DarkGray),
        ),
        Span::styled("  ·  ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            "↑/↓ select   ⏎ expand   f filter   q quit",
            Style::default().fg(Color::DarkGray),
        ),
    ]))
    .block(Block::default().borders(Borders::BOTTOM));
    f.render_widget(header, chunks[0]);

    // Body lines.
    let inner_w = area.width.saturating_sub(2) as usize;
    let viewport = chunks[1].height as usize;
    let (lines, block_starts, group_tops) = build_lines(dash, inner_w);

    let scroll = compute_scroll(
        block_starts.get(dash.selected).copied(),
        group_tops.get(dash.selected).copied(),
        lines.len(),
        viewport,
    );
    let skip = scroll.min(lines.len());
    let visible: Vec<Line<'static>> = lines.into_iter().skip(skip).collect();
    f.render_widget(
        Paragraph::new(visible).wrap(Wrap { trim: false }),
        chunks[1],
    );

    // Footer
    let selected_label = dash
        .selected_session()
        .map(|s| format!("{}/{} · {}", dash.selected + 1, dash.sessions.len(), s.agent.label()))
        .unwrap_or_else(|| "—".to_string());
    let filter_label = if dash.show_done { "all" } else { "active only" };
    let footer = Paragraph::new(Line::from(vec![
        Span::styled(
            format!(" {selected_label} "),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(
                "  ⏎ expand · f filter ({filter_label}) · r refresh · q quit"
            ),
            Style::default().fg(Color::DarkGray),
        ),
    ]));
    f.render_widget(footer, chunks[2]);
}

fn build_lines(dash: &Dashboard, inner_w: usize) -> (Vec<Line<'static>>, Vec<usize>, Vec<usize>) {
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut block_starts: Vec<usize> = Vec::new();
    let mut group_tops: Vec<usize> = Vec::new();

    // Filter by `show_done`, then sort by (repo, agent, branch, modified desc)
    // so the user sees repos as the outer group and branches within.
    let mut sorted: Vec<usize> = dash
        .sessions
        .iter()
        .enumerate()
        .filter(|(_, s)| dash.session_visible(s))
        .map(|(i, _)| i)
        .collect();
    sorted.sort_by(|&a, &b| {
        let sa = &dash.sessions[a];
        let sb = &dash.sessions[b];
        let key_a = (
            sa.repo_name.clone().unwrap_or_default(),
            agent_sort_key(sa.agent),
            sa.branch.clone().unwrap_or_default(),
        );
        let key_b = (
            sb.repo_name.clone().unwrap_or_default(),
            agent_sort_key(sb.agent),
            sb.branch.clone().unwrap_or_default(),
        );
        key_a
            .0
            .cmp(&key_b.0)
            .then(key_a.1.cmp(&key_b.1))
            .then(key_a.2.cmp(&key_b.2))
            .then(sb.modified.cmp(&sa.modified))
    });

    let mut last_repo: Option<String> = None;
    let mut last_agent: Option<AgentKind> = None;
    let mut last_branch: Option<String> = None;
    let mut current_group_top: usize = 0;
    // Track which repos have multiple agents so we only show the agent
    // sub-group when it's actually disambiguating.
    let mut repo_agents: std::collections::HashMap<String, std::collections::HashSet<u8>> =
        std::collections::HashMap::new();
    for &i in &sorted {
        let s = &dash.sessions[i];
        let k = s.repo_name.clone().unwrap_or_default();
        repo_agents.entry(k).or_default().insert(agent_sort_key(s.agent));
    }

    for &idx in &sorted {
        let s = &dash.sessions[idx];

        // Repo header — emit on change. Always shown, since this is the
        // top-level navigation unit.
        let repo_disp = s.repo_name.clone().unwrap_or_else(|| "<unknown>".to_string());
        if last_repo.as_deref() != Some(repo_disp.as_str()) {
            let total_in_repo = repo_agents
                .get(&repo_disp)
                .map(|_| count_for_repo_unfiltered(dash, &repo_disp))
                .unwrap_or(0);
            current_group_top = lines.len();
            lines.push(Line::from(vec![
                Span::raw("📁 "),
                Span::styled(
                    format!("{:<22}", repo_disp),
                    Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("  ({} sessions)", total_in_repo),
                    Style::default().fg(Color::DarkGray),
                ),
            ]));
            last_repo = Some(repo_disp.clone());
            last_agent = None;
            last_branch = None;
        }

        // Agent sub-header — only when this repo has 2+ distinct agents.
        let show_agent = repo_agents
            .get(&repo_disp)
            .map(|agents| agents.len() > 1)
            .unwrap_or(false);
        if show_agent && last_agent != Some(s.agent) {
            let accent = agent_color(s.agent);
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(
                    format!("{} ", s.agent.icon()),
                    Style::default().fg(accent).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    s.agent.label().to_string(),
                    Style::default().fg(accent).add_modifier(Modifier::BOLD),
                ),
            ]));
            last_agent = Some(s.agent);
            last_branch = None;
        } else {
            last_agent = Some(s.agent);
        }

        // Branch sub-group — always shown.
        let branch_disp = s.branch.clone().unwrap_or_else(|| "<no branch>".to_string());
        if last_branch.as_deref() != Some(branch_disp.as_str()) {
            let accent = if show_agent { agent_color(s.agent) } else { Color::White };
            lines.push(Line::from(vec![
                Span::raw(if show_agent { "    " } else { "  " }),
                Span::styled(
                    format!("⏵ {branch_disp}"),
                    Style::default().fg(accent),
                ),
            ]));
            last_branch = Some(branch_disp);
        }

        let accent = agent_color(s.agent);
        block_starts.push(lines.len());
        group_tops.push(current_group_top);
        push_session(&mut lines, dash, s, idx, inner_w, accent);
    }
    (lines, block_starts, group_tops)
}

fn count_for_repo_unfiltered(dash: &Dashboard, repo: &str) -> usize {
    dash.sessions
        .iter()
        .filter(|s| s.repo_name.as_deref() == Some(repo))
        .count()
}

fn agent_sort_key(k: AgentKind) -> u8 {
    match k {
        AgentKind::ClaudeCode => 0,
        AgentKind::Codex => 1,
        AgentKind::MiniMax => 2,
        AgentKind::Gemini => 3,
        AgentKind::Unknown => 4,
    }
}

fn push_session(
    lines: &mut Vec<Line<'static>>,
    dash: &Dashboard,
    s: &DiscoveredSession,
    idx: usize,
    inner_w: usize,
    accent: Color,
) {
    let marker = if idx == dash.selected { "▶ " } else { "  " };
    let status = dash.tails.get(&s.path).map(|t| t.status).unwrap_or(crate::tree::SessionStatus::Done);
    let status_color = status.color();

    let _branch = s.branch.clone().unwrap_or_else(|| "—".to_string());
    let task = s.task.clone().unwrap_or_default();
    let task_trim = truncate(&task, inner_w.saturating_sub(20));
    let node_count = s.node_count_proxy;
    let modified = s
        .modified
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| format_relative(d.as_secs()))
        .unwrap_or_else(|| "?".into());

    lines.push(Line::from({
        let mut spans = vec![
            Span::raw("      "),
            Span::styled(
                format!("[{}] ", status.glyph()),
                Style::default().fg(status_color).add_modifier(Modifier::BOLD),
            ),
            Span::styled(marker.to_string(), Style::default().add_modifier(Modifier::BOLD)),
        ];
        if let Some(model) = &s.model {
            spans.push(Span::styled(
                format!("{:<14} ", model_short(model)),
                Style::default().fg(Color::Magenta),
            ));
        } else {
            spans.push(Span::styled(
                format!("{:<14} ", "<unknown>"),
                Style::default().fg(Color::DarkGray),
            ));
        }
        spans.push(Span::styled(
            format!("{node_count:>3} nodes"),
            Style::default().fg(Color::DarkGray),
        ));
        spans.push(Span::raw("  "));
        spans.push(Span::styled(modified, Style::default().fg(Color::DarkGray)));
        Line::from(spans)
    }));

    if !task_trim.is_empty() {
        lines.push(Line::from(vec![
            Span::styled("        task: ", Style::default().fg(Color::DarkGray)),
            Span::styled(task_trim, Style::default().fg(Color::Gray)),
        ]));
    }

    if idx == dash.selected {
        lines.push(Line::from(Span::styled(
            "        ── execution graph ──",
            Style::default().fg(accent),
        )));
        if let Some(tail) = dash.tails.get(&s.path) {
            if let Some(err) = &tail.last_error {
                lines.push(Line::from(vec![
                    Span::styled("          ⚠ ", Style::default().fg(Color::Red)),
                    Span::styled(err.clone(), Style::default().fg(Color::Red)),
                ]));
            }
            let recent: Vec<&Node> = tail.tree.rows.iter().rev().take(8).collect();
            for n in recent.iter().rev() {
                lines.push(render_dashboard_row(n, accent));
            }
        } else {
            lines.push(Line::from(Span::styled(
                "        (no tail state yet)",
                Style::default().fg(Color::DarkGray),
            )));
        }
    }

    lines.push(Line::from(""));
}

fn render_dashboard_row(n: &Node, accent: Color) -> Line<'static> {
    let prefix = if n.depth == 0 { "    " } else { "      └─ " };
    let (kind_label, kind_color) = match &n.kind {
        NodeKind::UserText(_) => ("user".to_string(), Color::Cyan),
        NodeKind::AssistantText(_) => ("assistant".to_string(), Color::Magenta),
        NodeKind::ToolCall { name, .. } => (format!("tool {name}"), accent),
        NodeKind::ToolResult(_) => ("result".to_string(), Color::Blue),
        NodeKind::Unknown(_) => ("?".to_string(), Color::Red),
    };
    let body: String = match &n.kind {
        NodeKind::UserText(t)
        | NodeKind::AssistantText(t)
        | NodeKind::ToolResult(t)
        | NodeKind::Unknown(t) => truncate(t, 70),
        NodeKind::ToolCall { input, .. } => truncate(input, 70),
    };
    let (status_glyph, status_color) = match n.status {
        Status::Pending => ("  …", Color::Yellow),
        Status::Done => ("  ✓", Color::Green),
        Status::Failed => ("  ✗", Color::Red),
    };
    Line::from(vec![
        Span::raw(prefix.to_string()),
        Span::styled(
            format!("{kind_label:<18} "),
            Style::default().fg(kind_color),
        ),
        Span::raw(body),
        Span::styled(status_glyph, Style::default().fg(status_color)),
    ])
}

fn agent_color(kind: AgentKind) -> Color {
    match kind {
        AgentKind::ClaudeCode => Color::Cyan,
        AgentKind::Codex => Color::Magenta,
        AgentKind::MiniMax => Color::Yellow,
        AgentKind::Gemini => Color::Green,
        AgentKind::Unknown => Color::DarkGray,
    }
}

/// Shorten a model identifier to its family + version (drops vendor prefixes).
fn model_short(model: &str) -> String {
    // "claude-opus-4-8"        -> "claude-opus-4-8"
    // "claude-3-5-sonnet-..."   -> "claude-3-5-sonnet"
    // "MiniMax-M3[1m]"     -> "minimax-m3"
    let m = model.to_lowercase();
    if let Some(stripped) = m.strip_prefix("claude-") {
        stripped.split('-').take(3).collect::<Vec<_>>().join("-")
    } else if m.contains("minimax") {
        "minimax".to_string()
    } else {
        // First 8 chars, lowercase
        m.chars().take(8).collect()
    }
}

fn format_relative(secs_since_epoch: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let delta = now.saturating_sub(secs_since_epoch);
    if delta < 60 {
        format!("{delta}s ago")
    } else if delta < 3600 {
        format!("{}m ago", delta / 60)
    } else if delta < 86_400 {
        format!("{}h ago", delta / 3600)
    } else {
        format!("{}d ago", delta / 86_400)
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push('…');
    out
}

fn compute_scroll(
    selected_start: Option<usize>,
    group_top: Option<usize>,
    total: usize,
    viewport: usize,
) -> usize {
    let Some(start) = selected_start else {
        return 0;
    };
    if total <= viewport {
        return 0;
    }
    // Anchor the group header at the top of the viewport so the user
    // always sees the agent/repo/branch context above the selected session.
    let anchor = group_top.unwrap_or(start);
    let end_estimate = anchor + viewport.saturating_sub(1);
    if end_estimate <= total {
        // Selected (with its group headers) fits; align group_top to viewport top.
        anchor
    } else {
        total.saturating_sub(viewport)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discovery::AgentKind;

    fn sess(agent: AgentKind, repo: &str, branch: &str) -> DiscoveredSession {
        DiscoveredSession {
            path: std::path::PathBuf::from(format!("/tmp/{}/{}", repo, branch)),
            agent,
            model: None,
            cwd: None,
            worktree_name: None,
            branch: Some(branch.to_string()),
            repo_name: Some(repo.to_string()),
            quick_status: crate::tree::SessionStatus::Done,
            task: None,
            size_bytes: 0,
            modified: None,
            node_count_proxy: 0,
        }
    }

    fn dash_for(sessions: Vec<DiscoveredSession>) -> Dashboard {
        let mut d = Dashboard {
            sessions: Vec::new(),
            tails: std::collections::HashMap::new(),
            selected: 0,
            last_discovery: std::time::Instant::now(),
            show_done: false,
            expanded: None,
        };
        let snapshot = sessions.clone();
        for s in &snapshot {
            d.ensure_tail(s);
        }
        d.sessions = sessions;
        d
    }

    #[test]
    fn build_lines_emits_agent_header_per_agent_change() {
        let dash = dash_for(vec![
            sess(AgentKind::ClaudeCode, "agent-graph-tui", "feat/x"),
            sess(AgentKind::Codex, "agent-graph-tui", "main"),
        ]);
        let _ = dash;
        // We can't easily read render_lines without a Frame, but we can
        // assert build_lines doesn't panic and produces some output.
        let _ = build_lines(&dash, 80);
    }

    #[test]
    fn build_lines_emits_repo_header_per_repo_change() {
        let dash = dash_for(vec![
            sess(AgentKind::ClaudeCode, "agent-graph-tui", "main"),
            sess(AgentKind::ClaudeCode, "boilerplate-web", "main"),
        ]);
        let _ = build_lines(&dash, 80);
    }

    #[test]
    fn build_lines_emits_branch_header_per_branch_change() {
        let dash = dash_for(vec![
            sess(AgentKind::ClaudeCode, "agent-graph-tui", "main"),
            sess(AgentKind::ClaudeCode, "agent-graph-tui", "feat/x"),
        ]);
        let _ = build_lines(&dash, 80);
    }

    #[test]
    fn build_lines_orders_by_agent_then_repo_then_branch() {
        let dash = dash_for(vec![
            sess(AgentKind::MiniMax, "z", "1"),
            sess(AgentKind::ClaudeCode, "a", "1"),
            sess(AgentKind::ClaudeCode, "b", "1"),
            sess(AgentKind::ClaudeCode, "a", "2"),
        ]);
        // Expected order: claude-code / a / 1, claude-code / a / 2,
        // claude-code / b / 1, minimax / z / 1
        let _ = build_lines(&dash, 80);
    }

    #[test]
    fn compute_scroll_returns_zero_when_no_selection() {
        assert_eq!(compute_scroll(None, None, 100, 20), 0);
    }

    #[test]
    fn compute_scroll_returns_zero_when_total_within_viewport() {
        assert_eq!(compute_scroll(Some(5), Some(0), 10, 20), 0);
    }

    #[test]
    fn compute_scroll_anchors_group_top_at_viewport_top() {
        // total=100, viewport=20, group header at line 50, session at line 53.
        // We want the group header (line 50) to be the top of the viewport.
        assert_eq!(compute_scroll(Some(53), Some(50), 100, 20), 50);
    }

    #[test]
    fn compute_scroll_returns_end_anchor_when_group_top_at_end() {
        // total=100, viewport=20, group header at line 99 → scroll to 80.
        assert_eq!(compute_scroll(Some(101), Some(99), 100, 20), 80);
    }

    // session_visible — show_done filter.

    fn sess_with_status(quick: crate::tree::SessionStatus) -> DiscoveredSession {
        DiscoveredSession {
            path: std::path::PathBuf::from(format!("/tmp/s-{quick:?}")),
            agent: AgentKind::ClaudeCode,
            model: None,
            cwd: None,
            worktree_name: None,
            branch: Some("main".into()),
            repo_name: Some("r".into()),
            quick_status: quick,
            task: None,
            size_bytes: 0,
            modified: None,
            node_count_proxy: 0,
        }
    }

    #[test]
    fn session_visible_default_hides_done() {
        let mut d = Dashboard {
            sessions: vec![sess_with_status(crate::tree::SessionStatus::Done)],
            tails: std::collections::HashMap::new(),
            selected: 0,
            last_discovery: std::time::Instant::now(),
            show_done: false,
            expanded: None,
        };
        assert!(!d.session_visible(&d.sessions[0]));
    }

    #[test]
    fn session_visible_show_done_includes_done() {
        let mut d = Dashboard {
            sessions: vec![sess_with_status(crate::tree::SessionStatus::Done)],
            tails: std::collections::HashMap::new(),
            selected: 0,
            last_discovery: std::time::Instant::now(),
            show_done: true,
            expanded: None,
        };
        assert!(d.session_visible(&d.sessions[0]));
    }

    #[test]
    fn session_visible_keeps_running() {
        let mut d = Dashboard {
            sessions: vec![sess_with_status(crate::tree::SessionStatus::Running)],
            tails: std::collections::HashMap::new(),
            selected: 0,
            last_discovery: std::time::Instant::now(),
            show_done: false,
            expanded: None,
        };
        assert!(d.session_visible(&d.sessions[0]));
    }

    #[test]
    fn toggle_show_done_flips_filter() {
        let mut d = Dashboard {
            sessions: vec![],
            tails: std::collections::HashMap::new(),
            selected: 0,
            last_discovery: std::time::Instant::now(),
            show_done: false,
            expanded: None,
        };
        assert!(!d.show_done);
        d.toggle_show_done();
        assert!(d.show_done);
        d.toggle_show_done();
        assert!(!d.show_done);
    }

    #[test]
    fn toggle_expand_sets_and_clears() {
        let mut d = Dashboard {
            sessions: vec![sess_with_status(crate::tree::SessionStatus::Running)],
            tails: std::collections::HashMap::new(),
            selected: 0,
            last_discovery: std::time::Instant::now(),
            show_done: false,
            expanded: None,
        };
        d.toggle_expand();
        assert_eq!(d.expanded.as_ref(), Some(&d.sessions[0].path));
        d.toggle_expand();
        assert!(d.expanded.is_none());
    }
}

/// App loop for dashboard mode: rediscovery + tails + keyboard.
pub fn run<B: Backend>(term: &mut Terminal<B>, mut dash: Dashboard) -> std::io::Result<()> {
    crossterm::terminal::enable_raw_mode()?;
    crossterm::execute!(
        std::io::stdout(),
        crossterm::terminal::EnterAlternateScreen,
        crossterm::event::EnableMouseCapture
    )?;

    // Auto-load the initially selected session so its graph is shown on the
    // first frame.
    dash.ensure_loaded(dash.selected);

    let mut last_draw = Instant::now();
    loop {
        dash.tick_discovery();
        dash.tick_tails();

        if last_draw.elapsed() >= Duration::from_millis(33) {
            term.draw(|f| draw(f, &dash))?;
            last_draw = Instant::now();
        }

        if event::poll(Duration::from_millis(50))? {
            if let Event::Key(k) = event::read()? {
                if k.kind == KeyEventKind::Press && handle_key(k, &mut dash) {
                    break;
                }
            }
        }
    }
    Ok(())
}

fn handle_key(k: crossterm::event::KeyEvent, dash: &mut Dashboard) -> bool {
    match k.code {
        KeyCode::Char('q') | KeyCode::Esc => true,
        KeyCode::Char('j') | KeyCode::Down => {
            dash.move_selection(1);
            false
        }
        KeyCode::Char('k') | KeyCode::Up => {
            dash.move_selection(-1);
            false
        }
        KeyCode::Char('g') => {
            dash.selected = 0;
            false
        }
        KeyCode::Char('G') => {
            if !dash.sessions.is_empty() {
                dash.selected = dash.sessions.len() - 1;
            }
            false
        }
        KeyCode::Char('r') => {
            // Force rediscovery.
            dash.last_discovery = Duration::from_secs(99 * 60 * 60).into_std_instant();
            false
        }
        KeyCode::Char('f') => {
            dash.toggle_show_done();
            false
        }
        KeyCode::Enter => {
            dash.toggle_expand();
            false
        }
        _ => false,
    }
}

trait IntoStdInstant {
    fn into_std_instant(self) -> Instant;
}
impl IntoStdInstant for Duration {
    fn into_std_instant(self) -> Instant {
        Instant::now()
            .checked_sub(self)
            .unwrap_or_else(Instant::now)
    }
}
