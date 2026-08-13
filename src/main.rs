//! agent-graph-tui: terminal viewer for agent execution graphs.
//!
//! Two modes:
//!   `agent-graph-tui [PATH]` — single-session viewer for one jsonl file.
//!   `agent-graph-tui` (no arg) — multi-session dashboard; auto-discovers
//!     active Claude Code / Codex / MiniMax / Gemini sessions.

use std::io::{IsTerminal, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use agent_graph_tui::{app, dashboard, discovery, parser, render, tree};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

fn main() -> ExitCode {
    let started = Instant::now();
    let arg = std::env::args().nth(1);

    // Dashboard mode: no path.
    if arg.is_none() {
        return run_dashboard(started);
    }

    // Single-session mode.
    let path = match arg {
        Some(p) => PathBuf::from(p),
        None => {
            eprintln!("agent-graph-tui: missing path");
            return ExitCode::from(1);
        }
    };
    run_single(&path, started)
}

fn run_single(path: &PathBuf, started: Instant) -> ExitCode {
    let session = match parser::Session::open(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("agent-graph-tui: failed to open {}: {e}", path.display());
            return ExitCode::from(1);
        }
    };

    let cold_start_ms = started.elapsed().as_micros() as f64 / 1000.0;

    if !std::io::stdout().is_terminal() {
        let snapshot = tree::Session::build(&session);
        let stdout = std::io::stdout();
        let mut lock = stdout.lock();
        let _ = render::write_text(&snapshot, &mut lock);
        let _ = writeln!(lock, "cold start: {cold_start_ms:.2} ms (single mode)");
        return ExitCode::SUCCESS;
    }

    let backend = CrosstermBackend::new(std::io::stdout());
    let mut terminal = match Terminal::new(backend) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("agent-graph-tui: terminal init failed: {e}");
            return ExitCode::from(1);
        }
    };

    let res = app::run(&mut terminal, session);

    let _ = crossterm::execute!(
        std::io::stdout(),
        crossterm::terminal::LeaveAlternateScreen,
        crossterm::event::DisableMouseCapture
    );
    let _ = crossterm::terminal::disable_raw_mode();

    if let Err(e) = res {
        eprintln!("agent-graph-tui: {e}");
        return ExitCode::from(1);
    }
    eprintln!("cold start: {cold_start_ms:.2} ms");
    ExitCode::SUCCESS
}

fn run_dashboard(started: Instant) -> ExitCode {
    let report = discovery::discover();
    if report.sessions.is_empty() {
        eprintln!(
            "agent-graph-tui: no active sessions found under ~/.claude, ~/.codex, ~/.minimax, ~/.gemini"
        );
        return ExitCode::from(1);
    }
    let dash = dashboard::Dashboard::from_report(report);

    let cold_start_ms = started.elapsed().as_micros() as f64 / 1000.0;

    if !std::io::stdout().is_terminal() {
        // Plain-text: print each session header + first events.
        let stdout = std::io::stdout();
        let mut lock = stdout.lock();
        // Re-render with the same hierarchy as the TUI: repo → branch.
        // Apply the same `show_done` filter so text-mode and TUI-mode
        // present the same set of sessions.
        let mut sorted: Vec<usize> = dash
            .sessions
            .iter()
            .enumerate()
            .filter(|(_, s)| dash.session_visible(s))
            .map(|(i, _)| i)
            .collect();

        let total = dash.sessions.len();
        let _ = writeln!(
            lock,
            "agent-graph-tui — {} of {} sessions visible (text mode, cold start {:.2} ms)",
            sorted.len(),
            total,
            cold_start_ms
        );
        if sorted.is_empty() {
            let _ = writeln!(
                lock,
                "\n(no active sessions — all {} are Done. The TUI `f` key toggles show-done.)",
                total
            );
            return ExitCode::SUCCESS;
        }
        sorted.sort_by(|&a, &b| {
            let sa = &dash.sessions[a];
            let sb = &dash.sessions[b];
            (
                sa.repo_name.clone().unwrap_or_default(),
                sa.branch.clone().unwrap_or_default(),
            )
                .cmp(&(
                    sb.repo_name.clone().unwrap_or_default(),
                    sb.branch.clone().unwrap_or_default(),
                ))
        });
        let mut last_repo: Option<String> = None;
        let mut last_branch: Option<String> = None;
        for &idx in &sorted {
            let s = &dash.sessions[idx];
            let repo_disp = s
                .repo_name
                .clone()
                .unwrap_or_else(|| "<unknown>".to_string());
            if last_repo.as_deref() != Some(repo_disp.as_str()) {
                let _ = writeln!(lock, "\n  {repo_disp}");
                last_repo = Some(repo_disp);
                last_branch = None;
            }
            let branch_disp = s
                .branch
                .clone()
                .unwrap_or_else(|| "<no branch>".to_string());
            if last_branch.as_deref() != Some(branch_disp.as_str()) {
                let _ = writeln!(lock, "    ⏵ {branch_disp}");
                last_branch = Some(branch_disp);
            }
            let status = dash
                .tails
                .get(&s.path)
                .map(|t| t.status)
                .unwrap_or(s.quick_status);
            let model = s.model.clone().unwrap_or_else(|| "<unknown>".into());
            let _ = writeln!(
                lock,
                "      [{}] {model:<22} {:>3} nodes",
                status.glyph(),
                s.node_count_proxy
            );
            if let Some(t) = &s.task {
                let _ = writeln!(lock, "        task: {t}");
            }
        }
        return ExitCode::SUCCESS;
    }

    let backend = CrosstermBackend::new(std::io::stdout());
    let mut terminal = match Terminal::new(backend) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("agent-graph-tui: terminal init failed: {e}");
            return ExitCode::from(1);
        }
    };
    let res = dashboard::run(&mut terminal, dash);
    let _ = crossterm::execute!(
        std::io::stdout(),
        crossterm::terminal::LeaveAlternateScreen,
        crossterm::event::DisableMouseCapture
    );
    let _ = crossterm::terminal::disable_raw_mode();
    if let Err(e) = res {
        eprintln!("agent-graph-tui: {e}");
        return ExitCode::from(1);
    }
    eprintln!("cold start: {cold_start_ms:.2} ms");
    ExitCode::SUCCESS
}
