//! Backup and restore orchestration for the local backend.

use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use chrono::Local;
use serde_json::{Map, Value, json};

use crate::core::archive::{self, ArchiveOptions};
use crate::core::config;
use crate::core::errors::CoreError;
use crate::core::events::{self, EventProgress};
use crate::core::fmt;
use crate::core::logging;
use crate::core::manifest;
use crate::core::packages;
use crate::core::paths;
use crate::core::render;
use crate::core::storage::{self, Storage};
use crate::core::text;

const TOTAL_STAGES: i64 = 7;

fn cfg_string(cfg: &Value, key: &str, default: &str) -> String {
    cfg.get(key)
        .map(text::py_str)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| default.to_string())
}

fn excludes(cfg: &Value) -> Vec<String> {
    cfg.get("exclude_patterns")
        .and_then(Value::as_array)
        .map_or_default(|items| items.iter().map(text::py_str).collect())
}

fn text_field(value: &Value, key: &str) -> String {
    value.get(key).map_or_default(text::py_str)
}

fn text_field_or_dash(value: &Value, key: &str) -> String {
    value.get(key).map_or_else(|| "—".to_string(), text::py_str)
}

fn label_field(value: &Value) -> String {
    value
        .get("label")
        .map(text::py_str)
        .filter(|label| !label.is_empty())
        .unwrap_or_else(|| "—".to_string())
}

fn package_count(value: &Value) -> usize {
    value
        .get("packages")
        .and_then(Value::as_array)
        .map_or(0, Vec::len)
}

/// Sum `files[*].size` from an object map, ignoring entries without a number.
fn files_total_size(files: &Map<String, Value>) -> i64 {
    files
        .values()
        .filter_map(|info| info.get("size"))
        .filter_map(fmt::py_int)
        .sum()
}

/// Sum `files[*].size` from a manifest value without cloning the files map.
fn manifest_total_size(manifest: &Value) -> i64 {
    manifest
        .get("files")
        .and_then(Value::as_object)
        .map_or(0, files_total_size)
}

/// Stage event fields. Key order is part of the TUI protocol: id, label, state,
/// index, total.
fn stage_fields(id: &str, label: &str, state: &str, index: i64) -> Vec<(&'static str, Value)> {
    vec![
        ("id", Value::String(id.to_string())),
        ("label", Value::String(label.to_string())),
        ("state", Value::String(state.to_string())),
        ("index", Value::from(index)),
        ("total", Value::from(TOTAL_STAGES)),
    ]
}

fn emit_stage(id: &str, label: &str, state: &str, index: i64) {
    events::emit("stage", stage_fields(id, label, state, index));
}

fn emit_start(command: &str, name: &str, cfg: &Value) {
    events::emit(
        "start",
        vec![
            ("command", Value::String(command.to_string())),
            ("name", Value::String(name.to_string())),
            (
                "storage",
                Value::String(cfg_string(cfg, "storage", "local")),
            ),
            ("version", Value::String(paths::VERSION.to_string())),
        ],
    );
}

fn cleanup(path: &Path) {
    let _ = fs::remove_dir_all(path);
}

fn prepare_work_dir(path: &Path) -> Result<(), CoreError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(CoreError::os)?;
    }
    fs::create_dir(path).map_err(CoreError::os)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(CoreError::os)
}

fn restore_work_dir(name: &str) -> Result<PathBuf, CoreError> {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let dir = paths::tmp_root().join(format!("restore_{name}_{}_{}", std::process::id(), nanos));
    prepare_work_dir(&dir)?;
    Ok(dir)
}

fn ensure_extract_space(root: &Path, needed: u64) -> Result<(), CoreError> {
    let available = fs4::available_space(root).map_err(CoreError::os)?;
    let required = needed.saturating_add(64 * 1024 * 1024);
    if available < required {
        return Err(CoreError::user("Not enough free space"));
    }
    Ok(())
}

/// Report the configured backend as `{"kind": ...}` without touching it.
pub fn get_storage(cfg: &Value) -> Result<Value, CoreError> {
    let storage = Storage::from_config(cfg)?;
    Ok(json!({ "kind": storage.kind() }))
}

fn emit_backup_listing(cfg: &Value, backups: &[Value]) {
    if !events::enabled() {
        return;
    }
    for backup in backups {
        events::emit(
            "backup",
            vec![
                ("name", Value::String(text_field(backup, "name"))),
                ("date", Value::String(text_field(backup, "date"))),
                ("label", Value::String(text_field(backup, "label"))),
                ("packages", Value::from(package_count(backup))),
                ("size", Value::from(manifest_total_size(backup))),
                ("version", Value::String(text_field(backup, "version"))),
                (
                    "compression",
                    Value::String(text_field(backup, "compression")),
                ),
                (
                    "storage",
                    Value::String(cfg_string(cfg, "storage", "local")),
                ),
            ],
        );
    }
    events::emit(
        "result",
        vec![
            ("ok", Value::Bool(true)),
            ("count", Value::from(backups.len())),
        ],
    );
}

