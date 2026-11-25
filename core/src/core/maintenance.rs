//! Maintenance commands and interactive setup/schedule/daemon.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use chrono::{Local, Timelike};
use serde_json::{Map, Value, json};

use crate::core::backup;
use crate::core::config;
use crate::core::fmt;
use crate::core::fsutil;
use crate::core::json as pyjson;
use crate::core::logging;
use crate::core::manifest;
use crate::core::paths;
use crate::core::render;
use crate::core::storage::Storage;
use crate::core::text;

static DEBUG: AtomicBool = AtomicBool::new(false);

pub fn set_debug(enabled: bool) {
    DEBUG.store(enabled, Ordering::Relaxed);
}

fn debug_enabled() -> bool {
    DEBUG.load(Ordering::Relaxed)
        || env::var("TERMUX_SYNC_DEBUG")
            .map(|value| !value.is_empty())
            .unwrap_or(false)
}

fn cfg_string(cfg: &Value, key: &str, default: &str) -> String {
    cfg.get(key)
        .map(text::py_str)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| default.to_string())
}

fn map_string(map: &Map<String, Value>, key: &str, default: &str) -> String {
    map.get(key)
        .map(text::py_str)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| default.to_string())
}

pub fn delete(cfg: &Value, backup_name: &str, assume_yes: bool) -> Result<Value, String> {
    render::print_section(
        "DELETE BACKUP",
        &format!(
            "Storage: {}",
            cfg_string(cfg, "storage", "local").to_uppercase()
        ),
    );
    let storage = Storage::from_config(cfg)?;
    let backups = storage.list_backups();
    if backups.is_empty() {
        render::print_line("  No backups found.");
        if !backup_name.is_empty() {
            return Err(format!(
                "FileNotFoundError: Backup not found: {backup_name}"
            ));
        }
        return Ok(Value::Null);
    }
    render::print_backup_list(&backups, "Available Backups");

    let mut name = backup_name.to_string();
    if name.is_empty() {
        let choice = render::ask("Enter backup number or name to delete", "")?;
        if let Ok(index) = choice.parse::<usize>() {
            if index == 0 || index > backups.len() {
                render::print_line(&format!("  Invalid number. Enter 1-{}.", backups.len()));
                return Ok(Value::Null);
            }
            name = backups[index - 1]
                .get("name")
                .map(text::py_str)
                .unwrap_or_default();
        } else {
            name = choice;
        }
    }

    let cleaned = manifest::clean_backup_name(&name);
    if cleaned.is_empty() || cleaned != text::py_strip(&name) {
        render::print_line(&format!("  ✗ Invalid backup name: {name:?}"));
        return Ok(Value::Null);
    }
    render::print_line(&format!("  Backup to be deleted: {cleaned}"));
    if !assume_yes && !render::confirm("Are you sure? This cannot be undone.", false) {
        render::print_line("  Cancelled.");
        return Ok(Value::Null);
    }
    match storage.delete_backup(&cleaned) {
        Ok(()) => {
            render::print_line(&format!("  ✓ Backup '{cleaned}' deleted successfully."));
            logging::log("INFO", &format!("Deleted backup: {cleaned}"));
        }
        Err(error) => {
            let shown = [
                "FileNotFoundError: ",
                "UserError: ",
                "RuntimeError: ",
                "ValueError: ",
                "OSError: ",
            ]
            .iter()
            .find_map(|prefix| error.strip_prefix(prefix))
            .unwrap_or(&error);
            render::print_line(&format!("  ✗ Failed to delete backup: {shown}"));
            logging::log("ERROR", &format!("Delete failed for {cleaned}: {shown}"));
            return Err(error);
        }
    }
    Ok(Value::Null)
}

pub fn load_schedule() -> Result<Value, String> {
    let file = paths::cron_file();
    if !file.exists() {
        return Ok(json!({
            "enabled": false,
            "hour": 2,
            "minute": 0,
            "label": "auto",
        }));
    }
    normalize_schedule(pyjson::read_json(&file)?)
}

