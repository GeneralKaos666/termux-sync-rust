use std::borrow::Cow;
use std::env;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

#[derive(Debug, Clone)]
pub struct Config {
    pub storage: String,
    pub local_path: String,
    pub compression: String,
    pub max_backups: u64,
    pub schedule: String,
    pub gdrive_folder_id: String,
    pub github_repo: String,
    pub github_branch: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolStatus {
    pub name: &'static str,
    pub path: Option<PathBuf>,
    pub required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusReport {
    pub storage: String,
    pub target: String,
    pub compression: String,
    pub max_backups: u64,
    pub schedule: String,
    pub cli_version: Option<String>,
    pub config_path: String,
    pub log_path: String,
    pub history_path: String,
    pub tools: Vec<ToolStatus>,
}

#[derive(Debug, Clone)]
pub struct Backup {
    pub name: String,
    pub date: String,
    pub label: String,
    pub packages: usize,
    pub size: u64,
    pub version: String,
    pub compression: String,
    pub modified: Option<SystemTime>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LogParts<'a> {
    pub timestamp: &'a str,
    pub level: &'a str,
    pub message: &'a str,
}

pub fn parse_log_line(line: &str) -> Option<LogParts<'_>> {
    let rest = line.strip_prefix('[')?;
    let (timestamp, rest) = rest.split_once(']')?;
    let rest = rest.trim_start().strip_prefix('[')?;
    let (level, message) = rest.split_once(']')?;
    Some(LogParts {
        timestamp,
        level,
        message: message.trim_start(),
    })
}

#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub config: Config,
    pub backups: Vec<Backup>,
    pub log: Vec<String>,
    pub warning: Vec<String>,
}

impl Snapshot {
    /// Joins every warning into the single line the UI renders.
    pub fn warning_text(&self) -> Option<String> {
        (!self.warning.is_empty()).then(|| self.warning.join(" · "))
    }
}

pub fn home() -> PathBuf {
    env::var_os("HOME").map(PathBuf::from).unwrap_or_default()
}

pub fn config_dir() -> PathBuf {
    home().join(".config").join("termux-sync")
}

pub fn config_path() -> PathBuf {
    config_dir().join("config.json")
}

pub fn schedule_path() -> PathBuf {
    config_dir().join("cron.json")
}

pub fn log_path() -> PathBuf {
    config_dir().join("sync.log")
}

pub fn history_path() -> PathBuf {
    config_dir().join("tui-history.json")
}

pub fn load_snapshot() -> Snapshot {
    let mut warning = Vec::new();

    let config = match fs::read_to_string(config_path()) {
        Ok(text) => match serde_json::from_str::<Value>(&text) {
            Ok(value) => {
                let schedule = fs::read_to_string(schedule_path())
                    .ok()
                    .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
                    .unwrap_or(Value::Null);
                parse_config(&value, &schedule)
            }
            Err(err) => {
                warning.push(format!("config.json is not valid JSON: {err}"));
                Config::default()
            }
        },
        Err(_) => Config::default(),
    };

    let backups = if config.storage == "local" {
        match load_local_backups(Path::new(&expand_home(&config.local_path))) {
            Ok(list) => list,
            Err(err) => {
                warning.push(err);
                Vec::new()
            }
        }
    } else {
        warning.push(format!(
            "{} storage: press L to list backups in the TUI",
            config.storage.to_uppercase()
        ));
        Vec::new()
    };

    Snapshot {
        config,
        backups,
        log: tail_log(120),
        warning,
    }
}

pub fn parse_config(cfg: &Value, schedule: &Value) -> Config {
    let storage = string_field(cfg, "storage", "local").to_lowercase();
    let local_path = string_field(cfg, "local_path", "~/termux-backups");
    let compression = string_field(cfg, "compression", "gz");
    let max_backups = cfg.get("max_backups").and_then(Value::as_u64).unwrap_or(5);
    let enabled = schedule
        .get("enabled")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let hour = schedule.get("hour").and_then(Value::as_u64).unwrap_or(2);
    let minute = schedule.get("minute").and_then(Value::as_u64).unwrap_or(0);
    let schedule = if enabled {
        format!("daily {hour:02}:{minute:02}")
    } else {
        "off".to_string()
    };

    Config {
        storage,
        local_path,
        compression,
        max_backups,
        schedule,
        gdrive_folder_id: string_field(cfg, "gdrive_folder_id", ""),
        github_repo: string_field(cfg, "github_repo", ""),
        github_branch: string_field(cfg, "github_branch", "main"),
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            storage: "local".to_string(),
            local_path: "~/termux-backups".to_string(),
            compression: "gz".to_string(),
            max_backups: 5,
            schedule: "off".to_string(),
            gdrive_folder_id: String::new(),
            github_repo: String::new(),
            github_branch: "main".to_string(),
        }
    }
}