fn print_backup_entry(index: usize, backup: &Value) {
    render::print_line(&format!("Backup #{}", index + 1));
    render::print_line(&format!("Name {}", text_field_or_dash(backup, "name")));
    render::print_line(&format!(
        "Date {}",
        text_field(backup, "date")
            .chars()
            .take(19)
            .collect::<String>()
            .replace('T', " ")
    ));
    render::print_line(&format!("Label {}", label_field(backup)));
    render::print_line(&format!("Packages {}", package_count(backup)));
    render::print_line(&format!(
        "Files {}",
        backup
            .get("files")
            .and_then(Value::as_object)
            .map_or(0, Map::len)
    ));
    render::print_line(&format!(
        "Version {}",
        text_field_or_dash(backup, "version")
    ));
    render::print_line(&format!(
        "Compression {}",
        text_field_or_dash(backup, "compression").to_uppercase()
    ));
    let size = manifest_total_size(backup);
    if size != 0 {
        render::print_line(&format!("Total size {}", fmt::human_size(size)));
    }
}

/// Print the table of available backups and return them as a JSON array.
pub fn list(cfg: &Value) -> Result<Value, CoreError> {
    let storage = Storage::from_config(cfg)?;
    render::print_section(
        "BACKUP LIST",
        &format!(
            "Storage: {}",
            cfg_string(cfg, "storage", "local").to_uppercase()
        ),
    );
    let backups: Vec<Value> = storage
        .list_backups()?
        .into_iter()
        .filter(|backup| manifest::manifest_shape(backup, false))
        .collect();
    emit_backup_listing(cfg, &backups);
    if backups.is_empty() {
        render::print_line("  No backups found.");
        return Ok(Value::Null);
    }
    render::print_line(&format!("{} Backup(s) Found", backups.len()));
    for (index, backup) in backups.iter().enumerate() {
        print_backup_entry(index, backup);
    }
    Ok(Value::Null)
}

fn build_backup_name(timestamp: &str, label: &str) -> String {
    let clean_label = if label.is_empty() {
        String::new()
    } else {
        manifest::clean_label(label)
    };
    let mut backup_name = format!("termux_backup_{timestamp}");
    if !clean_label.is_empty() {
        backup_name.push('_');
        backup_name.push_str(&clean_label);
    }
    backup_name
}

fn resolve_compression(cfg: &Value) -> String {
    let compression = cfg_string(cfg, "compression", "gz");
    if matches!(compression.as_str(), "gz" | "bz2" | "xz") {
        compression
    } else {
        render::print_line(&format!(
            "  Warning: compression '{compression}' is invalid, falling back to 'gz'."
        ));
        "gz".to_string()
    }
}

fn run_preflight(cfg: &Value, tmp_dir: &Path, backup_name: &str) -> Result<Storage, CoreError> {
    let mut storage = match Storage::from_config(cfg) {
        Ok(storage) => storage,
        Err(error) => {
            cleanup(tmp_dir);
            return Err(error);
        }
    };
    if let Err(error) = storage.preflight() {
        cleanup(tmp_dir);
        return Err(error);
    }
    if let Storage::GitHub(github) = &mut storage
        && let Err(error) = github.begin_stream(backup_name, &tmp_dir.join(".parts"))
    {
        cleanup(tmp_dir);
        return Err(error);
    }
    emit_stage("preflight", "Preflight", "start", 1);
    emit_stage("preflight", "Preflight", "complete", 1);
    Ok(storage)
}

fn build_meta(
    cfg: &Value,
    backup_name: &str,
    label: &str,
    compression: &str,
) -> Map<String, Value> {
    let mut meta = Map::new();
    meta.insert("name".to_string(), Value::String(backup_name.to_string()));
    meta.insert(
        "date".to_string(),
        Value::String(Local::now().format("%Y-%m-%dT%H:%M:%S%.6f").to_string()),
    );
    meta.insert("label".to_string(), Value::String(label.to_string()));
    meta.insert(
        "termux_home".to_string(),
        Value::String(paths::termux_files().to_string_lossy().into_owned()),
    );
    meta.insert(
        "termux_usr".to_string(),
        Value::String(paths::prefix().to_string_lossy().into_owned()),
    );
    meta.insert(
        "storage".to_string(),
        Value::String(cfg_string(cfg, "storage", "local")),
    );
    meta.insert(
        "compression".to_string(),
        Value::String(compression.to_string()),
    );
    meta.insert(
        "version".to_string(),
        Value::String(paths::VERSION.to_string()),
    );
    meta.insert("files".to_string(), Value::Object(Map::new()));
    meta.insert("packages".to_string(), Value::Array(Vec::new()));
    meta
}

fn collect_packages(
    tmp_dir: &Path,
    meta: &mut Map<String, Value>,
) -> Result<Vec<String>, CoreError> {
    render::print_line("  Step 1/4  Collecting installed packages…");
    emit_stage("packages", "Packages", "start", 2);
    let installed = match packages::installed_packages() {
        Ok(list) => list,
        Err(error) => {
            cleanup(tmp_dir);
            return Err(error);
        }
    };
    meta.insert(
        "packages".to_string(),
        Value::Array(installed.iter().cloned().map(Value::String).collect()),
    );
    fs::write(tmp_dir.join("packages.txt"), installed.join("\n")).map_err(CoreError::os)?;
    render::print_line(&format!("  ✓ Found {} installed packages", installed.len()));
    logging::log("INFO", &format!("Collected {} packages", installed.len()));
    events::emit(
        "progress",
        vec![
            ("stage", Value::String("packages".to_string())),
            ("current", Value::from(installed.len())),
            ("total", Value::from(installed.len())),
            ("unit", Value::String("packages".to_string())),
        ],
    );
    emit_stage("packages", "Packages", "complete", 2);
    Ok(installed)
}

