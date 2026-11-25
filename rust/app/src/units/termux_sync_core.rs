use std::process::ExitCode;

use clap::ArgMatches;

pub fn run(_command: &str, _matches: &ArgMatches) -> ExitCode {
    crate::frontdoor::run_from_argv()
}
