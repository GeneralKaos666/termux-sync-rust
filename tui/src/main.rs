use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match termux_sync_tui::main_entry("termux-sync-tui", &args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("termux-sync-tui: {error}");
            ExitCode::from(1)
        }
    }
}