struct ArchiveContext<'a> {
    cfg: &'a Value,
    compression: &'a str,
    excludes: &'a [String],
    tmp_dir: &'a Path,
    backup_name: &'a str,
}

fn record_archive(
    meta: &mut Map<String, Value>,
    key: &str,
    source: &Path,
    name: &str,
    size: u64,
    checksum: &str,
) {
    if let Some(Value::Object(files)) = meta.get_mut("files") {
        files.insert(
            key.to_string(),
            json!({
                "archive": name,
                "size": size,
                "checksum": checksum,
                "source": source.to_string_lossy(),
            }),
        );
    }
}

impl ArchiveContext<'_> {
    fn archive_item(
        &self,
        storage: &Storage,
        meta: &mut Map<String, Value>,
        key: &str,
        label_text: &str,
        source: &Path,
        stage_index: i64,
    ) -> Result<(), CoreError> {
        let stage_id = format!("archive:{key}");
        emit_stage(&stage_id, label_text, "start", stage_index);
        if !source.exists() {
            render::print_line(&format!(
                "  Warning: skipping {} (not found)",
                source.display()
            ));
            emit_stage(&stage_id, label_text, "skipped", stage_index);
            return Ok(());
        }
        let out_name = format!("{key}.tar.{}", self.compression);
        let out_path = self.tmp_dir.join(&out_name);
        let mut item_excludes = self.excludes.to_vec();
        item_excludes.extend(manifest::storage_excludes(self.cfg, source));
        item_excludes.extend(manifest::self_excludes(source));
        let options = ArchiveOptions {
            compression: self.compression.to_string(),
            excludes: item_excludes,
            label: label_text.to_string(),
        };
        let mut seen = 0u64;
        let mut throttle = EventProgress::new(stage_id.clone());
        let mut on_member = |path: &Path, size: u64| {
            seen += size;
            throttle.emit(seen, None, "bytes", Some(&path.to_string_lossy()), false);
        };
        let injected_failure =
            std::env::var("TERMUX_SYNC_TEST_ARCHIVE_FAIL").is_ok_and(|want| want == key);

        let archive_result: Result<(String, u64, String), CoreError> = if injected_failure {
            Err(CoreError::plain("injected archive failure"))
        } else if let Storage::GitHub(github) = storage {
            match github.stream_sink(&out_name) {
                Ok(sink) => {
                    match archive::create_archive_to_writer(source, sink, &options, &mut on_member)
                    {
                        Ok(sink) => sink
                            .finish()
                            .map(|(total, digest)| (out_name.clone(), total, digest)),
                        Err(error) => Err(error),
                    }
                }
                Err(error) => Err(error),
            }
        } else {
            archive::create_archive(source, &out_path, &options, &mut on_member).and_then(
                |result| {
                    let name = result
                        .path
                        .file_name()
                        .map_or_default(|name| name.to_string_lossy().into_owned());
                    let checksum = storage::sha256_file(&result.path)?;
                    Ok((name, result.size, checksum))
                },
            )
        };
        match archive_result {
            Ok((name, archive_size, checksum)) => {
                record_archive(meta, key, source, &name, archive_size, &checksum);
                emit_stage(&stage_id, label_text, "complete", stage_index);
                events::emit(
                    "progress",
                    vec![
                        ("stage", Value::String(stage_id)),
                        ("current", Value::from(archive_size)),
                        ("total", Value::from(archive_size)),
                        ("unit", Value::String("bytes".to_string())),
                    ],
                );
                logging::log(
                    "INFO",
                    &format!(
                        "Archived {} → {} ({})",
                        source.display(),
                        name,
                        fmt::human_size(archive_size as i64)
                    ),
                );
                Ok(())
            }
            Err(error) => {
                emit_stage(&stage_id, label_text, "failed", stage_index);
                render::print_line(&format!(
                    "  ✗ Failed to archive {}: {error}",
                    source.display()
                ));
                logging::log(
                    "ERROR",
                    &format!("Archive failed for {}: {error}", source.display()),
                );
                if let Storage::GitHub(github) = storage {
                    github.abort_stream(self.backup_name);
                }
                cleanup(self.tmp_dir);
                Err(error)
            }
        }
    }
}

fn archive_directories(
    ctx: &ArchiveContext<'_>,
    storage: &Storage,
    meta: &mut Map<String, Value>,
) -> Result<(), CoreError> {
    render::print_line("");
    render::print_line("  Step 2/4  Archiving directories…");
    render::print_line("");
    let items = [
        ("home", "Home directory (~)", paths::termux_files()),
        (
            "usr_etc",
            "Termux config (/usr/etc)",
            paths::prefix().join("etc"),
        ),
    ];
    for (key, label_text, source) in items {
        let stage_index = if key == "home" { 3 } else { 4 };
        ctx.archive_item(storage, meta, key, label_text, &source, stage_index)?;
    }
    Ok(())
}

fn write_manifest(tmp_dir: &Path, meta: Map<String, Value>) -> Result<Value, CoreError> {
    render::print_line("");
    render::print_line("  Step 3/4  Writing manifest…");
    emit_stage("manifest", "Manifest", "start", 5);
    let meta_value = Value::Object(meta);
    crate::core::json::write_json_pretty_ascii(&tmp_dir.join("manifest.json"), &meta_value)?;
    render::print_line("  ✓ Manifest written");
    emit_stage("manifest", "Manifest", "complete", 5);
    Ok(meta_value)
}

