//! Standalone entry point for the `termux-sync-tui` dashboard.

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match termux_sync_tui::main_entry("termux-sync-tui", &args) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("termux-sync-tui: {error}");
            ExitCode::from(1)
        }
    }
}
