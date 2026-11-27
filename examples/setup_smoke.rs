//! Smoke test for the interactive setup and schedule commands.
use termux_sync::core::errors::CoreError;
use termux_sync::core::maintenance;

fn main() -> Result<(), CoreError> {
    match std::env::args().nth(1).as_deref() {
        Some("setup") => maintenance::setup().map(|_| ()),
        Some("schedule") => maintenance::schedule().map(|_| ()),
        _ => Err(CoreError::plain("usage: setup_smoke setup|schedule")),
    }
}
