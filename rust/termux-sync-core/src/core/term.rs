//! Terminal width and mode.

use std::env;
use std::io::{IsTerminal, stderr, stdout};

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

pub fn stdout_is_terminal() -> bool {
    stdout().is_terminal()
}

pub fn stderr_is_terminal() -> bool {
    stderr().is_terminal()
}