fn normalize_schedule(value: Value) -> Result<Value, String> {
    let Some(map) = value.as_object() else {
        return Err(
            "termux_sync.UserError: Invalid cron.json: top level must be an object".to_string(),
        );
    };
    let enabled = match map.get("enabled") {
        None => false,
        Some(value) => value
            .as_bool()
            .ok_or("termux_sync.UserError: Invalid cron.json: enabled must be true or false")?,
    };
    let hour = schedule_int(map.get("hour"), "hour", 2, 0, 23)?;
    let minute = schedule_int(map.get("minute"), "minute", 0, 0, 59)?;
    let label = match map.get("label") {
        None => "auto".to_string(),
        Some(Value::String(label)) => label.clone(),
        Some(_) => {
            return Err(
                "termux_sync.UserError: Invalid cron.json: label must be a string".to_string(),
            );
        }
    };
    Ok(json!({
        "enabled": enabled,
        "hour": hour,
        "minute": minute,
        "label": label,
    }))
}

fn schedule_int(
    value: Option<&Value>,
    key: &str,
    default: i64,
    min: i64,
    max: i64,
) -> Result<i64, String> {
    let Some(value) = value else {
        return Ok(default);
    };
    let Some(number) = fmt::py_int(value) else {
        return Err(format!(
            "termux_sync.UserError: Invalid cron.json: {key} must be an integer"
        ));
    };
    if !(min..=max).contains(&number) {
        return Err(format!(
            "termux_sync.UserError: Invalid cron.json: {key} must be between {min} and {max}"
        ));
    }
    Ok(number)
}

pub fn save_schedule(sched: &Value) -> Result<(), String> {
    fs::create_dir_all(paths::config_dir()).map_err(|error| format!("OSError: {error}"))?;
    pyjson::write_json_private(&paths::cron_file(), sched)
}

