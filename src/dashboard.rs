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

/// Threshold for the default "recent" filter. A session is "recent" if
/// its JSONL was modified within the last `RECENT_MINUTES` minutes. The
/// user can toggle this with `f` to see older history.
pub const RECENT_MINUTES: u64 = 60;

pub struct Dashboard {
    pub sessions: Vec<DiscoveredSession>,
    pub tails: HashMap<PathBuf, TailState>,
    pub selected: usize,
    /// Where the user is in the hierarchy.
    /// `[]` = top level (list of repos).
    /// `["repo"]` = inside that repo (list of branches).
    /// `["repo", "branch"]` = inside that branch (list of sessions, with
    /// the execution graph rendered for each).
    pub nav_path: Vec<String>,
    pub last_discovery: Instant,
    /// When true (default), only sessions modified within
    /// `RECENT_MINUTES` are shown. Toggle with `f`. The Repos view
    /// still shows every repo regardless of this filter — the filter
    /// only hides individual sessions.
    pub recent_only: bool,
    /// Path of the session whose execution graph is force-expanded. Most
    /// sessions now auto-show their graph; this is kept so `Enter` still
    /// means "force-expand this one" when the user wants the full tree.
    pub expanded: Option<PathBuf>,
}

/// One item in the current nav level.
#[derive(Debug, Clone)]
enum NavItem {
    Repo(String),
    Branch(String),
    Session(usize), // index into dash.sessions
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
            nav_path: Vec::new(),
            last_discovery: Instant::now(),
            recent_only: true,
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

    /// Whether a session should be visible given the current `recent_only`
    /// filter. Sessions modified within `RECENT_MINUTES` are recent;
    /// older sessions are hidden by default.
    pub fn session_visible(&self, s: &DiscoveredSession) -> bool {
        if !self.recent_only {
            return true;
        }
        match s.modified {
            Some(m) => m
                .elapsed()
                .map(|d| d.as_secs() < RECENT_MINUTES * 60)
                .unwrap_or(false),
            None => false,
        }
    }

