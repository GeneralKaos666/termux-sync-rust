mod frontdoor;

use std::process::ExitCode;

fn main() -> ExitCode {
    frontdoor::run_from_argv()
}