pub fn setup() -> Result<Value, String> {
    render::print_banner();
    render::print_section("SETUP", "Configure termux-sync storage and preferences");
    let cfg = match config::load_config() {
        Ok(cfg) => cfg,
        Err(error) => {
            render::print_line(&format!("  Warning: {error}"));
            render::print_line("  Starting from the default configuration.");
            Value::Object(config::default_config())
        }
    };
    let mut map = cfg.as_object().cloned().unwrap_or_default();

    render::print_line("  Choose your storage backend:");
    render::print_line("  [1]  Local storage   on device");
    render::print_line("  [2]  Google Drive    via rclone");
    render::print_line("  [3]  GitHub          private repo  (recommended)");
    render::print_line("");
    let choice = loop {
        let value = render::ask("Storage", "3")?;
        if matches!(value.as_str(), "1" | "2" | "3") {
            break value;
        }
        render::print_line("  ✗ Enter 1, 2 or 3.");
    };
    let storage = match choice.as_str() {
        "1" => "local",
        "2" => "gdrive",
        _ => "github",
    };
    map.insert("storage".to_string(), Value::String(storage.to_string()));

    match storage {
        "local" => loop {
            let value = render::ask(
                "Local backup path",
                &map_string(&map, "local_path", &paths::default_dest().to_string_lossy()),
            )?;
            let value = value.trim();
            if !value.is_empty() {
                map.insert(
                    "local_path".to_string(),
                    Value::String(paths::expand_user(value).to_string_lossy().into_owned()),
                );
                break;
            }
            render::print_line("  ✗ Path cannot be empty.");
        },
        "gdrive" => {
            render::print_line("");
            render::print_line(
                "  Make sure rclone is configured with a remote named termux-sync-gdrive.",
            );
            let value = render::ask(
                "Google Drive folder ID (leave blank for root)",
                &map_string(&map, "gdrive_folder_id", ""),
            )?;
            map.insert("gdrive_folder_id".to_string(), Value::String(value));
        }
        "github" => {
            render::print_line("");
            render::print_line(
                "  Create a private repository and a fine-grained token with Contents: Read and write.",
            );
            render::print_line(
                "  Token page: https://github.com/settings/personal-access-tokens/new",
            );
            let existing_token = map_string(&map, "github_token", "");
            if !existing_token.is_empty() {
                render::print_line("  A token is already saved. Press Enter to keep it.");
            }
            loop {
                if !existing_token.is_empty() {
                    let value = render::ask("GitHub token", "")?;
                    if value.trim().is_empty() {
                        break;
                    }
                    map.insert(
                        "github_token".to_string(),
                        Value::String(value.trim().to_string()),
                    );
                    break;
                }
                let value = render::ask("GitHub token", "")?;
                if value.trim().is_empty() {
                    render::print_line("  ✗ Token cannot be empty.");
                    continue;
                }
                map.insert(
                    "github_token".to_string(),
                    Value::String(value.trim().to_string()),
                );
                break;
            }
            loop {
                let value = render::ask(
                    "GitHub repo (owner/repo)",
                    &map_string(&map, "github_repo", ""),
                )?;
                let value = value.trim();
                if value.is_empty()
                    || value.starts_with('/')
                    || value.ends_with('/')
                    || !value.contains('/')
                {
                    render::print_line("  ✗ Invalid format. Use: username/repo-name");
                    continue;
                }
                map.insert("github_repo".to_string(), Value::String(value.to_string()));
                break;
            }
            let value = render::ask("Branch", &map_string(&map, "github_branch", "main"))?;
            map.insert("github_branch".to_string(), Value::String(value));
        }
        _ => {}
    }

    render::print_line("");
    render::print_line("  Compression:  gz = fast · bz2 = balanced · xz = best");
    let compression = loop {
        let value = render::ask("Compression", &map_string(&map, "compression", "gz"))?;
        if matches!(value.as_str(), "gz" | "bz2" | "xz") {
            break value;
        }
        render::print_line("  ✗ Enter gz, bz2 or xz.");
    };
    map.insert("compression".to_string(), Value::String(compression));

    let max_backups = loop {
        let raw = render::ask("Max backups to keep", &map_string(&map, "max_backups", "5"))?;
        if raw.chars().all(|ch| ch.is_ascii_digit())
            && let Ok(value) = raw.parse::<i64>()
            && value >= 1
        {
            break value;
        }
        render::print_line("  ✗ Enter a whole number of 1 or more.");
    };
    map.insert("max_backups".to_string(), Value::from(max_backups));

    let patterns = cfg
        .get("exclude_patterns")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if render::confirm("Customise exclude patterns?", false) {
        render::print_line(&format!(
            "  {}",
            patterns
                .iter()
                .map(text::py_str)
                .collect::<Vec<_>>()
                .join(", ")
        ));
        let raw = render::ask("Patterns (comma separated)", "")?;
        if !raw.trim().is_empty() {
            map.insert(
                "exclude_patterns".to_string(),
                Value::Array(
                    raw.split(',')
                        .map(str::trim)
                        .filter(|part| !part.is_empty())
                        .map(|part| Value::String(part.to_string()))
                        .collect(),
                ),
            );
        }
    }

    let saved = Value::Object(map.clone());
    config::save_config(&saved)?;
    render::print_line(&format!(
        "  ✓ Configuration saved to {}",
        fmt::short_path(&paths::config_file().to_string_lossy())
    ));
    logging::log(
        "INFO",
        &format!(
            "Setup complete. Storage: {}",
            map_string(&map, "storage", "local")
        ),
    );
    Ok(Value::Null)
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn daemon_script_body(exe: &Path) -> String {
    format!(
        "#!/data/data/com.termux/files/usr/bin/sh\n# Auto-generated by termux-sync\n{} daemon &\n",
        shell_quote(&exe.to_string_lossy())
    )
}

fn write_daemon_script(enabled: bool) -> Result<(), String> {
    let boot_dir = paths::home_dir().join(".termux").join("boot");
    let script = boot_dir.join("termux-sync-daemon.sh");
    if !enabled {
        if script.exists() {
            fs::remove_file(&script).map_err(|error| format!("OSError: {error}"))?;
        }
        return Ok(());
    }
    fs::create_dir_all(&boot_dir).map_err(|error| format!("OSError: {error}"))?;
    let executable = env::current_exe().unwrap_or_else(|_| PathBuf::from("termux-sync"));
    fs::write(&script, daemon_script_body(&executable))
        .map_err(|error| format!("OSError: {error}"))?;
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755))
        .map_err(|error| format!("OSError: {error}"))?;
    Ok(())
}

