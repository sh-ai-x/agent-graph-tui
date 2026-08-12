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
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

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
        let _ = writeln!(
            lock,
            "agent-graph-tui — {} sessions (text mode, cold start {:.2} ms)",
            dash.sessions.len(),
            cold_start_ms
        );
        for s in &dash.sessions {
            let label = s
                .repo_name
                .clone()
                .or_else(|| s.worktree_name.clone())
                .unwrap_or_else(|| {
                    let stem = s
                        .path
                        .file_stem()
                        .map(|f| f.to_string_lossy().to_string())
                        .unwrap_or_default();
                    stem.get(..8).unwrap_or(stem.as_str()).to_string()
                });
            let model = s.model.clone().unwrap_or_else(|| "—".into());
            let _ = writeln!(
                lock,
                "\n{} {} {} ⏵ {}",
                s.agent.icon(),
                s.agent.label(),
                label,
                s.branch.clone().unwrap_or_else(|| "—".into())
            );
            let _ = writeln!(lock, "  model: {model}");
            if let Some(t) = &s.task {
                let _ = writeln!(lock, "  task:  {t}");
            }
            if let Some(err) = dash.tails.get(&s.path).and_then(|t| t.last_error.as_ref()) {
                let _ = writeln!(lock, "  ⚠ {err}");
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