fn upload_backup(
    storage: &Storage,
    cfg: &Value,
    tmp_dir: &Path,
    backup_name: &str,
) -> Result<String, CoreError> {
    render::print_line("");
    render::print_line(&format!(
        "  Step 4/4  Uploading to {}…",
        cfg_string(cfg, "storage", "local").to_uppercase()
    ));
    render::print_line("");
    emit_stage("upload", "Upload", "start", 6);
    let uploaded = match storage.upload(tmp_dir, backup_name) {
        Ok(uploaded) => uploaded,
        Err(error) => {
            emit_stage("upload", "Upload", "failed", 6);
            logging::log("ERROR", &format!("Upload failed: {error}"));
            if let Storage::GitHub(github) = storage {
                github.abort_stream(backup_name);
            }
            return Err(error);
        }
    };
    emit_stage("upload", "Upload", "complete", 6);
    Ok(uploaded.location)
}

struct VerifyOutcome {
    verified: Option<bool>,
    details: String,
    total_size: i64,
}

fn verify_backup(storage: &Storage, meta_value: &Value, location: &str) -> VerifyOutcome {
    emit_stage("verify", "Verify", "start", 7);
    let total_size = manifest_total_size(meta_value);
    let empty = Value::Object(Map::new());
    let files = meta_value.get("files").unwrap_or(&empty);
    let (verified, details): (Option<bool>, String) = match storage {
        Storage::GitHub(_) => (Some(true), "stream digest".to_string()),
        Storage::Local(_) => {
            if std::env::var("TERMUX_SYNC_TEST_VERIFY_FAIL").is_ok() {
                (Some(false), "injected verification failure".to_string())
            } else {
                let (ok, details) = storage::verify_local_backup(Path::new(location), files);
                (Some(ok), details)
            }
        }
        Storage::Rclone(_) => (
            None,
            "verification unavailable for this backend".to_string(),
        ),
    };
    let verify_state = match verified {
        Some(true) => "verified",
        Some(false) => "failed",
        None => "skipped",
    };
    events::emit(
        "verify",
        vec![
            ("state", Value::String(verify_state.to_string())),
            ("details", Value::String(details.clone())),
        ],
    );
    emit_stage(
        "verify",
        "Verify",
        if verified == Some(false) {
            "failed"
        } else {
            "complete"
        },
        7,
    );
    VerifyOutcome {
        verified,
        details,
        total_size,
    }
}

struct BackupSummary<'a> {
    name: &'a str,
    meta: &'a Value,
    installed: usize,
    total_size: i64,
    location: &'a str,
    verified: Option<bool>,
    verify_details: &'a str,
    started: Instant,
}

fn report_backup_success(cfg: &Value, summary: &BackupSummary<'_>) -> Result<Value, CoreError> {
    render::print_line("");
    render::print_line("  ✓ Backup completed successfully!");
    render::print_line("");
    render::print_line(&format!("  Name      {}", summary.name));
    render::print_line(&format!(
        "  Date      {}",
        summary
            .meta
            .get("date")
            .map_or_default(text::py_str)
            .chars()
            .take(19)
            .collect::<String>()
            .replace('T', " ")
    ));
    render::print_line(&format!("  Packages  {}", summary.installed));
    render::print_line(&format!(
        "  Size      {}",
        fmt::human_size(summary.total_size)
    ));
    render::print_line(&format!(
        "  Storage   {}",
        cfg_string(cfg, "storage", "local").to_uppercase()
    ));
    render::print_line(&format!(
        "  Location  {}",
        fmt::short_path(summary.location)
    ));
    render::print_line("");
    logging::log(
        "INFO",
        &format!(
            "Backup completed: {} ({})",
            summary.name,
            fmt::human_size(summary.total_size)
        ),
    );
    events::emit(
        "result",
        vec![
            ("ok", Value::Bool(true)),
            ("name", Value::String(summary.name.to_string())),
            ("size", Value::from(summary.total_size)),
            ("location", Value::String(summary.location.to_string())),
            (
                "verified",
                summary.verified.map_or(Value::Null, Value::Bool),
            ),
            (
                "verify_details",
                Value::String(summary.verify_details.to_string()),
            ),
            (
                "duration_ms",
                Value::from(summary.started.elapsed().as_millis() as i64),
            ),
        ],
    );
    Ok(Value::Null)
}

fn report_backup_failure(
    backup_name: &str,
    total_size: i64,
    location: &str,
    details: &str,
    tmp_dir: &Path,
) -> Result<Value, CoreError> {
    events::emit(
        "result",
        vec![
            ("ok", Value::Bool(false)),
            ("name", Value::String(backup_name.to_string())),
            ("size", Value::from(total_size)),
            ("location", Value::String(location.to_string())),
            ("verified", Value::Bool(false)),
            ("verify_details", Value::String(details.to_string())),
            ("warnings", Value::Bool(true)),
        ],
    );
    cleanup(tmp_dir);
    Err(CoreError::runtime(format!(
        "Backup verification failed: {details}"
    )))
}

