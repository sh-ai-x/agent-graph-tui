//! Rendering: TUI frame + plain text fallback. Hand-rolled unicode; no graph-layout crate.

use std::io::Write;

use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
use ratatui::Frame;

use crate::tree::{Node, NodeKind, Session, Status};

pub fn draw(f: &mut Frame, s: &Session, selected: usize, scroll: usize, path: &str) {
    let area = f.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(
            [
                Constraint::Length(3),
                Constraint::Min(1),
                Constraint::Length(1),
            ],
        )
        .split(area);

    let header = Paragraph::new(Line::from(vec![
        Span::styled(
            "agent-graph-tui ",
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::styled(path, Style::default().fg(Color::DarkGray)),
    ]))
    .block(Block::default().borders(Borders::BOTTOM));
    f.render_widget(header, chunks[0]);

    let items: Vec<ListItem> = s
        .rows
        .iter()
        .skip(scroll)
        .map(|n| ListItem::new(render_row(n)))
        .collect();
    let mut state = ListState::default();
    let visible = selected.saturating_sub(scroll);
    if visible < items.len() {
        state.select(Some(visible));
    }
    let list = List::new(items)
        .block(Block::default().borders(Borders::NONE))
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▶ ");
    f.render_stateful_widget(list, chunks[1], &mut state);

    let total = s.rows.len();
    let shown = total.saturating_sub(scroll);
    let footer = Paragraph::new(format!(
        " {shown}/{total}   ↑↓ navigate   q quit "
    ));
    f.render_widget(footer, chunks[2]);
}

fn render_row(n: &Node) -> Line<'static> {
    let prefix = if n.depth == 0 { "" } else { "    └─ " };
    let prefix_owned: String = prefix.to_string();
    let ts_owned = if n.ts.is_empty() {
        String::new()
    } else {
        format!("{} ", short_ts(&n.ts))
    };
    let body_owned: String = match &n.kind {
        NodeKind::UserText(t)
        | NodeKind::AssistantText(t)
        | NodeKind::ToolResult(t)
        | NodeKind::Unknown(t) => truncate(t, 80),
        NodeKind::ToolCall { input, .. } => truncate(input, 80),
    };
    let kind_span = match &n.kind {
        NodeKind::UserText(_) => Span::styled("user      ", Style::default().fg(Color::Cyan)),
        NodeKind::AssistantText(_) => {
            Span::styled("assistant ", Style::default().fg(Color::Magenta))
        }
        NodeKind::ToolCall { name, .. } => Span::styled(
            format!("tool      {name} "),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        NodeKind::ToolResult(_) => Span::styled("result    ", Style::default().fg(Color::Blue)),
        NodeKind::Unknown(_) => Span::styled("?         ", Style::default().fg(Color::Red)),
    };
    let status = status_spans(n.status);
    let mut spans = vec![
        Span::styled(ts_owned, Style::default().fg(Color::DarkGray)),
        Span::raw(prefix_owned),
        kind_span,
        Span::raw(body_owned),
    ];
    spans.extend(status);
    Line::from(spans)
}

fn short_ts(ts: &str) -> &str {
    // ISO-8601 → hh:mm:ss
    if ts.len() >= 19 {
        &ts[11..19]
    } else {
        ts
    }
}

fn status_spans(s: Status) -> Vec<Span<'static>> {
    match s {
        Status::Pending => vec![Span::styled(" …", Style::default().fg(Color::Yellow))],
        Status::Done => vec![Span::styled(" ✓", Style::default().fg(Color::Green))],
        Status::Failed => vec![Span::styled(" ✗", Style::default().fg(Color::Red))],
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

/// Plain-text fallback for non-TTY stdout.
pub fn write_text<W: Write>(s: &Session, w: &mut W) -> std::io::Result<()> {
    for n in &s.rows {
        let _ = writeln!(w, "{}", render_row(n));
    }
    let _ = writeln!(w, "\n{} rows", s.rows.len());
    Ok(())
}