pub fn schedule() -> Result<Value, String> {
    render::print_banner();
    render::print_section("AUTO-BACKUP", "Schedule automatic backups");
    let mut sched = load_schedule()?;
    let enabled = sched
        .get("enabled")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let hour = sched.get("hour").and_then(fmt::py_int).unwrap_or(2);
    let minute = sched.get("minute").and_then(fmt::py_int).unwrap_or(0);
    render::print_line(&format!(
        "  Current schedule: {} @ {:02}:{:02} daily",
        if enabled { "ENABLED" } else { "DISABLED" },
        hour,
        minute
    ));
    let enable = render::confirm("Enable auto-backup?", enabled);
    let label_default = sched
        .get("label")
        .map(text::py_str)
        .unwrap_or_else(|| "auto".to_string());
    let map = sched
        .as_object_mut()
        .ok_or("termux_sync.UserError: Invalid cron.json")?;
    map.insert("enabled".to_string(), Value::Bool(enable));
    if enable {
        let hour = loop {
            let raw = render::ask("Hour (0-23)", &hour.to_string())?;
            match raw.parse::<i64>() {
                Ok(value) if (0..=23).contains(&value) => break value,
                _ => render::print_line("  ✗ Enter a number between 0 and 23."),
            }
        };
        let minute = loop {
            let raw = render::ask("Minute (0-59)", &minute.to_string())?;
            match raw.parse::<i64>() {
                Ok(value) if (0..=59).contains(&value) => break value,
                _ => render::print_line("  ✗ Enter a number between 0 and 59."),
            }
        };
        let label = render::ask("Backup label", &label_default)?;
        map.insert("hour".to_string(), Value::from(hour));
        map.insert("minute".to_string(), Value::from(minute));
        map.insert("label".to_string(), Value::String(label));
    }
    save_schedule(&sched)?;
    write_daemon_script(enable)?;
    if enable {
        render::print_line(&format!(
            "  ✓ Auto-backup scheduled at {:02}:{:02} daily.",
            sched.get("hour").and_then(fmt::py_int).unwrap_or(2),
            sched.get("minute").and_then(fmt::py_int).unwrap_or(0)
        ));
        render::print_line("  Start daemon with: termux-sync daemon &");
        render::print_line("  Or add to Termux:Boot: ~/.termux/boot/termux-sync-daemon.sh");
    } else {
        render::print_line("  Auto-backup disabled.");
    }
    logging::log(
        "INFO",
        &format!(
            "Schedule updated: enabled={enable}, time={:02}:{:02}",
            sched.get("hour").and_then(fmt::py_int).unwrap_or(2),
            sched.get("minute").and_then(fmt::py_int).unwrap_or(0)
        ),
    );
    Ok(Value::Null)
}

pub fn daemon(_cfg: &Value) -> Result<Value, String> {
    let _lock = fsutil::try_lock_exclusive(&paths::config_dir().join("cron.lock"))?;
    let mut sched = match load_schedule() {
        Ok(sched) => sched,
        Err(error) => {
            render::print_line(&format!("  ✗ {error}"));
            return Err(error);
        }
    };
    if !sched
        .get("enabled")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        render::print_line("Auto-backup is not enabled. Run: termux-sync schedule");
        return Ok(Value::Null);
    }
    let hour = sched.get("hour").and_then(fmt::py_int).unwrap_or(2) as u32;
    let minute = sched.get("minute").and_then(fmt::py_int).unwrap_or(0) as u32;
    render::print_line(&format!(
        "termux-sync daemon started. Scheduled at {hour:02}:{minute:02} daily."
    ));
    logging::log("INFO", "Daemon started");
    loop {
        let now = Local::now();
        let mut target = now
            .with_hour(hour)
            .and_then(|value| value.with_minute(minute))
            .and_then(|value| value.with_second(0))
            .and_then(|value| value.with_nanosecond(0))
            .unwrap_or(now);
        if target <= now {
            target += chrono::Duration::days(1);
        }
        let seconds = (target - now).num_seconds().max(0);
        logging::log(
            "INFO",
            &format!(
                "Next backup in {:.1}h at {}",
                seconds as f64 / 3600.0,
                target.format("%H:%M")
            ),
        );
        std::thread::sleep(Duration::from_secs(seconds as u64));
        sched = load_schedule()?;
        if !sched
            .get("enabled")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            logging::log("INFO", "Daemon stopped: auto-backup was disabled");
            render::print_line("Auto-backup was disabled. Daemon stopped.");
            return Ok(Value::Null);
        }
        logging::log("INFO", "Auto-backup triggered by daemon");
        let cfg = config::load_config()?;
        if let Err(error) = backup::backup(
            &cfg,
            &sched
                .get("label")
                .map(text::py_str)
                .unwrap_or_else(|| "auto".to_string()),
        ) {
            logging::log("ERROR", &format!("Daemon backup failed: {error}"));
        }
    }
}

