//! Command-line entry point for `termux-sync`.
//!
//! Argument parsing and command dispatch live in `frontdoor`; the binary only
//! maps the returned `ExitCode` to the process status.

use std::process::ExitCode;

fn main() -> ExitCode {
    termux_sync::frontdoor::run_from_argv()
}
