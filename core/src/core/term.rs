//! Terminal width and mode.

use std::env;
use std::io::{IsTerminal, stderr, stdout};

/// Current terminal width in columns. `$COLUMNS` wins when it parses to a
/// positive number; otherwise the tty size is used, falling back to 80.
#[must_use]
pub fn width() -> usize {
    if let Ok(value) = env::var("COLUMNS")
        && let Ok(parsed) = value.trim().parse::<usize>()
        && parsed > 0
    {
        return parsed;
    }
    terminal_size::terminal_size()
        .map(|(columns, _)| usize::from(columns.0))
        .filter(|width| *width > 0)
        .unwrap_or(80)
}

/// Whether stdout is attached to a terminal, used to decide on colour output.
#[must_use]
pub fn stdout_is_terminal() -> bool {
    stdout().is_terminal()
}

/// Whether stderr is attached to a terminal.
#[must_use]
pub fn stderr_is_terminal() -> bool {
    stderr().is_terminal()
}