pub fn load_local_backups(root: &Path) -> Result<Vec<Backup>, String> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Vec::new());
        }
        Err(err) => {
            return Err(format!("cannot read {}: {err}", root.display()));
        }
    };

    let mut backups = Vec::new();
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let manifest = dir.join("manifest.json");
        let Ok(text) = fs::read_to_string(&manifest) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        if let Some(mut backup) = parse_manifest(&value) {
            backup.modified = fs::metadata(&manifest)
                .and_then(|metadata| metadata.modified())
                .ok();
            backups.push(backup);
        }
    }

    backups.sort_by(|a, b| b.date.cmp(&a.date));
    Ok(backups)
}

pub fn is_safe_backup_name(name: &str) -> bool {
    !name.is_empty()
        && name.trim() == name
        && name != "."
        && name != ".."
        && !name.contains('/')
        && !name.contains('\\')
}

pub fn strip_controls(text: &str) -> Cow<'_, str> {
    if !text
        .chars()
        .any(|ch| ch.is_control() && !matches!(ch, '\n' | '\t'))
    {
        return Cow::Borrowed(text);
    }
    Cow::Owned(
        text.chars()
            .filter(|ch| !ch.is_control() || matches!(ch, '\n' | '\t'))
            .collect(),
    )
}

pub fn parse_manifest(value: &Value) -> Option<Backup> {
    let name = strip_controls(value.get("name")?.as_str()?);
    if !is_safe_backup_name(&name) {
        return None;
    }
    let label = value
        .get("label")
        .and_then(Value::as_str)
        .filter(|label| !label.is_empty() && *label != name)
        .map(strip_controls)
        .unwrap_or_default()
        .into_owned();
    let date = strip_controls(value.get("date").and_then(Value::as_str).unwrap_or("—"))
        .chars()
        .take(16)
        .collect::<String>()
        .replace('T', " ");
    let packages = value
        .get("packages")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    let size = value
        .get("files")
        .and_then(Value::as_object)
        .map(|files| {
            files
                .values()
                .filter_map(|file| file.get("size").and_then(Value::as_u64))
                .sum()
        })
        .unwrap_or(0);
    Some(Backup {
        name: name.into_owned(),
        date,
        label,
        packages,
        size,
        version: strip_controls(&string_field(value, "version", "—")).into_owned(),
        compression: strip_controls(&string_field(value, "compression", "—")).to_uppercase(),
        modified: None,
    })
}

pub fn backup_age(backup: &Backup) -> String {
    let now = SystemTime::now();
    if let Some(modified) = backup.modified {
        let seconds = now
            .duration_since(modified)
            .map(|duration| duration.as_secs() as i64)
            .unwrap_or(0);
        return format_age(seconds.max(0));
    }
    backup_age_at(&backup.date, now)
}

pub fn backup_age_at(date: &str, now: SystemTime) -> String {
    let Some(then) = parse_date_secs(date) else {
        return "—".to_string();
    };
    let now = now
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0);
    format_age((now - then).max(0))
}

fn format_age(seconds: i64) -> String {
    if seconds < 60 {
        "now".to_string()
    } else if seconds < 3_600 {
        format!("{}m", seconds / 60)
    } else if seconds < 86_400 {
        format!("{}h", seconds / 3_600)
    } else if seconds < 14 * 86_400 {
        format!("{}d", seconds / 86_400)
    } else if seconds < 60 * 86_400 {
        format!("{}w", seconds / (7 * 86_400))
    } else if seconds < 365 * 86_400 {
        format!("{}mo", seconds / (30 * 86_400))
    } else {
        format!("{}y", seconds / (365 * 86_400))
    }
}