pub fn status(cfg: &Value) -> Result<Value, String> {
    render::print_section("STATUS", "Current configuration and environment");
    let sched = load_schedule().unwrap_or_else(|_| json!({"enabled": false}));
    let storage = cfg_string(cfg, "storage", "local");
    render::print_line(&format!("  Storage backend  {}", storage.to_uppercase()));
    if storage == "local" {
        render::print_line(&format!(
            "  Local path       {}",
            fmt::short_path(&cfg_string(cfg, "local_path", "—"))
        ));
    } else if storage == "gdrive" {
        render::print_line(&format!(
            "  Drive folder     {}",
            cfg_string(cfg, "gdrive_folder_id", "(root)")
        ));
    } else {
        render::print_line(&format!(
            "  GitHub repo      {}",
            cfg_string(cfg, "github_repo", "—")
        ));
        render::print_line(&format!(
            "  Branch           {}",
            cfg_string(cfg, "github_branch", "main")
        ));
    }
    render::print_line(&format!(
        "  Compression      {}",
        cfg_string(cfg, "compression", "gz").to_uppercase()
    ));
    render::print_line(&format!(
        "  Max backups      {}",
        cfg.get("max_backups")
            .map(text::py_str)
            .unwrap_or_else(|| "5".to_string())
    ));
    render::print_line(&format!(
        "  Auto-backup      {}",
        if sched
            .get("enabled")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            "ENABLED"
        } else {
            "DISABLED"
        }
    ));
    if sched
        .get("enabled")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        render::print_line(&format!(
            "  Schedule         Daily at {:02}:{:02}",
            sched.get("hour").and_then(fmt::py_int).unwrap_or(2),
            sched.get("minute").and_then(fmt::py_int).unwrap_or(0)
        ));
    }
    render::print_line(&format!("  App version      v{}", paths::VERSION));
    render::print_line(&format!(
        "  Config file      {}",
        fmt::short_path(&paths::config_file().to_string_lossy())
    ));
    render::print_line(&format!(
        "  Log file         {}",
        fmt::short_path(&paths::log_file().to_string_lossy())
    ));
    render::print_line("");
    render::print_line("  Environment");
    for (name, needed) in fsutil::required_tools(&storage) {
        match fsutil::which(name) {
            Some(path) => render::print_line(&format!(
                "    {name:<8} ✓  {}",
                fmt::short_path(&path.to_string_lossy())
            )),
            None if needed => render::print_line(&format!("    {name:<8} ✗  missing")),
            None => render::print_line(&format!("    {name:<8} –  not needed for {storage}")),
        }
    }
    if storage == "local" {
        let local = paths::expand_user(&cfg_string(cfg, "local_path", ""));
        if local.exists()
            && let Ok(entries) = fs::read_dir(&local)
        {
            let count = entries
                .flatten()
                .filter(|entry| entry.path().is_dir())
                .count();
            render::print_line(&format!("  Stored backups: {count} at {}", local.display()));
        }
    }
    Ok(Value::Null)
}

pub fn logs(lines: i64) -> Result<Value, String> {
    render::print_section("LOGS", &paths::log_file().to_string_lossy());
    let file = paths::log_file();
    if !file.exists() {
        render::print_line("  No log file yet.");
        return Ok(Value::Null);
    }
    let text = fs::read_to_string(&file).unwrap_or_default();
    let mut all_lines: Vec<&str> = text::py_splitlines(&text);
    if !debug_enabled() {
        all_lines.retain(|line| !line.contains("[TRACE]"));
    }
    let start = if lines >= 0 {
        all_lines.len().saturating_sub(lines as usize)
    } else {
        ((-lines) as usize).min(all_lines.len())
    };
    let selected = &all_lines[start..];
    if selected.is_empty() {
        render::print_line("  No log entries.");
        return Ok(Value::Null);
    }
    if !debug_enabled() {
        render::print_line("  Tip: add --debug to also show saved tracebacks.");
        render::print_line("");
    }
    for line in selected {
        render::print_line(&format!("  {line}"));
    }
    Ok(Value::Null)
}

pub fn clear_logs(assume_yes: bool) -> Result<Value, String> {
    render::print_section("CLEAR LOGS", &paths::log_file().to_string_lossy());
    let file = paths::log_file();
    if !file.exists() {
        render::print_line("  Log file does not exist — nothing to clear.");
        return Ok(Value::Null);
    }
    let size = fs::metadata(&file).map(|meta| meta.len()).unwrap_or(0);
    render::print_line(&format!("  Log file : {}", file.display()));
    render::print_line(&format!("  Size     : {}", fmt::human_size(size as i64)));
    if !assume_yes && !render::confirm("Clear all log entries?", false) {
        render::print_line("  Cancelled.");
        return Ok(Value::Null);
    }
    fs::write(&file, "").map_err(|error| format!("OSError: {error}"))?;
    render::print_line("  ✓ Log file cleared.");
    logging::log("INFO", "Log file cleared by user");
    Ok(Value::Null)
}