/// Run a full backup: archive the configured sources, upload them and prune old
/// backups. Returns the manifest as JSON; progress is printed and emitted.
pub fn backup(cfg: &Value, label: &str) -> Result<Value, CoreError> {
    let started = Instant::now();
    render::print_section(
        "BACKUP",
        "Creating a full snapshot of your Termux environment",
    );

    let timestamp = Local::now().format("%Y%m%d_%H%M%S").to_string();
    let backup_name = build_backup_name(&timestamp, label);
    emit_start("backup", &backup_name, cfg);

    let tmp_dir = paths::tmp_root().join(&backup_name);
    prepare_work_dir(&tmp_dir)?;

    let compression = resolve_compression(cfg);
    let excludes = excludes(cfg);
    let storage = run_preflight(cfg, &tmp_dir, &backup_name)?;

    let mut meta = build_meta(cfg, &backup_name, label, &compression);
    let installed = collect_packages(&tmp_dir, &mut meta)?;
    let ctx = ArchiveContext {
        cfg,
        compression: &compression,
        excludes: &excludes,
        tmp_dir: &tmp_dir,
        backup_name: &backup_name,
    };
    archive_directories(&ctx, &storage, &mut meta)?;
    let meta_value = write_manifest(&tmp_dir, meta)?;
    let location = upload_backup(&storage, cfg, &tmp_dir, &backup_name)?;
    let outcome = verify_backup(&storage, &meta_value, &location);

    if outcome.verified != Some(false) {
        storage.delete_old(config::retention_limit(cfg))?;
    }

    if outcome.verified == Some(false) {
        return report_backup_failure(
            &backup_name,
            outcome.total_size,
            &location,
            &outcome.details,
            &tmp_dir,
        );
    }

    cleanup(&tmp_dir);

    let summary = BackupSummary {
        name: &backup_name,
        meta: &meta_value,
        installed: installed.len(),
        total_size: outcome.total_size,
        location: &location,
        verified: outcome.verified,
        verify_details: &outcome.details,
        started,
    };
    report_backup_success(cfg, &summary)
}

struct PackageDiff {
    available: bool,
    to_install: Vec<String>,
    installed: Vec<String>,
    device_only: Vec<String>,
}

fn package_diff_with(current: &BTreeSet<String>, backup_packages: &[String]) -> PackageDiff {
    if current.is_empty() {
        return PackageDiff {
            available: false,
            to_install: Vec::new(),
            installed: Vec::new(),
            device_only: Vec::new(),
        };
    }
    let backup: BTreeSet<String> = backup_packages.iter().cloned().collect();
    PackageDiff {
        available: true,
        to_install: backup.difference(current).cloned().collect(),
        installed: backup.intersection(current).cloned().collect(),
        device_only: current.difference(&backup).cloned().collect(),
    }
}

fn package_diff(backup_packages: &[String]) -> PackageDiff {
    let current: BTreeSet<String> = packages::installed_packages()
        .unwrap_or_default()
        .into_iter()
        .collect();
    package_diff_with(&current, backup_packages)
}

fn emit_package_diff(category: &str, packages: &[String]) {
    if packages.is_empty() {
        events::emit(
            "package_diff",
            vec![
                ("category", Value::String(category.to_string())),
                ("part", Value::from(1)),
                ("parts", Value::from(1)),
                ("packages", Value::Array(Vec::new())),
            ],
        );
        return;
    }
    let chunk = 500usize;
    let parts = packages.len().div_ceil(chunk).max(1);
    for (index, chunk_items) in packages.chunks(chunk).enumerate() {
        events::emit(
            "package_diff",
            vec![
                ("category", Value::String(category.to_string())),
                ("part", Value::from(index + 1)),
                ("parts", Value::from(parts)),
                (
                    "packages",
                    Value::Array(chunk_items.iter().cloned().map(Value::String).collect()),
                ),
            ],
        );
    }
}

fn restore_roots() -> HashMap<&'static str, (PathBuf, String)> {
    let mut roots = HashMap::new();
    for (key, root) in [
        ("home", paths::termux_files()),
        ("usr_etc", paths::prefix().join("etc")),
    ] {
        let parent = root
            .parent()
            .map_or_else(|| PathBuf::from("/"), Path::to_path_buf);
        let top = root
            .file_name()
            .map_or_default(|name| name.to_string_lossy().into_owned());
        roots.insert(key, (paths::resolve_lenient(&parent), top));
    }
    roots
}

fn extract_stage(key: &str) -> (&str, i64) {
    match key {
        "home" => ("Home", 4),
        "usr_etc" => ("Config", 5),
        _ => (key, 4),
    }
}

struct RestorePlan<'a> {
    name: &'a str,
    manifest: &'a Value,
    backup_packages: &'a [String],
    archives: Vec<String>,
    size: i64,
    diff: PackageDiff,
}

impl RestorePlan<'_> {
    fn emit(&self, cfg: &Value) {
        let mut plan: Vec<(&str, Value)> = vec![
            ("name", Value::String(self.name.to_string())),
            ("date", Value::String(text_field(self.manifest, "date"))),
            ("label", Value::String(text_field(self.manifest, "label"))),
            (
                "storage",
                Value::String(cfg_string(cfg, "storage", "local")),
            ),
            (
                "versions",
                json!({
                    "backup": self.manifest.get("version").map_or_else(|| "—".to_string(), text::py_str),
                    "current": paths::VERSION,
                }),
            ),
            ("packages", Value::from(self.backup_packages.len() as i64)),
            ("diff_available", Value::Bool(self.diff.available)),
            (
                "archives",
                Value::Array(self.archives.iter().cloned().map(Value::String).collect()),
            ),
            ("size", Value::from(self.size)),
            ("checksums", Value::String("pending".to_string())),
        ];
        if self.diff.available {
            plan.push(("to_install", Value::from(self.diff.to_install.len() as i64)));
            plan.push((
                "already_installed",
                Value::from(self.diff.installed.len() as i64),
            ));
            plan.push((
                "device_only",
                Value::from(self.diff.device_only.len() as i64),
            ));
        }
        events::emit("plan", plan);
        if self.diff.available {
            emit_package_diff("to_install", &self.diff.to_install);
            emit_package_diff("already_installed", &self.diff.installed);
            emit_package_diff("device_only", &self.diff.device_only);
        }
    }
}

