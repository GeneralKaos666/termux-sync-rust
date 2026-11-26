//! The Rust port of `src/termux_sync/`.
//!
//! Wave W1 covers config, paths, logging, events, errors and formatting. Later waves
//! replace the `pending` stubs below.

#![forbid(unsafe_code)]

use std::fs;
use std::path::Path;

use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::core::errors::CoreError;
use crate::core::{
    backup, config, errors, events, fmt, fsutil, json as pyjson, logging, maintenance, packages,
    render, text,
};

/// Port of `log` (`src/termux_sync/__init__.py:131`).
pub fn log(level: &str, message: &str) {
    logging::log(level, message);
}

/// Port of `load_config` (`src/termux_sync/__init__.py:160`).
pub fn load_config() -> Result<Value, CoreError> {
    config::load_config()
}

/// Port of `save_config` (`src/termux_sync/__init__.py:179`).
pub fn save_config(cfg: Value) -> Result<Value, CoreError> {
    config::save_config(&cfg)?;
    Ok(Value::Null)
}

/// Port of `event_stage` (`src/termux_sync/__init__.py:238`).
pub fn event_stage(stage_id: &str, label: &str, index: i64, total: i64, state: &str) {
    events::emit(
        "stage",
        vec![
            ("id", Value::String(stage_id.to_string())),
            ("label", Value::String(label.to_string())),
            ("state", Value::String(state.to_string())),
            ("index", Value::from(index)),
            ("total", Value::from(total)),
        ],
    );
}

/// Port of `event_confirmation` (`src/termux_sync/__init__.py:358`).
#[must_use]
pub fn event_confirmation() -> bool {
    events::confirmation()
}

/// Port of `enable_event_mode` (`src/termux_sync/__init__.py:371`).
pub fn enable_event_mode(cfg: Value) {
    let secrets = ["github_token", "encrypt_password"]
        .iter()
        .filter_map(|key| cfg.get(*key))
        .map(text::py_str)
        .filter(|secret| !secret.is_empty())
        .collect();
    events::enable(secrets);
}

/// Store redaction secrets from a config without enabling event mode.
pub fn set_secrets(cfg: &Value) {
    let secrets = ["github_token", "encrypt_password"]
        .iter()
        .filter_map(|key| cfg.get(*key))
        .map(text::py_str)
        .filter(|secret| !secret.is_empty())
        .collect();
    events::set_secrets(secrets);
}

/// Forward the debug flag to [`maintenance::set_debug`], which gates verbose output.
pub fn set_debug(enabled: bool) {
    maintenance::set_debug(enabled);
}

