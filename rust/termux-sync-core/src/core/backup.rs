//! Backup and restore orchestration for the local backend.

use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Instant;

use chrono::Local;
use serde_json::{Map, Value, json};

use crate::core::archive::{self, ArchiveOptions};
use crate::core::config;
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
        .map(|items| items.iter().map(text::py_str).collect())
        .unwrap_or_default()
}

fn cleanup(path: &Path) {
    let _ = fs::remove_dir_all(path);
}

fn prepare_work_dir(path: &Path) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| format!("OSError: {error}"))?;
    }
    fs::create_dir(path).map_err(|error| format!("OSError: {error}"))?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|error| format!("OSError: {error}"))
}

pub fn get_storage(cfg: &Value) -> Result<Value, String> {
    let storage = Storage::from_config(cfg)?;
    Ok(json!({ "kind": storage.kind() }))
}

pub fn list(cfg: &Value) -> Result<Value, String> {
    let storage = Storage::from_config(cfg)?;
    render::print_section(
        "BACKUP LIST",
        &format!(
            "Storage: {}",
            cfg_string(cfg, "storage", "local").to_uppercase()
        ),
    );
    let backups = storage.list_backups();
    let backups: Vec<Value> = backups
        .into_iter()
        .filter(|backup| manifest::manifest_shape(backup, false))
        .collect();
    if events::enabled() {
        for backup in &backups {
            let files = backup.get("files").and_then(Value::as_object);
            let size: i64 = files
                .map(|files| {
                    files
                        .values()
                        .filter_map(|info| info.get("size"))
                        .filter_map(fmt::py_int)
                        .sum()
                })
                .unwrap_or(0);
            events::emit(
                "backup",
                vec![
                    (
                        "name",
                        Value::String(backup.get("name").map(text::py_str).unwrap_or_default()),
                    ),
                    (
                        "date",
                        Value::String(backup.get("date").map(text::py_str).unwrap_or_default()),
                    ),
                    (
                        "label",
                        Value::String(backup.get("label").map(text::py_str).unwrap_or_default()),
                    ),
                    (
                        "packages",
                        Value::from(
                            backup
                                .get("packages")
                                .and_then(Value::as_array)
                                .map(Vec::len)
                                .unwrap_or(0),
                        ),
                    ),
                    ("size", Value::from(size)),
                    (
                        "version",
                        Value::String(backup.get("version").map(text::py_str).unwrap_or_default()),
                    ),
                    (
                        "compression",
                        Value::String(
                            backup
                                .get("compression")
                                .map(text::py_str)
                                .unwrap_or_default(),
                        ),
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
    if backups.is_empty() {
        render::print_line("  No backups found.");
        return Ok(Value::Null);
    }
    render::print_line(&format!("{} Backup(s) Found", backups.len()));
    for (index, backup) in backups.iter().enumerate() {
        render::print_line(&format!("Backup #{}", index + 1));
        render::print_line(&format!(
            "Name {}",
            backup
                .get("name")
                .map(text::py_str)
                .unwrap_or_else(|| "—".to_string())
        ));
        render::print_line(&format!(
            "Date {}",
            backup
                .get("date")
                .map(text::py_str)
                .unwrap_or_default()
                .chars()
                .take(19)
                .collect::<String>()
                .replace('T', " ")
        ));
        render::print_line(&format!(
            "Label {}",
            backup
                .get("label")
                .map(text::py_str)
                .filter(|label| !label.is_empty())
                .unwrap_or_else(|| "—".to_string())
        ));
        render::print_line(&format!(
            "Packages {}",
            backup
                .get("packages")
                .and_then(Value::as_array)
                .map(Vec::len)
                .unwrap_or(0)
        ));
        render::print_line(&format!(
            "Files {}",
            backup
                .get("files")
                .and_then(Value::as_object)
                .map(Map::len)
                .unwrap_or(0)
        ));
        render::print_line(&format!(
            "Version {}",
            backup
                .get("version")
                .map(text::py_str)
                .unwrap_or_else(|| "—".to_string())
        ));
        render::print_line(&format!(
            "Compression {}",
            backup
                .get("compression")
                .map(text::py_str)
                .unwrap_or_else(|| "—".to_string())
                .to_uppercase()
        ));
        let size: i64 = backup
            .get("files")
            .and_then(Value::as_object)
            .map(|files| {
                files
                    .values()
                    .filter_map(|info| info.get("size"))
                    .filter_map(fmt::py_int)
                    .sum()
            })
            .unwrap_or(0);
        if size != 0 {
            render::print_line(&format!("Total size {}", fmt::human_size(size)));
        }
    }
    Ok(Value::Null)
}

pub fn backup(cfg: &Value, label: &str) -> Result<Value, String> {
    let started = Instant::now();
    render::print_section(
        "BACKUP",
        "Creating a full snapshot of your Termux environment",
    );

    let timestamp = Local::now().format("%Y%m%d_%H%M%S").to_string();
    let mut backup_name = format!("termux_backup_{timestamp}");
    let clean_label = if label.is_empty() {
        String::new()
    } else {
        manifest::clean_label(label)
    };
    if !clean_label.is_empty() {
        backup_name.push('_');
        backup_name.push_str(&clean_label);
    }

    events::emit(
        "start",
        vec![
            ("command", Value::String("backup".to_string())),
            ("name", Value::String(backup_name.clone())),
            (
                "storage",
                Value::String(cfg_string(cfg, "storage", "local")),
            ),
            ("version", Value::String(paths::VERSION.to_string())),
        ],
    );

    let tmp_dir = paths::tmp_root().join(&backup_name);
    prepare_work_dir(&tmp_dir)?;

    let mut compression = cfg_string(cfg, "compression", "gz");
    if !matches!(compression.as_str(), "gz" | "bz2" | "xz") {
        render::print_line(&format!(
            "  Warning: compression '{compression}' is invalid, falling back to 'gz'."
        ));
        compression = "gz".to_string();
    }
    let excludes = excludes(cfg);
    let mut storage = match Storage::from_config(cfg) {
        Ok(storage) => storage,
        Err(error) => {
            cleanup(&tmp_dir);
            return Err(error);
        }
    };
    if let Err(error) = storage.preflight() {
        cleanup(&tmp_dir);
        return Err(error);
    }
    if let Storage::GitHub(github) = &mut storage
        && let Err(error) = github.begin_stream(&backup_name, &tmp_dir.join(".parts"))
    {
        cleanup(&tmp_dir);
        return Err(error);
    }

    events::emit(
        "stage",
        vec![
            ("id", Value::String("preflight".to_string())),
            ("label", Value::String("Preflight".to_string())),
            ("state", Value::String("start".to_string())),
            ("index", Value::from(1)),
            ("total", Value::from(TOTAL_STAGES)),
        ],
    );
    events::emit(
        "stage",
        vec![
            ("id", Value::String("preflight".to_string())),
            ("label", Value::String("Preflight".to_string())),
            ("state", Value::String("complete".to_string())),
            ("index", Value::from(1)),
            ("total", Value::from(TOTAL_STAGES)),
        ],
    );

    let mut meta = Map::new();
    meta.insert("name".to_string(), Value::String(backup_name.clone()));
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
        Value::String(compression.clone()),
    );
    meta.insert(
        "version".to_string(),
        Value::String(paths::VERSION.to_string()),
    );
    meta.insert("files".to_string(), Value::Object(Map::new()));
    meta.insert("packages".to_string(), Value::Array(Vec::new()));

    render::print_line("  Step 1/4  Collecting installed packages…");
    events::emit(
        "stage",
        vec![
            ("id", Value::String("packages".to_string())),
            ("label", Value::String("Packages".to_string())),
            ("state", Value::String("start".to_string())),
            ("index", Value::from(2)),
            ("total", Value::from(TOTAL_STAGES)),
        ],
    );
    let installed = match packages::installed_packages() {
        Ok(list) => list,
        Err(error) => {
            cleanup(&tmp_dir);
            return Err(error);
        }
    };
    meta.insert(
        "packages".to_string(),
        Value::Array(installed.iter().cloned().map(Value::String).collect()),
    );
    fs::write(tmp_dir.join("packages.txt"), installed.join("\n"))
        .map_err(|error| format!("OSError: {error}"))?;
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
    events::emit(
        "stage",
        vec![
            ("id", Value::String("packages".to_string())),
            ("label", Value::String("Packages".to_string())),
            ("state", Value::String("complete".to_string())),
            ("index", Value::from(2)),
            ("total", Value::from(TOTAL_STAGES)),
        ],
    );

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
        let stage_id = format!("archive:{key}");
        events::emit(
            "stage",
            vec![
                ("id", Value::String(stage_id.clone())),
                ("label", Value::String(label_text.to_string())),
                ("state", Value::String("start".to_string())),
                ("index", Value::from(stage_index)),
                ("total", Value::from(TOTAL_STAGES)),
            ],
        );
        if !source.exists() {
            render::print_line(&format!(
                "  Warning: skipping {} (not found)",
                source.display()
            ));
            events::emit(
                "stage",
                vec![
                    ("id", Value::String(stage_id)),
                    ("label", Value::String(label_text.to_string())),
                    ("state", Value::String("skipped".to_string())),
                    ("index", Value::from(stage_index)),
                    ("total", Value::from(TOTAL_STAGES)),
                ],
            );
            continue;
        }
        let out_name = format!("{key}.tar.{compression}");
        let out_path = tmp_dir.join(&out_name);
        let mut item_excludes = excludes.clone();
        item_excludes.extend(manifest::storage_excludes(cfg, &source));
        item_excludes.extend(manifest::self_excludes(&source));
        let options = ArchiveOptions {
            compression: compression.clone(),
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

        let archive_result: Result<(String, u64, String), String> = if injected_failure {
            Err("injected archive failure".to_string())
        } else if let Storage::GitHub(github) = &storage {
            match github.stream_sink(&out_name) {
                Ok(sink) => {
                    match archive::create_archive_to_writer(&source, sink, &options, &mut on_member)
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
            archive::create_archive(&source, &out_path, &options, &mut on_member).and_then(
                |result| {
                    let name = result
                        .path
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    let checksum = storage::sha256_file(&result.path)?;
                    Ok((name, result.size, checksum))
                },
            )
        };
        match archive_result {
            Ok((name, archive_size, checksum)) => {
                if let Some(Value::Object(files)) = meta.get_mut("files") {
                    files.insert(
                        key.to_string(),
                        json!({
                            "archive": name,
                            "size": archive_size,
                            "checksum": checksum,
                            "source": source.to_string_lossy(),
                        }),
                    );
                }
                events::emit(
                    "stage",
                    vec![
                        ("id", Value::String(stage_id.clone())),
                        ("label", Value::String(label_text.to_string())),
                        ("state", Value::String("complete".to_string())),
                        ("index", Value::from(stage_index)),
                        ("total", Value::from(TOTAL_STAGES)),
                    ],
                );
                events::emit(
                    "progress",
                    vec![
                        ("stage", Value::String(stage_id.clone())),
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
            }
            Err(error) => {
                events::emit(
                    "stage",
                    vec![
                        ("id", Value::String(stage_id)),
                        ("label", Value::String(label_text.to_string())),
                        ("state", Value::String("failed".to_string())),
                        ("index", Value::from(stage_index)),
                        ("total", Value::from(TOTAL_STAGES)),
                    ],
                );
                render::print_line(&format!(
                    "  ✗ Failed to archive {}: {error}",
                    source.display()
                ));
                logging::log(
                    "ERROR",
                    &format!("Archive failed for {}: {error}", source.display()),
                );
                if let Storage::GitHub(github) = &storage {
                    github.abort_stream(&backup_name);
                }
                cleanup(&tmp_dir);
                return Err(error);
            }
        }
    }

    render::print_line("");
    render::print_line("  Step 3/4  Writing manifest…");
    events::emit(
        "stage",
        vec![
            ("id", Value::String("manifest".to_string())),
            ("label", Value::String("Manifest".to_string())),
            ("state", Value::String("start".to_string())),
            ("index", Value::from(5)),
            ("total", Value::from(TOTAL_STAGES)),
        ],
    );
    let meta_value = Value::Object(meta);
    crate::core::json::write_json_pretty_ascii(&tmp_dir.join("manifest.json"), &meta_value)?;
    render::print_line("  ✓ Manifest written");
    events::emit(
        "stage",
        vec![
            ("id", Value::String("manifest".to_string())),
            ("label", Value::String("Manifest".to_string())),
            ("state", Value::String("complete".to_string())),
            ("index", Value::from(5)),
            ("total", Value::from(TOTAL_STAGES)),
        ],
    );

    render::print_line("");
    render::print_line(&format!(
        "  Step 4/4  Uploading to {}…",
        cfg_string(cfg, "storage", "local").to_uppercase()
    ));
    render::print_line("");
    events::emit(
        "stage",
        vec![
            ("id", Value::String("upload".to_string())),
            ("label", Value::String("Upload".to_string())),
            ("state", Value::String("start".to_string())),
            ("index", Value::from(6)),
            ("total", Value::from(TOTAL_STAGES)),
        ],
    );
    let uploaded = match storage.upload(&tmp_dir, &backup_name) {
        Ok(uploaded) => uploaded,
        Err(error) => {
            events::emit(
                "stage",
                vec![
                    ("id", Value::String("upload".to_string())),
                    ("label", Value::String("Upload".to_string())),
                    ("state", Value::String("failed".to_string())),
                    ("index", Value::from(6)),
                    ("total", Value::from(TOTAL_STAGES)),
                ],
            );
            logging::log("ERROR", &format!("Upload failed: {error}"));
            if let Storage::GitHub(github) = &storage {
                github.abort_stream(&backup_name);
            }
            return Err(error);
        }
    };
    let location = uploaded.location;

    events::emit(
        "stage",
        vec![
            ("id", Value::String("upload".to_string())),
            ("label", Value::String("Upload".to_string())),
            ("state", Value::String("complete".to_string())),
            ("index", Value::from(6)),
            ("total", Value::from(TOTAL_STAGES)),
        ],
    );

    events::emit(
        "stage",
        vec![
            ("id", Value::String("verify".to_string())),
            ("label", Value::String("Verify".to_string())),
            ("state", Value::String("start".to_string())),
            ("index", Value::from(7)),
            ("total", Value::from(TOTAL_STAGES)),
        ],
    );
    let files = meta_value
        .get("files")
        .cloned()
        .unwrap_or(Value::Object(Map::new()));
    let (verified, verify_details): (Option<bool>, String) = match &storage {
        Storage::GitHub(_) => (Some(true), "stream digest".to_string()),
        Storage::Local(_) => {
            let (ok, details) = storage::verify_local_backup(Path::new(&location), &files);
            (Some(ok), details)
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
            ("details", Value::String(verify_details.clone())),
        ],
    );
    events::emit(
        "stage",
        vec![
            ("id", Value::String("verify".to_string())),
            ("label", Value::String("Verify".to_string())),
            (
                "state",
                Value::String(
                    if verified != Some(false) {
                        "complete"
                    } else {
                        "failed"
                    }
                    .to_string(),
                ),
            ),
            ("index", Value::from(7)),
            ("total", Value::from(TOTAL_STAGES)),
        ],
    );

    if verified != Some(false) {
        storage.delete_old(config::retention_limit(cfg))?;
    }

    cleanup(&tmp_dir);

    let total_size: i64 = files
        .as_object()
        .map(|files| {
            files
                .values()
                .filter_map(|info| info.get("size"))
                .filter_map(fmt::py_int)
                .sum()
        })
        .unwrap_or(0);
    render::print_line("");
    render::print_line("  ✓ Backup completed successfully!");
    render::print_line("");
    render::print_line(&format!("  Name      {backup_name}"));
    render::print_line(&format!(
        "  Date      {}",
        meta_value
            .get("date")
            .map(text::py_str)
            .unwrap_or_default()
            .chars()
            .take(19)
            .collect::<String>()
            .replace('T', " ")
    ));
    render::print_line(&format!("  Packages  {}", installed.len()));
    render::print_line(&format!("  Size      {}", fmt::human_size(total_size)));
    render::print_line(&format!(
        "  Storage   {}",
        cfg_string(cfg, "storage", "local").to_uppercase()
    ));
    render::print_line(&format!("  Location  {}", fmt::short_path(&location)));
    render::print_line("");
    logging::log(
        "INFO",
        &format!(
            "Backup completed: {backup_name} ({})",
            fmt::human_size(total_size)
        ),
    );
    events::emit(
        "result",
        vec![
            ("ok", Value::Bool(true)),
            ("name", Value::String(backup_name)),
            ("size", Value::from(total_size)),
            ("location", Value::String(location)),
            ("verified", verified.map(Value::Bool).unwrap_or(Value::Null)),
            ("verify_details", Value::String(verify_details)),
            (
                "duration_ms",
                Value::from(started.elapsed().as_millis() as i64),
            ),
        ],
    );
    Ok(Value::Null)
}

fn package_diff(backup_packages: &[String]) -> (bool, Vec<String>, Vec<String>, Vec<String>) {
    let current: BTreeSet<String> = packages::installed_packages()
        .unwrap_or_default()
        .into_iter()
        .collect();
    if current.is_empty() {
        return (false, Vec::new(), Vec::new(), Vec::new());
    }
    let backup: BTreeSet<String> = backup_packages.iter().cloned().collect();
    let to_install: Vec<String> = backup.difference(&current).cloned().collect();
    let installed: Vec<String> = backup.intersection(&current).cloned().collect();
    let device_only: Vec<String> = current.difference(&backup).cloned().collect();
    (true, to_install, installed, device_only)
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
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("/"));
        let top = root
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        roots.insert(key, (paths::resolve_lenient(&parent), top));
    }
    roots
}

pub fn restore(cfg: &Value, backup_name: &str) -> Result<Value, String> {
    let started = Instant::now();
    let name = manifest::clean_backup_name(backup_name);
    if name.is_empty() || name != text::py_strip(backup_name) {
        return Err("termux_sync.UserError: Invalid backup name".to_string());
    }
    let storage = Storage::from_config(cfg)?;
    let tmp_dir = paths::tmp_root().join(format!("restore_{name}"));
    fs::create_dir_all(&tmp_dir).map_err(|error| format!("OSError: {error}"))?;

    events::emit(
        "start",
        vec![
            ("command", Value::String("restore".to_string())),
            ("name", Value::String(name.clone())),
            (
                "storage",
                Value::String(cfg_string(cfg, "storage", "local")),
            ),
            ("version", Value::String(paths::VERSION.to_string())),
        ],
    );

    events::emit(
        "stage",
        vec![
            ("id", Value::String("metadata".to_string())),
            ("label", Value::String("Metadata".to_string())),
            ("state", Value::String("start".to_string())),
            ("index", Value::from(1)),
            ("total", Value::from(TOTAL_STAGES)),
        ],
    );

    let manifest = storage.fetch_manifest(&name);
    let manifest = match manifest {
        Ok(Some(manifest)) => manifest,
        Ok(None) => {
            cleanup(&tmp_dir);
            return Err("termux_sync.UserError: Could not read manifest".to_string());
        }
        Err(error) => {
            cleanup(&tmp_dir);
            return Err(error);
        }
    };
    if !manifest::manifest_shape(&manifest, true) {
        cleanup(&tmp_dir);
        return Err("termux_sync.UserError: Backup manifest is malformed".to_string());
    }
    events::emit(
        "stage",
        vec![
            ("id", Value::String("metadata".to_string())),
            ("label", Value::String("Metadata".to_string())),
            ("state", Value::String("complete".to_string())),
            ("index", Value::from(1)),
            ("total", Value::from(TOTAL_STAGES)),
        ],
    );

    let backup_packages: Vec<String> = manifest
        .get("packages")
        .and_then(Value::as_array)
        .map(|items| items.iter().map(text::py_str).collect())
        .unwrap_or_default();
    let (diff_ok, to_install, already_installed, device_only) = package_diff(&backup_packages);
    let files = manifest
        .get("files")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let archives: Vec<String> = files
        .values()
        .filter_map(|info| info.get("archive"))
        .map(text::py_str)
        .collect();
    let size: i64 = files
        .values()
        .filter_map(|info| info.get("size"))
        .filter_map(fmt::py_int)
        .sum();

    let mut plan: Vec<(&str, Value)> = vec![
        ("name", Value::String(name.clone())),
        (
            "date",
            Value::String(manifest.get("date").map(text::py_str).unwrap_or_default()),
        ),
        (
            "label",
            Value::String(manifest.get("label").map(text::py_str).unwrap_or_default()),
        ),
        (
            "storage",
            Value::String(cfg_string(cfg, "storage", "local")),
        ),
        (
            "versions",
            json!({
                "backup": manifest.get("version").map(text::py_str).unwrap_or_else(|| "—".to_string()),
                "current": paths::VERSION,
            }),
        ),
        ("packages", Value::from(backup_packages.len() as i64)),
        ("diff_available", Value::Bool(diff_ok)),
        (
            "archives",
            Value::Array(archives.iter().cloned().map(Value::String).collect()),
        ),
        ("size", Value::from(size)),
        ("checksums", Value::String("pending".to_string())),
    ];
    if diff_ok {
        plan.push(("to_install", Value::from(to_install.len() as i64)));
        plan.push((
            "already_installed",
            Value::from(already_installed.len() as i64),
        ));
        plan.push(("device_only", Value::from(device_only.len() as i64)));
    }
    events::emit("plan", plan);
    if diff_ok {
        emit_package_diff("to_install", &to_install);
        emit_package_diff("already_installed", &already_installed);
        emit_package_diff("device_only", &device_only);
    }

    let confirmed = if events::enabled() {
        events::confirmation()
    } else {
        render::confirm("Proceed with restore?", false)
    };
    if !confirmed {
        cleanup(&tmp_dir);
        events::emit(
            "cancelled",
            vec![("reason", Value::String("declined".to_string()))],
        );
        return Ok(Value::Null);
    }

    events::emit(
        "stage",
        vec![
            ("id", Value::String("download".to_string())),
            ("label", Value::String("Download".to_string())),
            ("state", Value::String("start".to_string())),
            ("index", Value::from(2)),
            ("total", Value::from(TOTAL_STAGES)),
        ],
    );
    if let Err(error) = storage.download(&name, &tmp_dir) {
        cleanup(&tmp_dir);
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
    events::emit(
        "stage",
        vec![
            ("id", Value::String("download".to_string())),
            ("label", Value::String("Download".to_string())),
            ("state", Value::String("complete".to_string())),
            ("index", Value::from(2)),
            ("total", Value::from(TOTAL_STAGES)),
        ],
    );

    events::emit(
        "stage",
        vec![
            ("id", Value::String("verify".to_string())),
            ("label", Value::String("Verify".to_string())),
            ("state", Value::String("start".to_string())),
            ("index", Value::from(3)),
            ("total", Value::from(TOTAL_STAGES)),
        ],
    );
    for (index, (key, info)) in files.iter().enumerate() {
        let archive_name = info.get("archive").map(text::py_str).unwrap_or_default();
        if manifest::clean_backup_name(&archive_name) != archive_name {
            cleanup(&tmp_dir);
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
            return Err("termux_sync.UserError: Checksum verification failed".to_string());
        }
        let archive_path = tmp_dir.join(&archive_name);
        let expected = info.get("checksum").map(text::py_str).unwrap_or_default();
        let actual = storage::sha256_file(&archive_path).unwrap_or_default();
        if !archive_path.exists() || expected.is_empty() || actual != expected {
            cleanup(&tmp_dir);
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
            return Err("termux_sync.UserError: Checksum verification failed".to_string());
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
        let _ = key;
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
    events::emit(
        "stage",
        vec![
            ("id", Value::String("verify".to_string())),
            ("label", Value::String("Verify".to_string())),
            ("state", Value::String("complete".to_string())),
            ("index", Value::from(3)),
            ("total", Value::from(TOTAL_STAGES)),
        ],
    );

    let roots = restore_roots();
    let mut failed_archives = Vec::new();
    for (key, info) in &files {
        let stage_id = format!("extract:{key}");
        let stage_label = if key == "home" {
            "Home"
        } else if key == "usr_etc" {
            "Config"
        } else {
            key
        };
        let stage_index = if key == "home" {
            4
        } else if key == "usr_etc" {
            5
        } else {
            4
        };
        events::emit(
            "stage",
            vec![
                ("id", Value::String(stage_id.clone())),
                ("label", Value::String(stage_label.to_string())),
                ("state", Value::String("start".to_string())),
                ("index", Value::from(stage_index)),
                ("total", Value::from(TOTAL_STAGES)),
            ],
        );
        let archive_name = info.get("archive").map(text::py_str).unwrap_or_default();
        if manifest::clean_backup_name(&archive_name) != archive_name {
            failed_archives.push(archive_name);
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
        match archive::extract_archive(
            &tmp_dir.join(&archive_name),
            root,
            Some(top),
            None,
            &mut on_member,
        ) {
            Ok(result) => {
                for warning in result.warnings {
                    events::emit(
                        "log",
                        vec![
                            ("level", Value::String("WARNING".to_string())),
                            ("message", Value::String(warning)),
                        ],
                    );
                }
                events::emit(
                    "stage",
                    vec![
                        ("id", Value::String(stage_id)),
                        ("label", Value::String(stage_label.to_string())),
                        ("state", Value::String("complete".to_string())),
                        ("index", Value::from(stage_index)),
                        ("total", Value::from(TOTAL_STAGES)),
                    ],
                );
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
                events::emit(
                    "stage",
                    vec![
                        ("id", Value::String(stage_id)),
                        ("label", Value::String(stage_label.to_string())),
                        ("state", Value::String("failed".to_string())),
                        ("index", Value::from(stage_index)),
                        ("total", Value::from(TOTAL_STAGES)),
                    ],
                );
            }
        }
    }

    let package_failures = if backup_packages.is_empty() {
        events::emit(
            "stage",
            vec![
                ("id", Value::String("packages".to_string())),
                ("label", Value::String("Packages".to_string())),
                ("state", Value::String("skipped".to_string())),
                ("index", Value::from(6)),
                ("total", Value::from(TOTAL_STAGES)),
            ],
        );
        Vec::new()
    } else {
        events::emit(
            "stage",
            vec![
                ("id", Value::String("packages".to_string())),
                ("label", Value::String("Packages".to_string())),
                ("state", Value::String("start".to_string())),
                ("index", Value::from(6)),
                ("total", Value::from(TOTAL_STAGES)),
            ],
        );
        let failures = packages::install(&backup_packages);
        events::emit(
            "stage",
            vec![
                ("id", Value::String("packages".to_string())),
                ("label", Value::String("Packages".to_string())),
                ("state", Value::String("complete".to_string())),
                ("index", Value::from(6)),
                ("total", Value::from(TOTAL_STAGES)),
            ],
        );
        failures
    };

    events::emit(
        "stage",
        vec![
            ("id", Value::String("cleanup".to_string())),
            ("label", Value::String("Cleanup".to_string())),
            ("state", Value::String("start".to_string())),
            ("index", Value::from(7)),
            ("total", Value::from(TOTAL_STAGES)),
        ],
    );
    cleanup(&tmp_dir);
    events::emit(
        "stage",
        vec![
            ("id", Value::String("cleanup".to_string())),
            ("label", Value::String("Cleanup".to_string())),
            ("state", Value::String("complete".to_string())),
            ("index", Value::from(7)),
            ("total", Value::from(TOTAL_STAGES)),
        ],
    );

    events::emit(
        "result",
        vec![
            ("ok", Value::Bool(true)),
            ("name", Value::String(name)),
            ("size", Value::from(size)),
            ("packages", Value::from(backup_packages.len())),
            (
                "packages_installed",
                Value::from(backup_packages.len() - package_failures.len()),
            ),
            (
                "packages_failed",
                Value::Array(
                    package_failures
                        .iter()
                        .cloned()
                        .map(Value::String)
                        .collect(),
                ),
            ),
            ("archives", Value::from(archives.len())),
            (
                "archives_failed",
                Value::Array(failed_archives.into_iter().map(Value::String).collect()),
            ),
            ("warnings", Value::Bool(!package_failures.is_empty())),
            ("restart_required", Value::Bool(true)),
            (
                "duration_ms",
                Value::from(started.elapsed().as_millis() as i64),
            ),
        ],
    );
    Ok(Value::Null)
}