fn load_restore_manifest(
    storage: &Storage,
    name: &str,
    tmp_dir: &Path,
) -> Result<Value, CoreError> {
    emit_stage("metadata", "Metadata", "start", 1);
    let manifest = match storage.fetch_manifest(name) {
        Ok(Some(manifest)) => manifest,
        Ok(None) => {
            cleanup(tmp_dir);
            return Err(CoreError::user("Could not read manifest"));
        }
        Err(error) => {
            cleanup(tmp_dir);
            return Err(error);
        }
    };
    if !manifest::manifest_shape(&manifest, true) {
        cleanup(tmp_dir);
        return Err(CoreError::user("Backup manifest is malformed"));
    }
    emit_stage("metadata", "Metadata", "complete", 1);
    Ok(manifest)
}

fn confirm_restore() -> bool {
    if events::enabled() {
        events::confirmation()
    } else {
        render::confirm("Proceed with restore?", false)
    }
}

fn download_backup(storage: &Storage, name: &str, tmp_dir: &Path) -> Result<(), CoreError> {
    emit_stage("download", "Download", "start", 2);
    if let Err(error) = storage.download(name, tmp_dir) {
        cleanup(tmp_dir);
        events::emit(
            "error",
            vec![
                (
                    "message",
                    Value::String(format!("Download failed: {error}")),
                ),
                (
                    "hint",
                    Value::String("Check storage access and try again.".to_string()),
                ),
                ("exit_code", Value::from(1)),
            ],
        );
        return Err(error);
    }
    emit_stage("download", "Download", "complete", 2);
    Ok(())
}

fn verify_checksums(tmp_dir: &Path, files: &Map<String, Value>) -> Result<(), CoreError> {
    emit_stage("verify", "Verify", "start", 3);
    for (index, info) in files.values().enumerate() {
        let archive_name = info.get("archive").map_or_default(text::py_str);
        if manifest::clean_backup_name(&archive_name) != archive_name {
            cleanup(tmp_dir);
            events::emit(
                "error",
                vec![
                    (
                        "message",
                        Value::String("Checksum verification failed".to_string()),
                    ),
                    (
                        "hint",
                        Value::String(
                            "Restore was stopped before extraction. The backup may be corrupt."
                                .to_string(),
                        ),
                    ),
                    ("exit_code", Value::from(1)),
                ],
            );
            return Err(CoreError::user("Checksum verification failed"));
        }
        let archive_path = tmp_dir.join(&archive_name);
        let expected = info.get("checksum").map_or_default(text::py_str);
        let actual = storage::sha256_file(&archive_path).unwrap_or_default();
        if !archive_path.exists() || expected.is_empty() || actual != expected {
            cleanup(tmp_dir);
            events::emit(
                "verify",
                vec![
                    ("state", Value::String("failed".to_string())),
                    (
                        "details",
                        Value::String("checksum verification failed".to_string()),
                    ),
                ],
            );
            return Err(CoreError::user("Checksum verification failed"));
        }
        events::emit(
            "progress",
            vec![
                ("stage", Value::String("verify".to_string())),
                ("current", Value::from(index + 1)),
                ("total", Value::from(files.len())),
                ("unit", Value::String("items".to_string())),
                ("detail", Value::String(archive_name)),
            ],
        );
    }
    events::emit(
        "verify",
        vec![
            ("state", Value::String("verified".to_string())),
            (
                "details",
                Value::String("all archive checksums match".to_string()),
            ),
        ],
    );
    emit_stage("verify", "Verify", "complete", 3);
    Ok(())
}

fn check_extract_space(
    files: &Map<String, Value>,
    roots: &HashMap<&'static str, (PathBuf, String)>,
    needed: u64,
    tmp_dir: &Path,
) -> Result<(), CoreError> {
    let mut checked_roots: Vec<&Path> = Vec::new();
    for key in files.keys() {
        let Some((root, _)) = roots.get(key.as_str()) else {
            continue;
        };
        if checked_roots.contains(&root.as_path()) {
            continue;
        }
        if let Err(error) = ensure_extract_space(root, needed) {
            cleanup(tmp_dir);
            events::emit(
                "error",
                vec![
                    ("message", Value::String(error.to_string())),
                    (
                        "hint",
                        Value::String("Free some space and try again.".to_string()),
                    ),
                    ("exit_code", Value::from(1)),
                ],
            );
            return Err(error);
        }
        checked_roots.push(root.as_path());
    }
    Ok(())
}

