//! Interactive terminal dashboard for `termux-sync`.
//!
//! [`run`] drives the event loop; [`app::App`] holds the state and key
//! handling, [`ops`] owns the child process and event protocol, [`data`] reads
//! local state, and [`ui`] renders frames.

pub mod app;
pub mod data;
pub mod history;
pub mod ops;
pub mod ui;

use std::io::{self, IsTerminal, Write};
use std::process::ExitCode;
use std::time::Duration;

use ratatui::DefaultTerminal;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind, MouseButton, MouseEventKind,
};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{Clear, ClearType, EnterAlternateScreen, enable_raw_mode};

use crate::app::App;

const VERSION: &str = env!("CARGO_PKG_VERSION");
const FRAME_TIME: Duration = Duration::from_millis(50);
const IDLE_TIME: Duration = Duration::from_millis(1_000);

/// Runs the dashboard, returning the exit code the caller should propagate.
///
/// The no-terminal case is reported as [`ExitCode::from(2)`] instead of
/// terminating the process, so callers keep control of process exit.
pub fn main_entry(program: &str, args: &[String]) -> io::Result<ExitCode> {
    if args.iter().any(|arg| arg == "--version" || arg == "-V") {
        println!("{program} {VERSION}");
        return Ok(ExitCode::SUCCESS);
    }
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        print_help(program);
        return Ok(ExitCode::SUCCESS);
    }
    if !io::stdout().is_terminal() {
        eprintln!("{program}: refusing to start: stdout is not a terminal");
        return Ok(ExitCode::from(2));
    }

    install_panic_hook();
    let mut terminal = match init_terminal() {
        Ok(terminal) => terminal,
        Err(err) => {
            restore_terminal();
            return Err(err);
        }
    };
    let result = run(&mut terminal, args);
    restore_terminal();
    result?;
    Ok(ExitCode::SUCCESS)
}

fn run(terminal: &mut DefaultTerminal, args: &[String]) -> io::Result<()> {
    let reduce_motion = std::env::var_os("TERMUX_SYNC_TUI_REDUCED_MOTION")
        .is_some_and(|value| !value.is_empty())
        || args.iter().any(|arg| arg == "--no-anim");
    let mut app = App::new();
    app.reduce_motion = reduce_motion;
    app.ascii = std::env::var_os("TERMUX_SYNC_TUI_ASCII").is_some_and(|value| !value.is_empty())
        || args.iter().any(|arg| arg == "--ascii");
    let mut redraw = true;

    loop {
        if redraw {
            terminal.draw(|frame| ui::draw(frame, &mut app))?;
            redraw = false;
        }

        let timeout = if app.needs_tick() {
            FRAME_TIME
        } else {
            IDLE_TIME
        };
        if event::poll(timeout)? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    app.handle_key_event(key);
                    redraw = true;
                }
                Event::Mouse(mouse) if mouse.kind == MouseEventKind::Down(MouseButton::Left) => {
                    app.handle_mouse(mouse.column, mouse.row);
                    redraw = true;
                }
                Event::Mouse(mouse)
                    if matches!(
                        mouse.kind,
                        MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
                    ) =>
                {
                    app.handle_scroll(
                        mouse.column,
                        mouse.row,
                        mouse.kind == MouseEventKind::ScrollUp,
                    );
                    redraw = true;
                }
                Event::Resize(_, _) => redraw = true,
                _ => {}
            }
        }

        if app.needs_tick() {
            app.on_tick();
            redraw = true;
        }

        if app.poll_worker() {
            redraw = true;
        }

        if app.poll_operation() {
            redraw = true;
        }

        if app.should_quit {
            break;
        }

        if let Some(argv) = app.pending.take() {
            handoff(terminal, &mut app, &argv)?;
            redraw = true;
        }
    }

    Ok(())
}

fn init_terminal() -> io::Result<DefaultTerminal> {
    enable_raw_mode()?;
    execute!(
        io::stdout(),
        EnterAlternateScreen,
        EnableMouseCapture,
        Clear(ClearType::All)
    )?;
    Terminal::new(CrosstermBackend::new(io::stdout()))
}

fn restore_terminal() {
    let _ = execute!(io::stdout(), DisableMouseCapture);
    ratatui::restore();
}

fn install_panic_hook() {
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
        original_hook(info);
    }));
}

fn handoff(terminal: &mut DefaultTerminal, app: &mut App, argv: &[String]) -> io::Result<()> {
    restore_terminal();

    let result = data::cli_command(argv).status();

    print!("\nPress Enter to return to the dashboard…");
    io::stdout().flush()?;
    let mut line = String::new();
    let _ = io::stdin().read_line(&mut line);

    *terminal = init_terminal()?;

    app.status = match result {
        Ok(status) if status.success() => format!("termux-sync {} finished", argv.join(" ")),
        Ok(status) => format!("termux-sync {} exited with {status}", argv.join(" ")),
        Err(err) => format!("cannot run termux-sync: {err}"),
    };
    app.refresh();
    Ok(())
}

fn print_help(program: &str) {
    println!(
        "{program} {VERSION}
Ratatui dashboard wrapper for the termux-sync CLI (spike).

Usage: {program}

Keys:
  ↑ / ↓      move selection
  Enter      run action or restore selected backup
  Click      run an action, select a backup, focus a pane
  d          delete selected backup
  Tab        switch pane
  R          refresh backups and status
  L          list backups, including remote
  s          status
  /          filter backups
  h          operation history
  [ / ]      Output pane: fewer / more log lines
  ?          help
  q          quit
  Ctrl+C     quit dashboard; cancel a running backup; refused during protected restore

Actions:
  backup, restore, delete, list, logs, clear logs, status, disk check,
  export/import config, daemon, setup, schedule, clear cache.
  Setup and schedule open a handoff card.

Options:
  --no-anim  disable motion (also TERMUX_SYNC_TUI_REDUCED_MOTION=1)
  --ascii    use ASCII icons (also TERMUX_SYNC_TUI_ASCII=1)

Environment:
  TERMUX_SYNC_BIN                 termux-sync executable (default: termux-sync)
  TERMUX_SYNC_TUI_REDUCED_MOTION  disable motion when non-empty
  TERMUX_SYNC_TUI_ASCII           use ASCII icons when non-empty"
    );
}
