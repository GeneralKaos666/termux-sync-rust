//! Command-line entry point for `termux-sync`.
//!
//! Argument parsing and command dispatch live in [`frontdoor`]; the binary only
//! maps the returned [`std::process::ExitCode`] to the process status.

mod frontdoor;

use std::process::ExitCode;

fn main() -> ExitCode {
    frontdoor::run_from_argv()
}
