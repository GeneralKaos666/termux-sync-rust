//! Front-door CLI: parse argv, call the ported unit, report errors.

use std::ffi::OsString;
use std::process::ExitCode;

use termux_sync_core::port;

pub struct Options {
    pub json_events: bool,
    pub yes: bool,
    pub debug: bool,
    pub lines: i64,
    pub label: String,
    pub name: String,
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
    "tui",
    "help",
];

fn parse_int(raw: &str) -> Option<i64> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let sign = if trimmed.starts_with('-') { -1 } else { 1 };
    let digits = trimmed.trim_start_matches(['-', '+']).replace('_', "");
    if digits.is_empty() || !digits.chars().all(|ch| ch.is_ascii_digit()) {
        return None;
    }
    digits.parse::<i64>().ok().map(|value| value * sign)
}

fn usage(message: &str) -> ExitCode {
    let _ = port::print_error(
        "Invalid usage".to_string(),
        message
            .replace("argument --lines/", "--lines")
            .replace("argument ", ""),
        "Run termux-sync help to see the correct usage.".to_string(),
        String::new(),
    );
    ExitCode::from(2)
}

fn unknown(command: &str) -> ExitCode {
    let hints = match command {
        "update" => "termux-sync has no built-in update command. Use tsctl updater to update.",
        "upgrade" => "To update, use tsctl updater.",
        "install" => "To install or reinstall, use the install script, or tsctl.",
        "remove" => {
            "To delete a backup use termux-sync delete. To uninstall the tool use tsctl uninstall."
        }
        "config" => "Use termux-sync setup to change settings, or termux-sync status to view them.",
        "sync" => "Use termux-sync backup to upload, or termux-sync restore to download.",
        _ => "Run termux-sync help to see all commands.",
    };
    let _ = port::print_error(
        "Unknown command".to_string(),
        format!("'{command}' is not a termux-sync command."),
        format!("{hints}\nRun termux-sync help to see all commands."),
        String::new(),
    );
    ExitCode::from(2)
}

fn report_error(error: &str) -> ExitCode {
    let (kind, message) = error.split_once(": ").unwrap_or(("", error));
    let (title, hint) = if kind.ends_with("UserError") {
        ("Cannot continue".to_string(), String::new())
    } else {
        (
            "Operation failed".to_string(),
            "See the message above. Details are saved in the log.".to_string(),
        )
    };
    termux_sync_core::core::logging::log("ERROR", error);
    let _ = port::print_error(title, message.to_string(), hint, String::new());
    ExitCode::from(1)
}

pub fn run_from_argv() -> ExitCode {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    if raw.first().map(String::as_str) == Some("tui") {
        return match termux_sync_tui::main_entry("termux-sync tui", &raw[1..]) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("termux-sync tui: {error}");
                ExitCode::from(1)
            }
        };
    }
    let mut opts = Options::default();
    let mut help = false;
    let mut version = false;
    let mut positional_only = false;
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let mut positionals: Vec<String> = Vec::new();
    let mut index = 0usize;
    while index < args.len() {
        let arg = args[index].to_string_lossy().into_owned();
        index += 1;
        if positional_only {
            positionals.push(arg);
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
                    return usage("argument --lines: expected one argument");
                };
                index += 1;
                let Some(lines) = parse_int(&value.to_string_lossy()) else {
                    return usage(&format!(
                        "argument --lines: invalid int value: '{}'",
                        value.to_string_lossy()
                    ));
                };
                opts.lines = lines;
            }
            value if value.starts_with("--lines=") => {
                let Some(lines) = parse_int(&value["--lines=".len()..]) else {
                    return usage(&format!(
                        "argument --lines: invalid int value: '{}'",
                        &value["--lines=".len()..]
                    ));
                };
                opts.lines = lines;
            }
            "--label" | "-l" => {
                let Some(value) = args.get(index) else {
                    return usage("argument --label: expected one argument");
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
                    return usage("argument --name: expected one argument");
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
                return usage(&format!("unrecognized arguments: {value}"));
            }
            _ => positionals.push(arg),
        }
    }

    if version {
        println!("termux-sync v{}", termux_sync_core::core::paths::VERSION);
        return ExitCode::SUCCESS;
    }
    if help || positionals.is_empty() || positionals[0] == "help" {
        let _ = port::print_help();
        return ExitCode::SUCCESS;
    }
    if positionals.len() > 2 {
        return usage(&format!(
            "unrecognized arguments: {}",
            positionals[2..].join(" ")
        ));
    }
    let command = positionals[0].clone();
    if !COMMANDS.contains(&command.as_str()) {
        return unknown(&command);
    }
    if positionals.len() > 1 {
        opts.target = positionals[1].clone();
    }

    let cfg = match port::load_config() {
        Ok(cfg) => cfg,
        Err(error) => return report_error(&error),
    };
    if opts.json_events {
        let _ = port::enable_event_mode(cfg.clone());
    }

    let result = match command.as_str() {
        "backup" => port::cmd_backup(cfg, opts.label.clone()),
        "restore" => port::cmd_restore(cfg, opts.name.clone()),
        "list" => port::cmd_list(cfg),
        "delete" => port::cmd_delete(cfg, opts.name.clone(), opts.yes),
        "setup" => port::cmd_setup(),
        "schedule" => port::cmd_schedule(),
        "daemon" => port::cmd_daemon(cfg),
        "export-config" => port::cmd_export_config(opts.target.clone()),
        "import-config" => port::cmd_import_config(opts.target.clone()),
        "status" => port::cmd_status(cfg),
        "logs" => port::cmd_logs(opts.lines),
        "clear-logs" => port::cmd_clear_logs(opts.yes),
        "check" => port::cmd_check(opts.target.clone()),
        "clear-cache" => port::cmd_clear_cache(opts.yes),
        "tui" => Ok(serde_json::Value::Null),
        "help" => port::print_help(),
        _ => Ok(serde_json::Value::Null),
    };
    match result {
        Ok(_) => ExitCode::SUCCESS,
        Err(error) => report_error(&error),
    }
}