fn check_paths(target: &str) -> Vec<(String, PathBuf)> {
    let home = paths::home_dir();
    let prefix = paths::prefix();
    let root = PathBuf::from("/data/data/com.termux/files");
    if target.trim().is_empty() {
        return vec![
            ("Termux root".to_string(), root),
            ("Home (~)".to_string(), home),
            ("$PREFIX".to_string(), prefix),
        ];
    }
    let value = target.trim();
    if matches!(value, "~" | "$HOME" | "home") {
        vec![("Home (~)".to_string(), home)]
    } else if matches!(value, "$PREFIX" | "prefix" | "usr") {
        vec![("$PREFIX".to_string(), prefix)]
    } else if matches!(value, "/data/data/com.termux/files" | "termux" | "root") {
        vec![("Termux root".to_string(), root)]
    } else {
        let path = paths::resolve_lenient(&paths::expand_user(value));
        let label = path.to_string_lossy().into_owned();
        vec![(label, path)]
    }
}

pub fn check(target: &str) -> Result<Value, String> {
    render::print_section("DISK CHECK", "Storage usage overview");
    let paths = check_paths(target);
    if paths.iter().any(|(_, path)| !path.exists()) {
        let missing = paths
            .iter()
            .find(|(_, path)| !path.exists())
            .map(|(_, path)| path.display().to_string())
            .unwrap_or_default();
        render::print_line(&format!("  Path not found: {missing}"));
        return Ok(Value::Null);
    }
    let mut sizes = Vec::new();
    for (_, path) in &paths {
        sizes.push(fsutil::dir_size(path));
    }
    let total = sizes.iter().sum::<i64>().max(1);
    render::print_line("Location Size Usage");
    for ((label, path), size) in paths.iter().zip(sizes.iter()) {
        render::print_line(&format!(
            "  {label:<20} {}  {}",
            fmt::human_size(*size),
            fmt::pct_bar(*size, total, 24)
        ));
        let _ = path;
    }
    if paths.len() == 1 {
        let (_, path) = &paths[0];
        check_subdirs(path, sizes[0]);
    }
    logging::log(
        "INFO",
        &format!(
            "check ran on: [{}]",
            paths
                .iter()
                .map(|(_, path)| format!("'{}'", path.display()))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    );
    Ok(Value::Null)
}

fn check_subdirs(path: &Path, parent_total: i64) {
    let Ok(entries) = fs::read_dir(path) else {
        return;
    };
    let mut children: Vec<(PathBuf, i64)> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| !path.is_symlink())
        .map(|path| {
            let size = if path.is_dir() {
                fsutil::dir_size(&path)
            } else {
                fs::metadata(&path)
                    .map(|meta| meta.len() as i64)
                    .unwrap_or(0)
            };
            (path, size)
        })
        .collect();
    if children.is_empty() {
        return;
    }
    children.sort_by_key(|item| std::cmp::Reverse(item.1));
    children.truncate(12);
    render::print_line("Name Size Usage");
    for (child, size) in children {
        let icon = if child.is_dir() { "📁" } else { "📄" };
        render::print_line(&format!(
            "  {icon} {:<24} {}  {}",
            child
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default(),
            fmt::human_size(size),
            fmt::pct_bar(size, parent_total, 24)
        ));
    }
}

fn cache_targets() -> Vec<(&'static str, PathBuf)> {
    let home = paths::home_dir();
    let prefix = paths::prefix();
    vec![
        ("$PREFIX/tmp", prefix.join("tmp")),
        ("~/.cache", home.join(".cache")),
        ("~/.npm", home.join(".npm")),
        ("~/.cargo/registry", home.join(".cargo").join("registry")),
        ("~/.cargo/git", home.join(".cargo").join("git")),
        ("~/__pycache__", home.join("__pycache__")),
        ("~/.gradle/caches", home.join(".gradle").join("caches")),
        ("~/.m2/repository", home.join(".m2").join("repository")),
        (
            "~/go/pkg/mod/cache",
            home.join("go").join("pkg").join("mod").join("cache"),
        ),
    ]
}

