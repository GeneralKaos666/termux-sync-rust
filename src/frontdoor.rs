//! Front-door CLI: parse argv, call the ported unit, report errors.

use std::ffi::{OsStr, OsString};
use std::process::ExitCode;

use crate::core::errors::{self, CoreError};
use crate::port;

/// Parsed command-line options shared by all subcommands.
#[derive(Debug)]
pub struct Options {
    /// Emit machine-readable JSON events instead of prose.
    pub json_events: bool,
    /// Skip interactive confirmation where the command supports it.
    pub yes: bool,
    /// Enable verbose debug logging.
    pub debug: bool,
    /// Line count for `logs`; defaults to 40.
    pub lines: i64,
    /// Backup label from `--label`.
    pub label: String,
    /// Backup name for restore or delete.
    pub name: String,
    /// Positional target: a backup name or a path.
    pub target: String,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            json_events: false,
            yes: false,
            debug: false,
            lines: 40,
            label: String::new(),
            name: String::new(),
            target: String::new(),
        }
    }
}

const COMMANDS: &[&str] = &[
    "backup",
    "restore",
    "list",
    "delete",
    "setup",
    "schedule",
    "daemon",
    "export-config",
    "import-config",
    "status",
    "logs",
    "clear-logs",
    "check",
    "clear-cache",
    "help",
];

fn parse_int(raw: &str) -> Option<i64> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    trimmed.replace('_', "").parse::<i64>().ok()
}

fn usage(message: &str) -> ExitCode {
    port::print_error(
        "Invalid usage",
        message,
        "Run termux-sync help to see the correct usage.",
        "",
    );
    ExitCode::from(2)
}

fn unknown(command: &str) -> ExitCode {
    let hints = match command {
        "update" | "upgrade" => "To update: git pull --ff-only && bash install.sh",
        "install" => "To install or reinstall: bash install.sh",
        "remove" => {
            "To delete a backup use termux-sync delete. To uninstall, delete the two binaries from $PREFIX/bin."
        }
        "config" => "Use termux-sync setup to change settings, or termux-sync status to view them.",
        "sync" => "Use termux-sync backup to upload, or termux-sync restore to download.",
        _ => "Run termux-sync help to see all commands.",
    };
    let message = format!("'{command}' is not a termux-sync command.");
    let hint = format!("{hints}\nRun termux-sync help to see all commands.");
    port::print_error("Unknown command", &message, &hint, "");
    ExitCode::from(2)
}

fn report_error(error: &CoreError) -> ExitCode {
    port::log("ERROR", &error.to_string());
    let (title, message, hint) = errors::explain(error);
    port::print_error(&title, &message, &hint, "");
    ExitCode::from(1)
}

#[derive(Debug)]
struct Parsed {
    opts: Options,
    help: bool,
    version: bool,
    positionals: Vec<OsString>,
}

fn parse_args(args: &[OsString]) -> Result<Parsed, String> {
    let mut opts = Options::default();
    let mut help = false;
    let mut version = false;
    let mut positional_only = false;
    let mut positionals: Vec<OsString> = Vec::new();
    let mut index = 0usize;
    while index < args.len() {
        let arg = args[index].to_string_lossy().into_owned();
        index += 1;
        if positional_only {
            positionals.push(args[index - 1].clone());
            continue;
        }
        match arg.as_str() {
            "--" => positional_only = true,
            "-h" | "--help" => help = true,
            "-v" | "--version" => version = true,
            "--json-events" => opts.json_events = true,
            "--yes" => opts.yes = true,
            "--debug" => opts.debug = true,
            "--lines" => {
                let Some(value) = args.get(index) else {
                    return Err("--lines: expected one argument".to_string());
                };
                index += 1;
                let Some(lines) = parse_int(&value.to_string_lossy()) else {
                    return Err(format!(
                        "--lines: invalid int value: '{}'",
                        value.to_string_lossy()
                    ));
                };
                opts.lines = lines;
            }
            value if value.starts_with("--lines=") => {
                let Some(lines) = parse_int(&value["--lines=".len()..]) else {
                    return Err(format!(
                        "--lines: invalid int value: '{}'",
                        &value["--lines=".len()..]
                    ));
                };
                opts.lines = lines;
            }
            "--label" | "-l" => {
                let Some(value) = args.get(index) else {
                    return Err("--label: expected one argument".to_string());
                };
                index += 1;
                opts.label = value.to_string_lossy().into_owned();
            }
            value if value.starts_with("--label=") => {
                opts.label = value["--label=".len()..].to_string();
            }
            value if value.starts_with("-l") && value.len() > 2 => {
                opts.label = value[2..].to_string();
            }
            "--name" | "-n" => {
                let Some(value) = args.get(index) else {
                    return Err("--name: expected one argument".to_string());
                };
                index += 1;
                opts.name = value.to_string_lossy().into_owned();
            }
            value if value.starts_with("--name=") => {
                opts.name = value["--name=".len()..].to_string();
            }
            value if value.starts_with("-n") && value.len() > 2 => {
                opts.name = value[2..].to_string();
            }
            value if value.starts_with('-') && value != "-" => {
                return Err(format!("unrecognized arguments: {value}"));
            }
            _ => positionals.push(args[index - 1].clone()),
        }
    }
    Ok(Parsed {
        opts,
        help,
        version,
        positionals,
    })
}

