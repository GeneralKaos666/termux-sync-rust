//! Written by `rust-anything app lift`: every command is answered in Rust.

use std::process::ExitCode;

use clap::ArgMatches;

mod termux_sync_core;

pub fn dispatch(command: &str, matches: &ArgMatches) -> Option<ExitCode> {
    match command {
        "backup" | "restore" | "list" | "delete" | "setup" | "schedule" | "daemon"
        | "export-config" | "import-config" | "status" | "logs" | "clear-logs" | "check"
        | "clear-cache" | "help" => Some(termux_sync_core::run(command, matches)),
        _ => None,
    }
}
