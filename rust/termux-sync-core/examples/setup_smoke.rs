use termux_sync_core::core::maintenance;

fn main() -> Result<(), String> {
    match std::env::args().nth(1).as_deref() {
        Some("setup") => maintenance::setup().map(|_| ()),
        Some("schedule") => maintenance::schedule().map(|_| ()),
        _ => Err("usage: setup_smoke setup|schedule".to_string()),
    }
}