pub fn clear_cache(assume_yes: bool) -> Result<Value, String> {
    render::print_section("CLEAR CACHE", "Remove temporary and cached files");
    let existing: Vec<(&str, PathBuf, i64)> = cache_targets()
        .into_iter()
        .filter(|(_, path)| path.exists())
        .map(|(label, path)| (label, path.clone(), fsutil::dir_size(&path)))
        .collect();
    if existing.is_empty() {
        render::print_line("  Nothing to clear — all cache directories are already empty.");
        return Ok(Value::Null);
    }
    let mut total = 0;
    for (index, (label, _, size)) in existing.iter().enumerate() {
        render::print_line(&format!(
            "  {:>2}  {label:<22} {}",
            index + 1,
            fmt::human_size(*size)
        ));
        total += size;
    }
    render::print_line(&format!("  Total reclaimable: {}", fmt::human_size(total)));
    let mut selected: Vec<usize> = Vec::new();
    if assume_yes {
        selected = (0..existing.len()).collect();
    } else {
        let raw = render::ask("Target (e.g. 1,3 or Enter=all)", "")?;
        if raw.trim().is_empty() {
            selected = (0..existing.len()).collect();
        } else {
            for part in raw.split(',') {
                match part.trim().parse::<usize>() {
                    Ok(value) if value >= 1 && value <= existing.len() => selected.push(value - 1),
                    _ => {
                        render::print_line("  Invalid selection. Aborting.");
                        return Ok(Value::Null);
                    }
                }
            }
        }
    }
    if selected.is_empty() {
        render::print_line("  No targets selected.");
        return Ok(Value::Null);
    }
    if !assume_yes && !render::confirm("Proceed with clearing cache?", false) {
        render::print_line("  Aborted.");
        return Ok(Value::Null);
    }
    render::print_line("  The following will be permanently deleted:");
    for index in &selected {
        let (label, _, size) = &existing[*index];
        render::print_line(&format!("    ✗  {label}  ({})", fmt::human_size(*size)));
    }
    if !assume_yes && !render::confirm("Are you sure? This cannot be undone.", false) {
        render::print_line("  Aborted.");
        return Ok(Value::Null);
    }
    let mut cleared = 0i64;
    let mut failures: Vec<String> = Vec::new();
    for index in selected {
        let (label, path, size) = &existing[index];
        let mut cleared_here = 0u64;
        match fs::read_dir(path) {
            Ok(entries) => {
                for entry in entries.flatten() {
                    let child = entry.path();
                    match remove_cache_entry(&child) {
                        Ok(bytes) => cleared_here += bytes,
                        Err(error) => {
                            logging::log(
                                "WARNING",
                                &format!("Could not remove {}: {error}", child.display()),
                            );
                            failures.push(format!("{}: {error}", child.display()));
                        }
                    }
                }
            }
            Err(error) => {
                logging::log(
                    "WARNING",
                    &format!("Could not read {}: {error}", path.display()),
                );
                failures.push(format!("{}: {error}", path.display()));
            }
        }
        cleared += cleared_here as i64;
        logging::log(
            "INFO",
            &format!(
                "Cleared cache: {} ({})",
                path.display(),
                fmt::human_size(*size)
            ),
        );
        let _ = label;
    }
    render::print_line(&format!(
        "  Done! Freed approximately {}",
        fmt::human_size(cleared)
    ));
    if !failures.is_empty() {
        return Err(format!(
            "RuntimeError: Could not remove: {}",
            failures.join(", ")
        ));
    }
    Ok(Value::Null)
}

fn remove_cache_entry(path: &Path) -> Result<u64, String> {
    let size = fsutil::dir_size(path).max(0) as u64;
    let metadata = fs::symlink_metadata(path).map_err(|error| format!("OSError: {error}"))?;
    if metadata.is_dir() && !metadata.file_type().is_symlink() {
        fs::remove_dir_all(path).map_err(|error| format!("OSError: {error}"))?;
    } else {
        fs::remove_file(path).map_err(|error| format!("OSError: {error}"))?;
    }
    Ok(size)
}

fn ensure_not_live_config(out: &Path) -> Result<(), String> {
    let live = paths::resolve_lenient(&paths::config_file());
    if paths::resolve_lenient(out) == live {
        return Err(
            "termux_sync.UserError: Refusing to export over the live config file".to_string(),
        );
    }
    Ok(())
}