fn parse_date_secs(date: &str) -> Option<i64> {
    let (day, time) = date.split_once(' ')?;
    let mut day_parts = day.split('-');
    let year: i64 = day_parts.next()?.parse().ok()?;
    let month: u32 = day_parts.next()?.parse().ok()?;
    let day: u32 = day_parts.next()?.parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }

    let mut time_parts = time.split(':');
    let hour: i64 = time_parts.next()?.parse().ok()?;
    let minute: i64 = time_parts.next()?.parse().ok()?;
    let second: i64 = time_parts.next().unwrap_or("0").parse().ok()?;
    if hour > 23 || minute > 59 || second > 59 {
        return None;
    }

    Some(days_from_civil(year, month, day) * 86_400 + hour * 3_600 + minute * 60 + second)
}

fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month_prime = ((month + 9) % 12) as i64;
    let day_of_year = (153 * month_prime + 2) / 5 + day as i64 - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

pub fn tail_log(lines: usize) -> Vec<String> {
    tail_log_bounded(&log_path(), lines, 256 * 1024)
}

fn tail_log_bounded(path: &Path, lines: usize, max_bytes: usize) -> Vec<String> {
    let Ok(mut file) = fs::File::open(path) else {
        return Vec::new();
    };
    let len = file.metadata().map(|meta| meta.len()).unwrap_or(0);
    let start = len.saturating_sub(max_bytes as u64);
    if file.seek(SeekFrom::Start(start)).is_err() {
        return Vec::new();
    }
    let mut text = String::new();
    if file.read_to_string(&mut text).is_err() {
        return Vec::new();
    }
    if start > 0
        && let Some(newline) = text.find('\n')
    {
        text.drain(..=newline);
    }
    let all: Vec<&str> = text.lines().collect();
    let first = all.len().saturating_sub(lines);
    all[first..]
        .iter()
        .map(|line| (*line).to_string())
        .collect()
}

pub fn run_captured(args: &[String]) -> Result<String, String> {
    let output = cli_command(args)
        .output()
        .map_err(|err| format!("cannot start termux-sync: {err}"))?;
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    let text = strip_ansi(&text).trim().to_string();

    if output.status.success() {
        Ok(text)
    } else if text.is_empty() {
        Err(format!("termux-sync exited with {}", output.status))
    } else {
        Err(text)
    }
}

pub fn cli_command(args: &[String]) -> Command {
    let mut command = Command::new(cli_bin());
    command.args(args);
    command
}

pub fn cli_bin() -> PathBuf {
    env::var_os("TERMUX_SYNC_BIN")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("termux-sync"))
}

pub fn expand_home(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("~/") {
        return home().join(rest).to_string_lossy().into_owned();
    }
    if path == "~" {
        return home().to_string_lossy().into_owned();
    }
    path.to_string()
}

pub fn human_size(bytes: u64) -> String {
    let mut value = bytes as f64;
    for unit in ["B", "KB", "MB", "GB"] {
        if value < 1024.0 {
            return format!("{value:.1} {unit}");
        }
        value /= 1024.0;
    }
    format!("{value:.1} TB")
}

pub fn status_target(config: &Config) -> String {
    match config.storage.as_str() {
        "local" => config.local_path.clone(),
        "gdrive" => {
            if config.gdrive_folder_id.is_empty() {
                "(root)".to_string()
            } else {
                config.gdrive_folder_id.clone()
            }
        }
        "github" => {
            if config.github_repo.is_empty() {
                "—".to_string()
            } else if config.github_branch.is_empty() {
                config.github_repo.clone()
            } else {
                format!("{} @ {}", config.github_repo, config.github_branch)
            }
        }
        _ => "—".to_string(),
    }
}

pub fn which_in(name: &str, paths: &[PathBuf]) -> Option<PathBuf> {
    paths
        .iter()
        .map(|path| path.join(name))
        .find(|path| path.is_file())
}

pub fn probe_tools(storage: &str, paths: &[PathBuf]) -> Vec<ToolStatus> {
    termux_sync_core::core::fsutil::required_tools(storage)
        .into_iter()
        .map(|(name, required)| ToolStatus {
            name,
            path: which_in(name, paths),
            required,
        })
        .collect()
}

pub fn cli_version_with(bin: &Path) -> Option<String> {
    let output = Command::new(bin).arg("--version").output().ok()?;
    parse_cli_version(output.status.success(), &output.stdout)
}

