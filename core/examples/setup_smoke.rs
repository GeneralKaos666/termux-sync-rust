use termux_sync_core::core::errors::CoreError;
use termux_sync_core::core::maintenance;

fn main() -> Result<(), CoreError> {
    match std::env::args().nth(1).as_deref() {
        Some("setup") => maintenance::setup().map(|_| ()),
        Some("schedule") => maintenance::schedule().map(|_| ()),
        _ => Err(CoreError::plain("usage: setup_smoke setup|schedule")),
    }
}
