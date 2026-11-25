use std::ffi::OsString;

use clap::{Arg, ArgAction, Command, value_parser};

/// The command `units::dispatch` gets when argv names no subcommand.
pub const ROOT: &str = "termux-sync";

fn rest() -> Arg {
    Arg::new("args")
        .num_args(0..)
        .trailing_var_arg(true)
        .allow_hyphen_values(true)
        .value_parser(value_parser!(OsString))
}

fn command(name: &'static str) -> Command {
    Command::new(name).disable_help_flag(true).arg(rest())
}

fn flag(name: &'static str, long: &'static str) -> Arg {
    Arg::new(name).long(long).num_args(1)
}

fn switch(name: &'static str, long: &'static str) -> Arg {
    Arg::new(name).long(long).action(ArgAction::Count)
}

/// Parses argv into a command and its arguments.
pub fn command_tree() -> Command {
    Command::new(ROOT)
        .disable_help_flag(true)
        .disable_version_flag(true)
        .disable_help_subcommand(true)
        .args_conflicts_with_subcommands(true)
        .arg(rest())
        .subcommand(
            command("backup")
                .arg(flag("label", "label"))
                .arg(switch("debug", "debug")),
        )
        .subcommand(
            command("restore")
                .arg(flag("name", "name"))
                .arg(switch("debug", "debug")),
        )
        .subcommand(command("list"))
        .subcommand(
            command("delete")
                .arg(flag("name", "name"))
                .arg(switch("yes", "yes"))
                .arg(switch("debug", "debug")),
        )
        .subcommand(command("setup"))
        .subcommand(command("schedule"))
        .subcommand(command("daemon"))
        .subcommand(command("export-config"))
        .subcommand(command("import-config"))
        .subcommand(command("status"))
        .subcommand(
            command("logs")
                .arg(flag("lines", "lines"))
                .arg(switch("debug", "debug")),
        )
        .subcommand(
            command("clear-logs")
                .arg(switch("yes", "yes"))
                .arg(switch("debug", "debug")),
        )
        .subcommand(command("check"))
        .subcommand(
            command("clear-cache")
                .arg(switch("yes", "yes"))
                .arg(switch("debug", "debug")),
        )
        .subcommand(command("help"))
}