pub fn cli_version() -> Option<String> {
    cli_version_with(&cli_bin())
}

pub fn parse_cli_version(success: bool, stdout: &[u8]) -> Option<String> {
    if !success {
        return None;
    }
    String::from_utf8_lossy(stdout)
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with("termux-sync"))
        .map(str::to_string)
}

pub fn collect_status(config: &Config) -> StatusReport {
    let paths: Vec<PathBuf> = env::var_os("PATH")
        .map(|value| env::split_paths(&value).collect())
        .unwrap_or_default();
    let tools = probe_tools(&config.storage, &paths);
    status_report(config, cli_version(), tools)
}

pub fn status_report(
    config: &Config,
    cli_version: Option<String>,
    tools: Vec<ToolStatus>,
) -> StatusReport {
    StatusReport {
        storage: config.storage.to_uppercase(),
        target: status_target(config),
        compression: config.compression.to_uppercase(),
        max_backups: config.max_backups,
        schedule: config.schedule.clone(),
        cli_version,
        config_path: short_path(&config_path()),
        log_path: short_path(&log_path()),
        history_path: short_path(&history_path()),
        tools,
    }
}

pub fn shorten_path(path: &Path, home: &Path, prefix: &Path) -> String {
    for (base, replacement) in [(prefix, "$PREFIX"), (home, "~")] {
        if base.as_os_str().is_empty() {
            continue;
        }
        if path == base {
            return replacement.to_string();
        }
        if let Ok(rest) = path.strip_prefix(base) {
            return format!("{replacement}/{}", rest.display());
        }
    }
    path.display().to_string()
}

pub fn short_path(path: &Path) -> String {
    let prefix = env::var_os("PREFIX").map(PathBuf::from).unwrap_or_default();
    shorten_path(path, &home(), &prefix)
}

pub fn boot_script_path() -> PathBuf {
    home()
        .join(".termux")
        .join("boot")
        .join("termux-sync-daemon.sh")
}

pub fn daemon_activity(lines: &[String]) -> Vec<String> {
    let markers = [
        "Daemon",
        "Next backup",
        "Auto-backup triggered",
        "Daemon backup failed",
        "auto-backup was disabled",
    ];
    let mut activity: Vec<String> = lines
        .iter()
        .filter(|line| markers.iter().any(|marker| line.contains(marker)))
        .cloned()
        .collect();
    let keep = activity.len().saturating_sub(5);
    activity.drain(..keep);
    activity
}

fn string_field(value: &Value, key: &str, default: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or(default)
        .to_string()
}

fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '\u{1b}' {
            out.push(ch);
            continue;
        }
        if chars.peek() == Some(&'[') {
            chars.next();
            for next in chars.by_ref() {
                if next.is_ascii_alphabetic() {
                    break;
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> Value {
        serde_json::json!({
            "name": "termux_backup_20260101_120000",
            "date": "2026-01-01T12:00:00.123456",
            "label": "before-upgrade",
            "packages": ["a", "b", "c"],
            "files": {
                "home.tar.gz": {"size": 2048},
                "usr_etc.tar.gz": {"size": 1024}
            },
            "version": "1.3.9",
            "compression": "gz"
        })
    }

    #[test]
    fn config_parser_keeps_only_display_fields() {
        let raw = serde_json::json!({
            "storage": "GITHUB",
            "local_path": "/tmp/backups",
            "compression": "xz",
            "max_backups": 3,
            "github_token": "secret-token",
            "encrypt_password": "secret-password"
        });
        let schedule = serde_json::json!({"enabled": true, "hour": 4, "minute": 5});
        let config = parse_config(&raw, &schedule);

        assert_eq!(config.storage, "github");
        assert_eq!(config.local_path, "/tmp/backups");
        assert_eq!(config.compression, "xz");
        assert_eq!(config.max_backups, 3);
        assert_eq!(config.schedule, "daily 04:05");
        let debug = format!("{config:?}");
        assert!(!debug.contains("secret-token"));
        assert!(!debug.contains("secret-password"));
    }

    #[test]
    fn config_parser_keeps_display_targets_and_no_secrets() {
        let raw = serde_json::json!({
            "storage": "github",
            "github_repo": "me/backups",
            "github_branch": "backup",
            "gdrive_folder_id": "folder-id",
            "github_token": "secret-token",
            "encrypt_password": "secret-password"
        });
        let config = parse_config(&raw, &serde_json::json!({"enabled": false}));
        assert_eq!(config.github_repo, "me/backups");
        assert_eq!(config.github_branch, "backup");
        assert_eq!(config.gdrive_folder_id, "folder-id");
        let debug = format!("{config:?}");
        assert!(!debug.contains("secret-token"));
        assert!(!debug.contains("secret-password"));
    }

    #[test]
    fn status_target_names_each_backend() {
        let mut config = Config {
            storage: "local".into(),
            local_path: "~/termux-backups".into(),
            ..Config::default()
        };
        assert_eq!(status_target(&config), "~/termux-backups");

        config.storage = "gdrive".into();
        config.gdrive_folder_id.clear();
        assert_eq!(status_target(&config), "(root)");
        config.gdrive_folder_id = "folder-id".into();
        assert_eq!(status_target(&config), "folder-id");

        config.storage = "github".into();
        config.github_repo = "me/backups".into();
        config.github_branch = "backup".into();
        assert_eq!(status_target(&config), "me/backups @ backup");
    }

    #[test]
    fn probe_tools_requires_rclone_only_for_gdrive() {
        let local = probe_tools("local", &[]);
        assert!(local.iter().all(|tool| tool.path.is_none()));
        assert!(local.iter().all(|tool| tool.name != "rclone"));

        let drive = probe_tools("gdrive", &[]);
        assert!(
            drive
                .iter()
                .find(|tool| tool.name == "rclone")
                .unwrap()
                .required
        );
    }

    #[test]
    fn which_in_finds_a_file_in_path_order() {
        let scratch = |name: &str| {
            let dir = std::env::temp_dir().join(format!("ts-tui-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            dir
        };
        let first = scratch("first");
        let second = scratch("second");
        std::fs::write(second.join("git"), "").unwrap();
        let paths = vec![first, second.clone()];
        assert_eq!(which_in("git", &paths), Some(second.join("git")));
    }

    #[test]
    fn shorten_path_replaces_home_and_prefix() {
        let home = Path::new("/data/data/com.termux/files/home");
        let prefix = Path::new("/data/data/com.termux/files/usr");
        assert_eq!(
            shorten_path(&home.join(".config/termux-sync"), home, prefix),
            "~/.config/termux-sync"
        );
        assert_eq!(
            shorten_path(&prefix.join("bin/tar"), home, prefix),
            "$PREFIX/bin/tar"
        );
    }

    #[test]
    fn cli_version_rejects_failed_or_unrelated_output() {
        assert_eq!(
            parse_cli_version(true, b"termux-sync v1.3.9\n"),
            Some("termux-sync v1.3.9".to_string())
        );
        assert_eq!(parse_cli_version(false, b"termux-sync v1.3.9\n"), None);
        assert_eq!(parse_cli_version(true, b"GNU coreutils 9.7\n"), None);
    }

    #[test]
    fn daemon_activity_keeps_only_daemon_lines_and_caps_them() {
        let lines: Vec<String> = (0..10)
            .map(|index| format!("line {index} [INFO] Daemon tick {index}"))
            .chain(std::iter::once("plain line".to_string()))
            .collect();
        let activity = daemon_activity(&lines);
        assert_eq!(activity.len(), 5);
        assert!(activity.iter().all(|line| line.contains("Daemon")));
        assert_eq!(activity.last().unwrap(), "line 9 [INFO] Daemon tick 9");
    }

    #[test]
    fn manifest_parser_reads_summary_fields() {
        let backup = parse_manifest(&manifest()).expect("manifest parses");

        assert_eq!(backup.name, "termux_backup_20260101_120000");
        assert_eq!(backup.date, "2026-01-01 12:00");
        assert_eq!(backup.label, "before-upgrade");
        assert_eq!(backup.packages, 3);
        assert_eq!(backup.size, 3072);
        assert_eq!(backup.version, "1.3.9");
        assert_eq!(backup.compression, "GZ");
    }

    #[test]
    fn age_uses_compact_units() {
        use std::time::Duration;

        let base = parse_date_secs("2026-01-01 12:00").unwrap();
        let at = |seconds: u64| UNIX_EPOCH + Duration::from_secs(base as u64 + seconds);

        assert_eq!(backup_age_at("2026-01-01 12:00", at(30)), "now");
        assert_eq!(backup_age_at("2026-01-01 12:00", at(5 * 60)), "5m");
        assert_eq!(backup_age_at("2026-01-01 12:00", at(3 * 3_600)), "3h");
        assert_eq!(backup_age_at("2026-01-01 12:00", at(2 * 86_400)), "2d");
        assert_eq!(backup_age_at("2026-01-01 12:00", at(21 * 86_400)), "3w");
        assert_eq!(backup_age_at("—", at(0)), "—");
    }

    #[test]
    fn backup_age_prefers_manifest_mtime() {
        use std::time::Duration;

        let mut backup = parse_manifest(&manifest()).unwrap();
        backup.modified = Some(SystemTime::now() - Duration::from_secs(2 * 3_600));
        assert_eq!(backup_age(&backup), "2h");
    }

    #[test]
    fn date_parser_handles_epoch() {
        assert_eq!(parse_date_secs("1970-01-01 00:00"), Some(0));
        assert_eq!(parse_date_secs("1970-01-02 00:00"), Some(86_400));
        assert_eq!(parse_date_secs("not a date"), None);
    }

    #[test]
    fn log_line_parser_splits_timestamp_level_and_message() {
        let parts = parse_log_line("[2026-11-21 01:17:33] [INFO] Collected 1947 packages").unwrap();
        assert_eq!(parts.timestamp, "2026-11-21 01:17:33");
        assert_eq!(parts.level, "INFO");
        assert_eq!(parts.message, "Collected 1947 packages");

        assert!(parse_log_line("plain text").is_none());
        assert!(parse_log_line("[2026-11-21 01:17:33] no level").is_none());
    }

    #[test]
    fn unsafe_manifest_name_is_skipped() {
        let mut value = manifest();
        value["name"] = serde_json::json!("../other");
        assert!(parse_manifest(&value).is_none());
    }

    #[test]
    fn manifest_without_name_is_skipped() {
        let value = serde_json::json!({"date": "2026-01-01"});
        assert!(parse_manifest(&value).is_none());
    }

    #[test]
    fn human_size_uses_binary_units() {
        assert_eq!(human_size(0), "0.0 B");
        assert_eq!(human_size(1536), "1.5 KB");
        assert_eq!(human_size(3 * 1024 * 1024), "3.0 MB");
    }

    #[test]
    fn ansi_sequences_are_removed() {
        assert_eq!(strip_ansi("\u{1b}[31mred\u{1b}[0m text"), "red text");
    }
}

#[cfg(test)]
mod task11_tests {
    use super::*;

    #[test]
    fn probe_tools_omits_python_and_tar() {
        let tools = probe_tools("local", &[]);
        assert!(tools.iter().all(|tool| tool.name != "python3"));
        assert!(tools.iter().all(|tool| tool.name != "tar"));
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "dpkg");
    }
}

#[cfg(test)]
mod task15_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn strips_escape_sequences_from_labels_and_logs() {
        assert_eq!(strip_controls("ok\u{1b}[31mred\u{1b}[0m"), "ok[31mred[0m");
        assert_eq!(strip_controls("a\u{0007}b"), "ab");
        let backup = parse_manifest(&serde_json::json!({
            "name": "termux_backup_x",
            "label": "x\u{1b}]0;pwned\u{0007}"
        }))
        .unwrap();
        assert!(!backup.label.contains('\u{1b}'));
    }

    #[test]
    fn tail_reads_only_the_end_of_a_large_log() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("sync.log");
        let mut text = String::new();
        for i in 0..50_000 {
            text.push_str(&format!("line {i}\n"));
        }
        fs::write(&path, text).unwrap();
        let tail = tail_log_bounded(&path, 2, 4096);
        assert_eq!(tail.len(), 2);
        assert!(tail[1].starts_with("line 49999"));
    }

    #[test]
    fn cli_version_queries_the_binary() {
        let temp = tempfile::tempdir().unwrap();
        let bin = temp.path().join("fake");
        fs::write(&bin, "#!/bin/sh\nprintf 'termux-sync v9.9.9\\n'\n").unwrap();
        fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            cli_version_with(&bin),
            Some("termux-sync v9.9.9".to_string())
        );
    }
}
