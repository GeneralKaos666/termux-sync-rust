mod cli;
mod frontdoor;
mod units;

use std::ffi::OsString;
use std::process::ExitCode;

fn report_fallbacks() {
    if let Some(path) = std::env::var_os("RUST_ANYTHING_FALLBACK_LOG") {
        let _ = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path);
    }
}

fn main() -> ExitCode {
    report_fallbacks();
    let argv: Vec<OsString> = std::env::args_os().collect();
    if let Ok(matches) = cli::command_tree().try_get_matches_from(&argv) {
        let (command, sub) = matches.subcommand().unwrap_or((cli::ROOT, &matches));
        if let Some(code) = units::dispatch(command, sub) {
            return code;
        }
    }
    frontdoor::run_from_argv()
}