fn extract_archives(
    files: &Map<String, Value>,
    roots: &HashMap<&'static str, (PathBuf, String)>,
    tmp_dir: &Path,
) -> Vec<String> {
    let mut failed_archives = Vec::new();
    for (key, info) in files {
        let stage_id = format!("extract:{key}");
        let (stage_label, stage_index) = extract_stage(key);
        emit_stage(&stage_id, stage_label, "start", stage_index);
        let archive_name = info.get("archive").map_or_default(text::py_str);
        if manifest::clean_backup_name(&archive_name) != archive_name {
            failed_archives.push(archive_name.clone());
            events::emit(
                "log",
                vec![
                    ("level", Value::String("ERROR".to_string())),
                    (
                        "message",
                        Value::String(format!("Invalid archive name in manifest: {archive_name}")),
                    ),
                ],
            );
            emit_stage(&stage_id, stage_label, "failed", stage_index);
            continue;
        }
        let Some((root, top)) = roots.get(key.as_str()) else {
            failed_archives.push(archive_name);
            logging::log(
                "WARNING",
                &format!("Refused restore for unknown manifest key: {key:?}"),
            );
            continue;
        };
        let mut count = 0usize;
        let mut throttle = EventProgress::new(stage_id.clone());
        let mut on_member = |path: &Path| {
            count += 1;
            throttle.emit(
                count as u64,
                None,
                "items",
                Some(&path.to_string_lossy()),
                false,
            );
        };
        let budget = info.get("size").and_then(fmt::py_int).unwrap_or(0).max(0) as u64;
        match archive::extract_archive(
            &tmp_dir.join(&archive_name),
            root,
            Some(top),
            Some(budget + 64 * 1024 * 1024),
            &mut on_member,
        ) {
            Ok(result) => {
                let extracted_none = result.extracted == 0;
                let fatal = extracted_none
                    || result
                        .warnings
                        .iter()
                        .any(|warning| !archive::is_ignorable_warning(warning));
                for warning in result.warnings {
                    events::emit(
                        "log",
                        vec![
                            ("level", Value::String("WARNING".to_string())),
                            ("message", Value::String(warning)),
                        ],
                    );
                }
                if fatal {
                    failed_archives.push(archive_name.clone());
                    let message = if extracted_none {
                        format!("Archive extracted no entries and may be corrupt: {archive_name}")
                    } else {
                        format!("Archive extracted with errors: {archive_name}")
                    };
                    events::emit(
                        "log",
                        vec![
                            ("level", Value::String("ERROR".to_string())),
                            ("message", Value::String(message)),
                        ],
                    );
                    emit_stage(&stage_id, stage_label, "failed", stage_index);
                } else {
                    emit_stage(&stage_id, stage_label, "complete", stage_index);
                }
            }
            Err(error) => {
                failed_archives.push(archive_name.clone());
                events::emit(
                    "log",
                    vec![
                        ("level", Value::String("ERROR".to_string())),
                        (
                            "message",
                            Value::String(format!("Extract failed for {archive_name}: {error}")),
                        ),
                    ],
                );
                emit_stage(&stage_id, stage_label, "failed", stage_index);
            }
        }
    }
    failed_archives
}

fn install_restore_packages(backup_packages: &[String]) -> Vec<String> {
    if backup_packages.is_empty() {
        emit_stage("packages", "Packages", "skipped", 6);
        return Vec::new();
    }
    emit_stage("packages", "Packages", "start", 6);
    let failures = packages::install(backup_packages);
    emit_stage("packages", "Packages", "complete", 6);
    failures
}

fn run_cleanup_stage(tmp_dir: &Path) {
    emit_stage("cleanup", "Cleanup", "start", 7);
    cleanup(tmp_dir);
    emit_stage("cleanup", "Cleanup", "complete", 7);
}

fn report_restore_failure(
    name: &str,
    failed_archives: &[String],
    tmp_dir: &Path,
) -> Result<Value, CoreError> {
    events::emit(
        "result",
        vec![
            ("ok", Value::Bool(false)),
            ("name", Value::String(name.to_string())),
            ("archives_failed", Value::from(failed_archives.to_vec())),
            ("warnings", Value::Bool(true)),
        ],
    );
    cleanup(tmp_dir);
    Err(CoreError::runtime(format!(
        "Restore incomplete; failed archives: {}",
        failed_archives.join(", ")
    )))
}

struct RestoreReport {
    name: String,
    size: i64,
    packages: usize,
    package_failures: Vec<String>,
    archives: usize,
    failed_archives: Vec<String>,
    started: Instant,
}

fn report_restore(report: RestoreReport) -> Result<Value, CoreError> {
    let warnings = !report.package_failures.is_empty() || !report.failed_archives.is_empty();
    events::emit(
        "result",
        vec![
            ("ok", Value::Bool(true)),
            ("name", Value::String(report.name)),
            ("size", Value::from(report.size)),
            ("packages", Value::from(report.packages)),
            (
                "packages_installed",
                Value::from(report.packages - report.package_failures.len()),
            ),
            (
                "packages_failed",
                Value::Array(
                    report
                        .package_failures
                        .into_iter()
                        .map(Value::String)
                        .collect(),
                ),
            ),
            ("archives", Value::from(report.archives)),
            (
                "archives_failed",
                Value::Array(
                    report
                        .failed_archives
                        .into_iter()
                        .map(Value::String)
                        .collect(),
                ),
            ),
            ("warnings", Value::Bool(warnings)),
            ("restart_required", Value::Bool(true)),
            (
                "duration_ms",
                Value::from(report.started.elapsed().as_millis() as i64),
            ),
        ],
    );
    Ok(Value::Null)
}