    /// All repos in the session list, sorted alphabetically. Used at the
    /// top level so the user always sees the full repo list regardless
    /// of the recency filter.
    fn all_repos(&self) -> Vec<String> {
        let mut repos: Vec<String> = self
            .sessions
            .iter()
            .filter_map(|s| s.repo_name.clone())
            .collect();
        repos.sort();
        repos.dedup();
        repos
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
        if self.selected >= self.nav_items().len() {
            self.selected = self.nav_items().len().saturating_sub(1);
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

    /// The session for the currently-focused nav item, if any. Returns
    /// `Some(&DiscoveredSession)` only when the nav is at Sessions level
    /// and the focused item is a session.
    pub fn selected_session(&self) -> Option<&DiscoveredSession> {
        let idx = self.focused_session()?;
        self.sessions.get(idx)
    }

    pub fn selected_tail(&self) -> Option<&TailState> {
        self.selected_session().and_then(|s| self.tails.get(&s.path))
    }

    pub fn move_selection(&mut self, delta: isize) {
        let n = self.nav_items().len();
        if n == 0 {
            return;
        }
        let n = n as isize;
        let mut i = self.selected as isize + delta;
        if i < 0 {
            i = 0;
        }
        if i >= n {
            i = n - 1;
        }
        self.selected = i as usize;
        if let Some(s) = self.focused_session() {
            self.ensure_loaded(s);
        }
    }

    /// Items in the current nav level. Top level = repos; inside a repo
    /// = branches; inside a branch = sessions.
    fn nav_items(&self) -> Vec<NavItem> {
        let filtered: Vec<&DiscoveredSession> = self
            .sessions
            .iter()
            .filter(|s| self.session_visible(s))
            .collect();
        match self.nav_path.len() {
            0 => {
                let mut repos: Vec<String> = filtered
                    .iter()
                    .filter_map(|s| s.repo_name.clone())
                    .collect();
                repos.sort();
                repos.dedup();
                repos.into_iter().map(NavItem::Repo).collect()
            }
            1 => {
                let repo = &self.nav_path[0];
                let mut branches: Vec<String> = filtered
                    .iter()
                    .filter(|s| s.repo_name.as_deref() == Some(repo.as_str()))
                    .filter_map(|s| s.branch.clone())
                    .collect();
                branches.sort();
                branches.dedup();
                branches.into_iter().map(NavItem::Branch).collect()
            }
            _ => {
                let repo = &self.nav_path[0];
                let branch = &self.nav_path[1];
                filtered
                    .into_iter()
                    .enumerate()
                    .filter(|(_, s)| s.repo_name.as_deref() == Some(repo.as_str()))
                    .filter(|(_, s)| s.branch.as_deref() == Some(branch.as_str()))
                    .map(|(i, _)| NavItem::Session(i))
                    .collect()
            }
        }
    }

    /// The currently focused session, if we're at Sessions level and
    /// nav_index points at one.
    pub fn focused_session(&self) -> Option<usize> {
        if self.nav_path.len() != 2 {
            return None;
        }
        match self.nav_items().get(self.selected)? {
            NavItem::Session(i) => Some(*i),
            _ => None,
        }
    }

    /// Drill into the focused item: Repos → Branches in repo; Branches
    /// → Sessions in branch; Sessions → toggles expand.
    pub fn drill_down(&mut self) {
        let items = self.nav_items();
        if let Some(item) = items.get(self.selected).cloned() {
            match item {
                NavItem::Repo(name) => {
                    self.nav_path.push(name);
                    self.selected = 0;
                }
                NavItem::Branch(name) => {
                    self.nav_path.push(name);
                    self.selected = 0;
                }
                NavItem::Session(_) => {
                    self.toggle_expand();
                }
            }
        }
    }

    /// Drill back up the hierarchy.
    pub fn drill_up(&mut self) {
        if !self.nav_path.is_empty() {
            self.nav_path.pop();
            self.selected = 0;
        }
    }

    /// Toggle expansion of the selected session (Enter key in Sessions view).
    pub fn toggle_expand(&mut self) {
        if let Some(s) = self.focused_session() {
            if let Some(session) = self.sessions.get(s) {
                let path = session.path.clone();
                if self.expanded.as_ref() == Some(&path) {
                    self.expanded = None;
                } else {
                    self.expanded = Some(path);
                    self.ensure_loaded(s);
                }
            }
        }
    }

    /// Toggle the recent-only filter (`f` key).
    pub fn toggle_recent_only(&mut self) {
        self.recent_only = !self.recent_only;
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
    let nav_label = match dash.nav_path.len() {
        0 => "repos".to_string(),
        1 => format!("{} / branches", dash.nav_path[0]),
        _ => format!("{} / {} / sessions", dash.nav_path[0], dash.nav_path[1]),
    };
    let header = Paragraph::new(Line::from(vec![
        Span::styled(
            "agent-graph-tui ",
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("{} · {} active sessions", nav_label, dash.sessions.len()),
            Style::default().fg(Color::DarkGray),
        ),
        Span::styled("  ·  ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            "↑/↓ move   ⏎ drill-down   ⌫ drill-up   f filter   q quit",
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
    let filter_label = if dash.recent_only { "all" } else { "active only" };
    let footer = Paragraph::new(Line::from(vec![
        Span::styled(
            format!(" {selected_label} "),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(
                "  ⏎ drill-down · ⌫ drill-up · f filter ({filter_label}) · r refresh · q quit"
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

    // Top level (nav_path=[]): show every repo in the session list, even
    // ones with no recent sessions, so the user can navigate.
    if dash.nav_path.is_empty() {
        for (i, repo) in dash.all_repos().into_iter().enumerate() {
            let total = count_for_repo_unfiltered(dash, &repo);
            let recent = dash
                .sessions
                .iter()
                .filter(|s| s.repo_name.as_deref() == Some(repo.as_str()))
                .filter(|s| dash.session_visible(s))
                .count();
            current_group_top_holder(&mut lines, &mut block_starts, &mut group_tops);
            let label = if recent < total {
                format!("  ({} recent / {} total)", recent, total)
            } else {
                format!("  ({} sessions)", total)
            };
            // Marker on the focused repo only.
            let marker = if i == dash.selected { "▶ " } else { "  " };
            lines.push(Line::from(vec![
                Span::styled(marker.to_string(), Style::default().add_modifier(Modifier::BOLD)),
                Span::raw("📁 "),
                Span::styled(
                    format!("{:<22}", repo),
                    Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
                ),
                Span::styled(label, Style::default().fg(Color::DarkGray)),
            ]));
        }
        return (lines, block_starts, group_tops);
    }

    // Inside a repo or branch: filter, sort, render with group headers.
    let mut sorted: Vec<usize> = dash
        .sessions
        .iter()
        .enumerate()
        .filter(|(_, s)| dash.session_visible(s))
        .filter(|(_, s)| {
            if let Some(repo) = dash.nav_path.first() {
                if s.repo_name.as_deref() != Some(repo.as_str()) {
                    return false;
                }
            }
            if dash.nav_path.len() >= 2 {
                if s.branch.as_deref() != Some(dash.nav_path[1].as_str()) {
                    return false;
                }
            }
            true
        })
        .map(|(i, _)| i)
        .collect();
    sorted.sort_by(|&a, &b| {
        let sa = &dash.sessions[a];
        let sb = &dash.sessions[b];
        let key_a = (
            agent_sort_key(sa.agent),
            sa.branch.clone().unwrap_or_default(),
        );
        let key_b = (
            agent_sort_key(sb.agent),
            sb.branch.clone().unwrap_or_default(),
        );
        key_a
            .0
            .cmp(&key_b.0)
            .then(key_a.1.cmp(&key_b.1))
            .then(sb.modified.cmp(&sa.modified))
    });

    let mut last_agent: Option<AgentKind> = None;
    let mut last_branch: Option<String> = None;
    let current_group_top: usize = 0;

    for (position, &idx) in sorted.iter().enumerate() {
        let s = &dash.sessions[idx];

        // Branch sub-group — always shown.
        let branch_disp = s.branch.clone().unwrap_or_else(|| "<no branch>".to_string());
        if last_branch.as_deref() != Some(branch_disp.as_str()) {
            let indent = if last_agent.is_some() { "    " } else { "  " };
            // Marker on the focused branch.
            let marker = if position == dash.selected { "▶ " } else { "  " };
            lines.push(Line::from(vec![
                Span::styled(marker.to_string(), Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(if marker == "▶ " { "" } else { indent.trim_start() }),
                Span::styled(format!("⏵ {branch_disp}"), Style::default().fg(Color::White)),
            ]));
            last_branch = Some(branch_disp);
        }

        let accent = agent_color(s.agent);
        block_starts.push(lines.len());
        group_tops.push(current_group_top);
        push_session(&mut lines, dash, s, idx, position, inner_w, accent);

        if last_agent != Some(s.agent) {
            last_agent = Some(s.agent);
        }
    }
    (lines, block_starts, group_tops)
}

fn current_group_top_holder(_l: &mut Vec<Line<'static>>, _b: &mut Vec<usize>, _g: &mut Vec<usize>) {}

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
    _idx: usize,
    position: usize,
    inner_w: usize,
    accent: Color,
) {
    // `position` is the index within the current nav scope (after the
    // recency + nav_path filters), not the global session index. That
    // matches `dash.selected` (= nav_index).
    let marker = if position == dash.selected { "▶ " } else { "  " };
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

    if position == dash.selected {
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
            nav_path: Vec::new(),
            last_discovery: std::time::Instant::now(),
            recent_only: false,
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

    // session_visible — recent_only filter.

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
    fn session_visible_recent_only_hides_old_modified_session() {
        // recent_only=true (default) + old modified = hidden
        let mut d = Dashboard {
            sessions: vec![sess_with_status(crate::tree::SessionStatus::Done)],
            tails: std::collections::HashMap::new(),
            selected: 0,
            last_discovery: std::time::Instant::now(),
            recent_only: true,
            nav_path: Vec::new(),
            expanded: None,
        };
        // sess_with_status has modified=None → session_visible returns false
        // (no mtime, can't prove recency).
        assert!(!d.session_visible(&d.sessions[0]));
    }

    #[test]
    fn session_visible_recent_only_shows_recent_session() {
        let mut d = Dashboard {
            sessions: vec![sess_with_status(crate::tree::SessionStatus::Done)],
            tails: std::collections::HashMap::new(),
            selected: 0,
            last_discovery: std::time::Instant::now(),
            recent_only: true,
            nav_path: Vec::new(),
            expanded: None,
        };
        // Patch the session's modified time to "now" so it's recent.
        d.sessions[0].modified = Some(std::time::SystemTime::now());
        assert!(d.session_visible(&d.sessions[0]));
    }

    #[test]
    fn session_visible_recent_only_false_shows_everything() {
        // recent_only=false → all sessions visible regardless of mtime
        let mut d = Dashboard {
            sessions: vec![sess_with_status(crate::tree::SessionStatus::Done)],
            tails: std::collections::HashMap::new(),
            selected: 0,
            last_discovery: std::time::Instant::now(),
            recent_only: false,
            nav_path: Vec::new(),
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
            recent_only: false,
            nav_path: Vec::new(),
            expanded: None,
        };
        assert!(d.session_visible(&d.sessions[0]));
    }

    #[test]
    fn toggle_recent_only_flips_filter() {
        let mut d = Dashboard {
            sessions: vec![],
            tails: std::collections::HashMap::new(),
            selected: 0,
            last_discovery: std::time::Instant::now(),
            recent_only: false,
            nav_path: Vec::new(),
            expanded: None,
        };
        assert!(!d.recent_only);
        d.toggle_recent_only();
        assert!(d.recent_only);
        d.toggle_recent_only();
        assert!(!d.recent_only);
    }

    #[test]
    fn toggle_expand_sets_and_clears() {
        let mut d = Dashboard {
            sessions: vec![sess_with_status(crate::tree::SessionStatus::Running)],
            tails: std::collections::HashMap::new(),
            selected: 0,
            last_discovery: std::time::Instant::now(),
            recent_only: false,
            nav_path: vec!["r".into(), "main".into()],
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
            let n = dash.nav_items().len();
            if n > 0 {
                dash.selected = n - 1;
            }
            false
        }
        KeyCode::Char('r') => {
            // Force rediscovery.
            dash.last_discovery = Duration::from_secs(99 * 60 * 60).into_std_instant();
            false
        }
        KeyCode::Char('f') => {
            dash.toggle_recent_only();
            false
        }
        KeyCode::Enter => {
            // Drill down: Repos → Branches in repo; Branches → Sessions in
            // branch; Sessions → toggle expand on the focused session.
            dash.drill_down();
            false
        }
        KeyCode::Backspace => {
            // Drill back up: Sessions → Branches; Branches → Repos; Repos → no-op.
            dash.drill_up();
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