/// Port of `print_error` (`src/termux_sync/__init__.py:391`).
pub fn print_error(title: &str, message: &str, hint: &str, r#where: &str) {
    render::print_error(title, message, hint, r#where);
}

/// Port of `handle_crash` (`src/termux_sync/__init__.py:471`).
pub fn handle_crash(exc: &Value) {
    let kind = exc
        .get("type")
        .map_or_else(|| "Exception".to_string(), text::py_str);
    let message = exc
        .get("message")
        .map_or_else(|| text::py_str(exc), text::py_str);
    logging::log("ERROR", &format!("{kind}: {message}"));
    let error = CoreError::from_kind(&kind, message);
    let (title, body, hint) = errors::explain(&error);
    render::print_error(&title, &body, &hint, "");
    events::emit(
        "error",
        vec![
            ("message", Value::String(body)),
            ("hint", Value::String(hint)),
            ("exit_code", Value::from(1)),
        ],
    );
}

/// Port of `print_banner` (`src/termux_sync/__init__.py:495`).
pub fn print_banner() {
    render::print_banner();
}

/// Port of `note` (`src/termux_sync/__init__.py:516`).
pub fn note(text: &str, _style: &str) {
    render::note(text);
}

/// Port of `print_section` (`src/termux_sync/__init__.py:522`).
pub fn print_section(title: &str, subtitle: &str) {
    render::print_section(title, subtitle);
}

/// Port of `status_icon` (`src/termux_sync/__init__.py:530`).
#[must_use]
pub fn status_icon(ok: bool) -> String {
    fmt::status_icon(ok)
}

/// Port of `is_narrow` (`src/termux_sync/__init__.py:534`).
#[must_use]
pub fn is_narrow() -> bool {
    fmt::is_narrow()
}

/// Port of `make_progress` (`src/termux_sync/__init__.py:539`).
pub fn make_progress(_transient: bool, _percent: bool) {}

/// Port of `fmt_elapsed` (`src/termux_sync/__init__.py:558`).
#[must_use]
pub fn fmt_elapsed(seconds: f64) -> String {
    fmt::fmt_elapsed(seconds)
}

/// Port of `print_backup_list` (`src/termux_sync/__init__.py:576`).
pub fn print_backup_list(backups: &[Value], title: &str) {
    render::print_backup_list(backups, title);
}

/// Port of `human_size` (`src/termux_sync/__init__.py:619`).
#[must_use]
pub fn human_size(size_bytes: i64) -> String {
    fmt::human_size(size_bytes)
}

/// Port of `sha256` (`src/termux_sync/__init__.py:628`).
pub fn sha256(path: Value, chunk: Option<Value>) -> Result<String, CoreError> {
    let path_text = text::py_str(&path);
    let path = Path::new(&path_text);
    let chunk = chunk
        .as_ref()
        .and_then(fmt::py_int)
        .unwrap_or(1 << 20)
        .max(1) as usize;
    let mut file = fs::File::open(path).map_err(CoreError::os)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; chunk];
    loop {
        use std::io::Read;
        let read = file.read(&mut buffer).map_err(CoreError::os)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

/// Port of `get_installed_packages` (`src/termux_sync/__init__.py:666`).
pub fn get_installed_packages() -> Result<Vec<String>, CoreError> {
    packages::installed_packages()
}

/// Port of `install_packages` (`src/termux_sync/__init__.py:685`).
#[must_use]
pub fn install_packages(pkg_list: &[String], _console: Value) -> Vec<String> {
    packages::install(pkg_list)
}

/// Port of `write_manifest` (`src/termux_sync/__init__.py:915`).
pub fn write_manifest(backup_dir: Value, meta: Value) -> Result<Value, CoreError> {
    let dir_text = text::py_str(&backup_dir);
    let dir = Path::new(&dir_text);
    fs::create_dir_all(dir).map_err(CoreError::os)?;
    pyjson::write_json_pretty_ascii(&dir.join("manifest.json"), &meta)?;
    Ok(Value::Null)
}

/// Port of `read_manifest` (`src/termux_sync/__init__.py:922`).
pub fn read_manifest(backup_dir: Value) -> Result<Option<Value>, CoreError> {
    let dir_text = text::py_str(&backup_dir);
    let dir = Path::new(&dir_text);
    let file = dir.join("manifest.json");
    if !file.exists() {
        return Ok(None);
    }
    pyjson::read_json(&file).map(Some)
}

/// Port of `get_storage` (`src/termux_sync/__init__.py:1670`).
pub fn get_storage(cfg: Value) -> Result<Value, CoreError> {
    backup::get_storage(&cfg)
}

/// Port of `cmd_backup` (`src/termux_sync/__init__.py:1707`).
pub fn cmd_backup(cfg: Value, label: String) -> Result<Value, CoreError> {
    backup::backup(&cfg, &label)
}

/// Port of `cmd_restore` (`src/termux_sync/__init__.py:2222`).
pub fn cmd_restore(cfg: Value, backup_name: String) -> Result<Value, CoreError> {
    backup::restore(&cfg, &backup_name)
}

/// Port of `cmd_list` (`src/termux_sync/__init__.py:2391`).
pub fn cmd_list(cfg: Value) -> Result<Value, CoreError> {
    backup::list(&cfg)
}

/// Port of `cmd_delete` (`src/termux_sync/__init__.py:2453`).
pub fn cmd_delete(cfg: Value, backup_name: String, assume_yes: bool) -> Result<Value, CoreError> {
    maintenance::delete(&cfg, &backup_name, assume_yes)
}

/// Port of `cmd_setup` (`src/termux_sync/__init__.py:2496`).
pub fn cmd_setup() -> Result<Value, CoreError> {
    maintenance::setup()
}

/// Port of `load_schedule` (`src/termux_sync/__init__.py:2616`).
pub fn load_schedule() -> Result<Value, CoreError> {
    maintenance::load_schedule()
}

/// Port of `save_schedule` (`src/termux_sync/__init__.py:2623`).
pub fn save_schedule(sched: Value) -> Result<Value, CoreError> {
    maintenance::save_schedule(&sched)?;
    Ok(Value::Null)
}

/// Port of `cmd_schedule` (`src/termux_sync/__init__.py:2629`).
pub fn cmd_schedule() -> Result<Value, CoreError> {
    maintenance::schedule()
}

/// Port of `cmd_daemon` (`src/termux_sync/__init__.py:2695`).
pub fn cmd_daemon(cfg: Value) -> Result<Value, CoreError> {
    maintenance::daemon(&cfg)
}

/// Port of `short_path` (`src/termux_sync/__init__.py:2726`).
#[must_use]
pub fn short_path(p: Value) -> String {
    fmt::short_path(&text::py_str(&p))
}

/// Port of `cmd_status` (`src/termux_sync/__init__.py:2738`).
pub fn cmd_status(cfg: Value) -> Result<Value, CoreError> {
    maintenance::status(&cfg)
}

/// Port of `cmd_logs` (`src/termux_sync/__init__.py:2805`).
pub fn cmd_logs(lines: i64) -> Result<Value, CoreError> {
    maintenance::logs(lines)
}

/// Port of `cmd_clear_logs` (`src/termux_sync/__init__.py:2831`).
pub fn cmd_clear_logs(assume_yes: bool) -> Result<Value, CoreError> {
    maintenance::clear_logs(assume_yes)
}

/// Port of `dir_size` (`src/termux_sync/__init__.py:2850`).
pub fn dir_size(path: Value) -> Result<i64, CoreError> {
    let path_text = text::py_str(&path);
    Ok(fsutil::dir_size(Path::new(&path_text)))
}

/// Port of `cmd_check` (`src/termux_sync/__init__.py:2875`).
pub fn cmd_check(target: String) -> Result<Value, CoreError> {
    maintenance::check(&target)
}

/// Port of `cmd_clear_cache` (`src/termux_sync/__init__.py:2981`).
pub fn cmd_clear_cache(assume_yes: bool) -> Result<Value, CoreError> {
    maintenance::clear_cache(assume_yes)
}

/// Port of `cmd_export_config` (`src/termux_sync/__init__.py:3072`).
pub fn cmd_export_config(output_path: String) -> Result<Value, CoreError> {
    maintenance::export_config(&output_path)
}

/// Port of `cmd_import_config` (`src/termux_sync/__init__.py:3098`).
pub fn cmd_import_config(input_path: String) -> Result<Value, CoreError> {
    maintenance::import_config(&input_path)
}

/// Port of `print_help` (`src/termux_sync/__init__.py:3199`).
pub fn print_help() {
    render::print_help();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_roundtrip_uses_python_json_escaping() {
        let temp = tempfile::tempdir().unwrap();
        let dir = serde_json::json!(temp.path().to_string_lossy());
        let meta = serde_json::json!({
            "name": "backup",
            "date": "2025-01-01T00:00:00",
            "label": "café",
            "files": {},
            "packages": [],
        });
        write_manifest(dir.clone(), meta.clone()).unwrap();
        let text = fs::read_to_string(temp.path().join("manifest.json")).unwrap();
        assert!(text.contains("caf\\u00e9"), "{text}");
        let read = read_manifest(dir).unwrap();
        assert_eq!(read, Some(meta));
    }

    #[test]
    fn missing_manifest_is_none() {
        let temp = tempfile::tempdir().unwrap();
        let dir = serde_json::json!(temp.path().to_string_lossy());
        assert_eq!(read_manifest(dir).unwrap(), None);
    }
}