pub fn export_config(output_path: &str) -> Result<Value, String> {
    render::print_section("EXPORT CONFIG", "Save configuration to a file");
    let cfg = config::load_config()?;
    let mut safe = cfg.as_object().cloned().unwrap_or_else(Map::new);
    if safe
        .get("github_token")
        .is_some_and(|value| !text::py_str(value).is_empty())
    {
        safe.insert(
            "github_token".to_string(),
            Value::String("*** REDACTED ***".to_string()),
        );
    }
    if safe
        .get("encrypt_password")
        .is_some_and(|value| !text::py_str(value).is_empty())
    {
        safe.insert(
            "encrypt_password".to_string(),
            Value::String("*** REDACTED ***".to_string()),
        );
    }
    let path = if output_path.is_empty() {
        render::ask(
            "Save to file",
            &paths::home_dir()
                .join("termux-sync-config-export.json")
                .to_string_lossy(),
        )?
    } else {
        output_path.to_string()
    };
    let out = paths::expand_user(&path);
    ensure_not_live_config(&out)?;
    pyjson::write_json_private(&out, &Value::Object(safe))?;
    render::print_line(&format!("  ✓ Configuration exported to: {}", out.display()));
    render::print_line("⚠ Token and password fields have been redacted from the export.");
    logging::log("INFO", &format!("Config exported to {}", out.display()));
    Ok(Value::Null)
}

pub fn import_config(input_path: &str) -> Result<Value, String> {
    render::print_section("IMPORT CONFIG", "Load configuration from a file");
    let path = if input_path.is_empty() {
        render::ask("Path to config file", "")?
    } else {
        input_path.to_string()
    };
    let source = paths::expand_user(&path);
    if !source.exists() {
        return Err(format!("termux_sync.UserError: {}", source.display()));
    }
    let bytes = fs::read(&source).map_err(|error| format!("OSError: {error}"))?;
    let new_cfg: Value =
        serde_json::from_slice(pyjson::strip_utf8_bom(&bytes)).map_err(|error| {
            let column = if error.column() == 0 {
                1
            } else {
                error.column()
            };
            format!(
                "termux_sync.UserError: Not a valid JSON file: {}\nLine {}, column {}: {}",
                source.display(),
                error.line(),
                column,
                error
            )
        })?;
    let Some(new_cfg) = new_cfg.as_object() else {
        return Err(format!(
            "termux_sync.UserError: Not a valid config file: {}\nThe top level of the file must be a JSON object.",
            source.display()
        ));
    };
    config::validate_patch(&Value::Object(new_cfg.clone()))?;
    let mut existing = match config::load_config() {
        Ok(cfg) => cfg,
        Err(error) => {
            render::print_line(&format!("  Warning: {error}"));
            render::print_line("  Starting from the default configuration.");
            Value::Object(config::default_config())
        }
    };
    let Some(existing_map) = existing.as_object_mut() else {
        return Err("AttributeError: config must be a JSON object".to_string());
    };
    for (key, value) in new_cfg {
        if let Value::String(text_value) = value
            && text_value.contains("REDACTED")
        {
            continue;
        }
        existing_map.insert(key.clone(), value.clone());
    }
    if !matches!(
        existing_map.get("compression").map(text::py_str).as_deref(),
        Some("gz" | "bz2" | "xz")
    ) {
        existing_map.insert("compression".to_string(), Value::String("gz".to_string()));
    }
    config::save_config(&existing)?;
    render::print_line(&format!(
        "  ✓ Configuration imported from: {}",
        source.display()
    ));
    logging::log(
        "INFO",
        &format!("Config imported from {}", source.display()),
    );
    Ok(Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn failed_removal_is_an_error() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("cache");
        fs::create_dir_all(dir.join("sub")).unwrap();
        fs::write(dir.join("sub/file"), b"data").unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o555)).unwrap();
        assert!(remove_cache_entry(&dir).is_err());
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    fn non_object_schedule_is_an_error() {
        assert!(normalize_schedule(json!([])).is_err());
        assert!(normalize_schedule(json!({"enabled": "yes"})).is_err());
        assert_eq!(
            normalize_schedule(json!({"enabled": true, "hour": 3, "minute": 4, "label": "x"}))
                .unwrap()["hour"],
            3
        );
    }

    #[test]
    fn daemon_script_quotes_the_path() {
        let body = daemon_script_body(Path::new("/data/a b/it's"));
        assert!(body.contains("'/data/a b/it'\\''s'"));
        assert!(!body.contains("\"/data"));
    }
}
