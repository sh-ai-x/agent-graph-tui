//! ratatui app loop: re-scans the jsonl tail on a 100 ms budget, redraws at ~30 fps.

use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind};
use ratatui::Terminal;
use ratatui::backend::Backend;

use crate::parser::Session as ParseSession;
use crate::render;
use crate::tree;

/// Vertical slices of the frame: 3-line header, body, 1-line footer.
const HEADER_HEIGHT: u16 = 3;
const FOOTER_HEIGHT: u16 = 1;

pub fn run<B: Backend>(term: &mut Terminal<B>, mut session: ParseSession) -> std::io::Result<()> {
    crossterm::terminal::enable_raw_mode()?;
    crossterm::execute!(
        std::io::stdout(),
        crossterm::terminal::EnterAlternateScreen,
        crossterm::event::EnableMouseCapture
    )?;

    let mut offset: u64 = {
        let f = std::fs::File::open(&session.path)?;
        f.metadata().map(|m| m.len()).unwrap_or(0)
    };
    let mut last_rescan = Instant::now();
    let mut last_draw = Instant::now();

    let path_str = session.path.display().to_string();
    let mut snap = tree::Session::build(&session);
    let mut selected: usize = 0;
    let mut scroll: usize = 0;

    loop {
        if last_rescan.elapsed() >= Duration::from_millis(100) {
            if let Ok(new_offset) = session.rescan_from(offset) {
                if new_offset != offset {
                    offset = new_offset;
                    snap = tree::Session::build(&session);
                }
            }
            last_rescan = Instant::now();
        }

        // Body viewport height = terminal height minus header & footer.
        let viewport = term
            .size()
            .map(|s| s.height.saturating_sub(HEADER_HEIGHT + FOOTER_HEIGHT) as usize)
            .unwrap_or(20)
            .max(1);

        if last_draw.elapsed() >= Duration::from_millis(33) {
            term.draw(|f| render::draw(f, &snap, selected, scroll, &path_str))?;
            last_draw = Instant::now();
        }

        if event::poll(Duration::from_millis(50))? {
            if let Event::Key(k) = event::read()? {
                if k.kind == KeyEventKind::Press
                    && handle_key(k, &mut selected, &mut scroll, snap.rows.len(), viewport)
                {
                    break;
                }
            }
        }
    }
    Ok(())
}

fn handle_key(
    k: KeyEvent,
    selected: &mut usize,
    scroll: &mut usize,
    total: usize,
    viewport: usize,
) -> bool {
    match k.code {
        KeyCode::Char('q') | KeyCode::Esc => true,
        KeyCode::Char('j') | KeyCode::Down => {
            if total > 0 && *selected + 1 < total {
                *selected += 1;
                if *selected >= *scroll + viewport {
                    *scroll = *selected + 1 - viewport;
                }
            }
            false
        }
        KeyCode::Char('k') | KeyCode::Up => {
            if *selected > 0 {
                *selected -= 1;
                if *selected < *scroll {
                    *scroll = *selected;
                }
            }
            false
        }
        KeyCode::Char('g') => {
            *selected = 0;
            *scroll = 0;
            false
        }
        KeyCode::Char('G') => {
            if total > 0 {
                *selected = total - 1;
                *scroll = total.saturating_sub(viewport);
            }
            false
        }
        _ => false,
    }
}