/// Parses process arguments, dispatches the command, and returns its exit code.
///
/// `tui` is forwarded to [`crate::tui::main_entry`]; usage errors map to 2 and runtime errors to 1.
pub fn run_from_argv() -> ExitCode {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    if args.first().is_some_and(|arg| arg == OsStr::new("tui")) {
        let rest: Vec<String> = args[1..]
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        return match crate::tui::main_entry("termux-sync tui", &rest) {
            Ok(code) => code,
            Err(error) => {
                eprintln!("termux-sync tui: {error}");
                ExitCode::from(1)
            }
        };
    }
    let Parsed {
        mut opts,
        help,
        version,
        positionals,
    } = match parse_args(&args) {
        Ok(parsed) => parsed,
        Err(message) => return usage(&message),
    };

    if opts.debug {
        port::set_debug(true);
    }
    if version {
        println!("termux-sync v{}", crate::core::paths::VERSION);
        return ExitCode::SUCCESS;
    }
    if help || positionals.is_empty() || positionals[0] == OsStr::new("help") {
        port::print_help();
        return ExitCode::SUCCESS;
    }
    if positionals.len() > 2 {
        let extra: Vec<String> = positionals[2..]
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        return usage(&format!("unrecognized arguments: {}", extra.join(" ")));
    }
    let command = match positionals[0].to_str() {
        Some(command) => command.to_string(),
        None => return usage("command must be valid UTF-8"),
    };
    if !COMMANDS.contains(&command.as_str()) {
        return unknown(&command);
    }
    if positionals.len() > 1 {
        match positionals[1].to_str() {
            Some(target) => opts.target = target.to_string(),
            None => return usage("target must be valid UTF-8"),
        }
    }

    let needs_config = matches!(
        command.as_str(),
        "backup" | "restore" | "list" | "delete" | "status" | "daemon"
    );
    let redaction_safe = !matches!(
        command.as_str(),
        "setup" | "import-config" | "export-config" | "logs"
    );
    // Load once: `set_secrets` needs it for redaction even when the command
    // itself can tolerate a broken config.
    let loaded = port::load_config();
    if let Ok(cfg) = &loaded {
        port::set_secrets(cfg);
    }
    let cfg = if needs_config || (opts.json_events && redaction_safe) {
        match loaded {
            Ok(cfg) => Some(cfg),
            Err(error) => return report_error(&error),
        }
    } else {
        None
    };
    if opts.json_events {
        port::enable_event_mode(cfg.clone().unwrap_or_default());
    }
    let cfg = cfg.unwrap_or_default();

    let result = match command.as_str() {
        "backup" => port::cmd_backup(cfg, opts.label),
        "restore" => port::cmd_restore(cfg, opts.name),
        "list" => port::cmd_list(cfg),
        "delete" => port::cmd_delete(cfg, opts.name, opts.yes),
        "setup" => port::cmd_setup(),
        "schedule" => port::cmd_schedule(),
        "daemon" => port::cmd_daemon(cfg),
        "export-config" => port::cmd_export_config(opts.target),
        "import-config" => port::cmd_import_config(opts.target),
        "status" => port::cmd_status(cfg),
        "logs" => port::cmd_logs(opts.lines),
        "clear-logs" => port::cmd_clear_logs(opts.yes),
        "check" => port::cmd_check(opts.target),
        "clear-cache" => port::cmd_clear_cache(opts.yes),
        _ => unreachable!("COMMANDS and dispatch are out of sync: {command}"),
    };
    match result {
        Ok(_) => ExitCode::SUCCESS,
        Err(error) => report_error(&error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    #[test]
    fn parse_int_accepts_the_full_i64_range() {
        assert_eq!(parse_int("-9223372036854775808"), Some(i64::MIN));
        assert_eq!(parse_int("+5"), Some(5));
    }

    #[test]
    fn parse_int_accepts_underscores_like_python() {
        assert_eq!(parse_int("1_000"), Some(1000));
    }

    #[test]
    fn parse_int_rejects_garbage() {
        assert_eq!(parse_int("x"), None);
        assert_eq!(parse_int(""), None);
        assert_eq!(parse_int("--1"), None);
    }

    #[test]
    fn parse_args_reads_attached_and_separate_values() {
        let parsed = parse_args(&args(&[
            "--json-events",
            "--lines=7",
            "--label",
            "nightly",
            "-nbackup",
            "list",
        ]))
        .expect("valid argv");
        assert!(parsed.opts.json_events);
        assert_eq!(parsed.opts.lines, 7);
        assert_eq!(parsed.opts.label, "nightly");
        assert_eq!(parsed.opts.name, "backup");
        assert_eq!(parsed.positionals, args(&["list"]));
    }

    #[test]
    fn parse_args_stops_flag_parsing_after_double_dash() {
        let parsed = parse_args(&args(&["delete", "--", "--yes"])).expect("valid argv");
        assert!(!parsed.opts.yes);
        assert_eq!(parsed.positionals, args(&["delete", "--yes"]));
    }

    #[test]
    fn parse_args_reports_the_legacy_lines_message() {
        let error = parse_args(&args(&["logs", "--lines", "x"])).expect_err("bad int");
        assert_eq!(error, "--lines: invalid int value: 'x'");
    }
}
