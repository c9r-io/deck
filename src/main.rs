mod app;
mod model;
mod tmux;
mod ui;

use anyhow::Result;
use app::{Action, App};
use ratatui::crossterm::event::{self, Event, KeyEventKind};
use ratatui::DefaultTerminal;
use std::process::Command;
use std::time::{Duration, Instant};

fn main() -> Result<()> {
    if !tmux::available() {
        eprintln!("deck needs tmux as its session backend.");
        eprintln!("install it with:  brew install tmux");
        std::process::exit(1);
    }

    // idempotent privacy migration before any data file is touched:
    // ~/.deck → 0700, anything an older deck (or the ambient umask) left
    // group/world-readable → 0600. Reported, never silently ignored.
    if let Err(e) = model::harden_data_dir(&model::data_dir()) {
        eprintln!("warning: {e}");
    }

    let mut app = App::load()?;
    let mut terminal = ratatui::init();
    let result = run(&mut terminal, &mut app);
    ratatui::restore();
    app.board.save()?;
    result
}

fn run(terminal: &mut DefaultTerminal, app: &mut App) -> Result<()> {
    let mut last_refresh = Instant::now();
    loop {
        terminal.draw(|f| ui::draw(f, app))?;

        if let Some(action) = app.take_action() {
            perform(terminal, app, action)?;
            continue;
        }
        if app.quit {
            return Ok(());
        }

        if event::poll(Duration::from_millis(200))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    app.on_key(key.code);
                }
            }
        }
        if last_refresh.elapsed() >= Duration::from_millis(1000) {
            app.refresh();
            last_refresh = Instant::now();
        }
    }
}

/// Run an action that needs the terminal for itself (attach, $EDITOR).
fn perform(terminal: &mut DefaultTerminal, app: &mut App, action: Action) -> Result<()> {
    match action {
        Action::Attach { session } => {
            if tmux::inside_tmux() {
                // Already inside tmux: just move this client, deck keeps running.
                if let Err(e) = tmux::switch_client(&session) {
                    app.status = format!("switch failed: {e}");
                }
            } else {
                ratatui::restore();
                let status = Command::new("tmux")
                    .args(["attach-session", "-t", &tmux::session_target(&session)])
                    .status();
                *terminal = ratatui::init();
                terminal.clear()?;
                if let Err(e) = status {
                    app.status = format!("attach failed: {e}");
                }
            }
        }
        Action::EditNotes { card_id } => {
            // create dir + file ourselves so notes are 0600 from the start:
            // an editor creating the file would use the ambient umask
            let path = match model::prepare_notes(&model::data_dir(), &card_id) {
                Ok(p) => p,
                Err(e) => {
                    app.status = format!("notes unavailable: {e}");
                    return Ok(());
                }
            };
            let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".into());
            ratatui::restore();
            let status = Command::new(&editor).arg(&path).status();
            *terminal = ratatui::init();
            terminal.clear()?;
            match status {
                Ok(_) => app.status = format!("notes saved: {}", path.display()),
                Err(e) => app.status = format!("editor '{editor}' failed: {e}"),
            }
        }
    }
    app.refresh();
    Ok(())
}
