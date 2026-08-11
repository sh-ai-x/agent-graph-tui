//! agent-graph-tui: terminal viewer for agent execution graphs.
//!
//! Reads a Claude Code / Codex session JSONL file and renders the
//! user / assistant / tool-call / tool-result tree live as new lines arrive.

use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use agent_graph_tui::{app, parser, render, tree};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

fn main() -> ExitCode {
    let started = Instant::now();
    let path = match resolve_path(std::env::args().nth(1)) {
        Some(p) => p,
        None => {
            eprintln!("agent-graph-tui: no session jsonl found under ~/.claude/projects/");
            return ExitCode::from(1);
        }
    };

    let session = match parser::Session::open(&path) {
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
        let _ = writeln!(lock, "cold start: {cold_start_ms:.2} ms (text mode)");
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

fn resolve_path(arg: Option<String>) -> Option<PathBuf> {
    if let Some(p) = arg {
        return Some(PathBuf::from(p));
    }
    let home = std::env::var_os("HOME")?;
    let base = PathBuf::from(home).join(".claude").join("projects");
    let mut newest: Option<(std::time::SystemTime, PathBuf)> = None;
    let Ok(rd) = std::fs::read_dir(&base) else {
        return None;
    };
    for proj in rd.flatten() {
        let Ok(pd) = std::fs::read_dir(proj.path()) else { continue };
        for entry in pd.flatten() {
            let p = entry.path();
            if p.extension().and_then(|s| s.to_str()) != Some("jsonl") {
                continue;
            }
            let Ok(meta) = entry.metadata() else { continue };
            let Ok(modified) = meta.modified() else { continue };
            if newest.as_ref().map_or(true, |(t, _)| modified > *t) {
                newest = Some((modified, p));
            }
        }
    }
    newest.map(|(_, p)| p)
}

#[allow(dead_code)]
fn _ensure_unused(_: &Path) {}