/// Download and extract `backup_name`, then reinstall its recorded packages.
/// Returns the restore summary as JSON.
pub fn restore(cfg: &Value, backup_name: &str) -> Result<Value, CoreError> {
    let started = Instant::now();
    let name = manifest::clean_backup_name(backup_name);
    if name.is_empty() || name != text::py_strip(backup_name) {
        return Err(CoreError::user("Invalid backup name"));
    }
    let storage = Storage::from_config(cfg)?;
    let tmp_dir = restore_work_dir(&name)?;

    emit_start("restore", &name, cfg);

    let manifest = load_restore_manifest(&storage, &name, &tmp_dir)?;
    let backup_packages: Vec<String> = manifest
        .get("packages")
        .and_then(Value::as_array)
        .map_or_default(|items| items.iter().map(text::py_str).collect());
    let empty_files = Map::new();
    let files = manifest
        .get("files")
        .and_then(Value::as_object)
        .unwrap_or(&empty_files);
    let archives: Vec<String> = files
        .values()
        .filter_map(|info| info.get("archive"))
        .map(text::py_str)
        .collect();
    let archive_count = archives.len();
    let size = files_total_size(files);
    {
        let plan = RestorePlan {
            name: &name,
            manifest: &manifest,
            backup_packages: &backup_packages,
            archives,
            size,
            diff: package_diff(&backup_packages),
        };
        plan.emit(cfg);
    }

    if !confirm_restore() {
        cleanup(&tmp_dir);
        events::emit(
            "cancelled",
            vec![("reason", Value::String("declined".to_string()))],
        );
        return Ok(Value::Null);
    }

    download_backup(&storage, &name, &tmp_dir)?;
    verify_checksums(&tmp_dir, files)?;

    let roots = restore_roots();
    check_extract_space(files, &roots, size.max(0) as u64, &tmp_dir)?;
    let failed_archives = extract_archives(files, &roots, &tmp_dir);
    if !failed_archives.is_empty() {
        return report_restore_failure(&name, &failed_archives, &tmp_dir);
    }

    let package_failures = install_restore_packages(&backup_packages);
    run_cleanup_stage(&tmp_dir);

    report_restore(RestoreReport {
        name,
        size,
        packages: backup_packages.len(),
        package_failures,
        archives: archive_count,
        failed_archives,
        started,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backup_name_appends_the_cleaned_label() {
        assert_eq!(
            build_backup_name("20250101_120000", ""),
            "termux_backup_20250101_120000"
        );
        assert_eq!(
            build_backup_name("20250101_120000", "before reset"),
            "termux_backup_20250101_120000_before_reset"
        );
        assert_eq!(
            build_backup_name("20250101_120000", "!!!"),
            "termux_backup_20250101_120000"
        );
        assert_eq!(
            build_backup_name("20250101_120000", "../a/b"),
            "termux_backup_20250101_120000_.._a_b"
        );
    }

    #[test]
    fn manifest_size_sums_numeric_file_sizes() {
        let manifest = json!({
            "files": {
                "home": {"archive": "home.tar.gz", "size": 7},
                "usr_etc": {"archive": "usr_etc.tar.gz", "size": 11},
                "odd": {"archive": "odd.tar.gz", "size": "not a number"},
            }
        });
        assert_eq!(manifest_total_size(&manifest), 18);
        assert_eq!(manifest_total_size(&json!({})), 0);
        assert_eq!(manifest_total_size(&json!({"files": 3})), 0);
    }

    #[test]
    fn package_diff_classifies_each_category() {
        let current: BTreeSet<String> = ["a", "b", "c"].into_iter().map(str::to_string).collect();
        let backup = vec!["b".to_string(), "c".to_string(), "d".to_string()];
        let diff = package_diff_with(&current, &backup);
        assert!(diff.available);
        assert_eq!(diff.to_install, vec!["d".to_string()]);
        assert_eq!(diff.installed, vec!["b".to_string(), "c".to_string()]);
        assert_eq!(diff.device_only, vec!["a".to_string()]);
    }

    #[test]
    fn package_diff_is_unavailable_without_current_packages() {
        let diff = package_diff_with(&BTreeSet::new(), &["a".to_string()]);
        assert!(!diff.available);
        assert_eq!(diff.to_install, [] as [std::string::String; 0]);
        assert_eq!(diff.installed, [] as [std::string::String; 0]);
        assert_eq!(diff.device_only, [] as [std::string::String; 0]);
    }

    #[test]
    fn stage_fields_keep_the_event_protocol_order() {
        let fields = stage_fields("preflight", "Preflight", "start", 1);
        let keys: Vec<&str> = fields.iter().map(|(key, _)| *key).collect();
        assert_eq!(keys, vec!["id", "label", "state", "index", "total"]);
        assert_eq!(fields[0].1, Value::String("preflight".to_string()));
        assert_eq!(fields[1].1, Value::String("Preflight".to_string()));
        assert_eq!(fields[2].1, Value::String("start".to_string()));
        assert_eq!(fields[3].1, Value::from(1));
        assert_eq!(fields[4].1, Value::from(TOTAL_STAGES));
    }

    #[test]
    fn extract_stage_labels_follow_manifest_keys() {
        assert_eq!(extract_stage("home"), ("Home", 4));
        assert_eq!(extract_stage("usr_etc"), ("Config", 5));
        assert_eq!(extract_stage("other"), ("other", 4));
    }
}
