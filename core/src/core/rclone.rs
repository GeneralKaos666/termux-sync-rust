//! Google Drive through rclone.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

use crate::core::errors::CoreError;
use crate::core::events;
use crate::core::manifest;
use crate::core::text;

pub struct RcloneStorage {
    folder_id: String,
    remote: String,
    binary: String,
}

impl RcloneStorage {
    pub const REMOTE: &'static str = "termux-sync-gdrive";

    pub fn new(cfg: &Value) -> Result<Self, CoreError> {
        let folder_id = cfg
            .get("gdrive_folder_id")
            .map(text::py_str)
            .unwrap_or_default();
        if !valid_folder_id(&folder_id) {
            return Err(CoreError::user(format!(
                "Invalid Google Drive folder ID: {folder_id:?}"
            )));
        }
        if find_in_path("rclone").is_none() {
            return Err(CoreError::user(
                "rclone not found: Google Drive storage needs rclone",
            ));
        }
        Ok(Self {
            folder_id,
            remote: Self::REMOTE.to_string(),
            binary: "rclone".to_string(),
        })
    }

    fn remote_path(&self, name: &str) -> String {
        let mut base = format!("{}:", self.remote);
        if !self.folder_id.is_empty() {
            base.push_str(&self.folder_id);
        }
        if name.is_empty() {
            base
        } else {
            format!("{base}/{name}")
        }
    }

    fn run(&self, args: &[&str]) -> Result<std::process::Output, CoreError> {
        let output = Command::new(&self.binary)
            .args(args)
            .output()
            .map_err(|error| CoreError::runtime(format!("rclone failed: {error}")))?;
        if output.status.success() {
            return Ok(output);
        }
        Err(command_error(&output))
    }

    /// Run `rclone cat`, treating a missing file as `Ok(None)`.
    fn cat_optional(&self, remote: &str) -> Result<Option<Vec<u8>>, CoreError> {
        let output = Command::new(&self.binary)
            .args(["cat", remote])
            .output()
            .map_err(|error| CoreError::runtime(format!("rclone failed: {error}")))?;
        if output.status.success() {
            return Ok(Some(output.stdout));
        }
        if matches!(output.status.code(), Some(3 | 4)) {
            return Ok(None);
        }
        Err(command_error(&output))
    }

    pub fn upload(&self, backup_dir: &Path, backup_name: &str) -> Result<String, CoreError> {
        let remote = self.remote_path(backup_name);
        self.run(&[
            "copy",
            &backup_dir.to_string_lossy(),
            &remote,
            "--stats",
            "500ms",
            "--stats-one-line",
        ])?;
        events::emit(
            "progress",
            vec![
                ("stage", Value::String("upload".to_string())),
                ("current", Value::from(1)),
                ("total", Value::from(1)),
                ("unit", Value::String("items".to_string())),
            ],
        );
        Ok(remote)
    }

    pub fn fetch_manifest(&self, backup_name: &str) -> Result<Option<Value>, CoreError> {
        let name = manifest::clean_backup_name(backup_name);
        if name.is_empty() {
            return Ok(None);
        }
        let Some(stdout) =
            self.cat_optional(&self.remote_path(&format!("{name}/manifest.json")))?
        else {
            return Ok(None);
        };
        match serde_json::from_slice(&stdout) {
            Ok(value) => Ok(Some(value)),
            Err(_) => Ok(None),
        }
    }

    pub fn list_backups(&self) -> Result<Vec<Value>, CoreError> {
        let output = self.run(&["lsjson", &self.remote_path(""), "--dirs-only"])?;
        let items: Value = serde_json::from_slice(&output.stdout).map_err(CoreError::runtime)?;
        let Some(items) = items.as_array() else {
            return Err(CoreError::runtime(
                "rclone lsjson response was not an array",
            ));
        };
        let mut sorted = items.clone();
        sorted.sort_by(|left, right| {
            let left = left.get("ModTime").map(text::py_str).unwrap_or_default();
            let right = right.get("ModTime").map(text::py_str).unwrap_or_default();
            right.cmp(&left)
        });
        let mut backups = Vec::new();
        for item in sorted {
            let name = item.get("Name").map(text::py_str).unwrap_or_default();
            let Some(stdout) =
                self.cat_optional(&self.remote_path(&format!("{name}/manifest.json")))?
            else {
                continue;
            };
            let value = match serde_json::from_slice(&stdout) {
                Ok(value) => value,
                Err(_) => serde_json::json!({
                    "name": name,
                    "date": item.get("ModTime").map(text::py_str).unwrap_or_default(),
                }),
            };
            backups.push(value);
        }
        Ok(backups)
    }

    pub fn download(&self, backup_name: &str, dest: &Path) -> Result<(), CoreError> {
        fs::create_dir_all(dest).map_err(CoreError::os)?;
        self.run(&[
            "copy",
            &self.remote_path(backup_name),
            &dest.to_string_lossy(),
            "--stats",
            "500ms",
            "--stats-one-line",
        ])?;
        events::emit(
            "progress",
            vec![
                ("stage", Value::String("download".to_string())),
                ("current", Value::from(1)),
                ("total", Value::from(1)),
                ("unit", Value::String("items".to_string())),
            ],
        );
        Ok(())
    }

    pub fn delete_old(&self, max_keep: i64) -> Result<(), CoreError> {
        let output = self.run(&["lsjson", &self.remote_path(""), "--dirs-only"])?;
        let items: Vec<Value> = serde_json::from_slice(&output.stdout)
            .map_err(|error| CoreError::plain(error.to_string()))?;
        let mut sorted: Vec<&Value> = items
            .iter()
            .filter(|item| {
                item.get("Name")
                    .map(text::py_str)
                    .is_some_and(|name| is_backup_name(&name))
            })
            .collect();
        sorted.sort_by(|left, right| {
            let left = left.get("ModTime").map(text::py_str).unwrap_or_default();
            let right = right.get("ModTime").map(text::py_str).unwrap_or_default();
            right.cmp(&left)
        });
        let keep = max_keep.max(1) as usize;
        let mut first_error = None;
        for item in sorted.into_iter().skip(keep) {
            let name = item.get("Name").map(text::py_str).unwrap_or_default();
            if let Err(error) = self.run(&["purge", &self.remote_path(&name)])
                && first_error.is_none()
            {
                first_error = Some(error);
            }
        }
        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    pub fn delete_backup(&self, backup_name: &str) -> Result<(), CoreError> {
        self.run(&["purge", &self.remote_path(backup_name)])
            .map_err(|error| {
                CoreError::runtime(format!(
                    "Failed to delete backup from Google Drive: {}",
                    error.message()
                ))
            })?;
        Ok(())
    }
}

fn command_error(output: &std::process::Output) -> CoreError {
    let code = output.status.code().unwrap_or(1);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stderr = stderr.trim();
    if stderr.is_empty() {
        return CoreError::runtime(format!("rclone exited with {code}"));
    }
    let mut chars = stderr.chars();
    let mut detail: String = chars.by_ref().take(497).collect();
    if chars.next().is_some() {
        detail.push_str("...");
    }
    CoreError::runtime(format!("rclone exited with {code}: {detail}"))
}

fn valid_folder_id(value: &str) -> bool {
    if value.is_empty() {
        return true;
    }
    if value.starts_with('/') {
        return false;
    }
    let forbidden = |ch: char| ch.is_control() || ch == ':' || ch == '\\';
    !value.chars().any(forbidden) && value.split('/').all(|segment| segment != "..")
}

fn is_backup_name(name: &str) -> bool {
    name.starts_with("termux_backup_") && manifest::valid_backup_name(name)
}

fn find_in_path(name: &str) -> Option<PathBuf> {
    let path = env::var_os("PATH")?;
    env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn fixture() -> (tempfile::TempDir, RcloneStorage) {
        let temp = tempfile::tempdir().unwrap();
        let script = temp.path().join("rclone");
        fs::write(
            &script,
            r#"#!/bin/sh
case "$1" in
  lsjson) printf '[{"Name":"termux_backup_20250101_120000","ModTime":"2025-01-01T00:00:00"}]';;
  cat) printf '{"name":"termux_backup_20250101_120000","files":{},"packages":[]}';;
  copy) case "$3" in /*) mkdir -p "$3"; printf data > "$3/copied";; esac;;
  purge) :;;
esac
"#,
        )
        .unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        let storage = RcloneStorage {
            folder_id: String::new(),
            remote: RcloneStorage::REMOTE.to_string(),
            binary: script.to_string_lossy().into_owned(),
        };
        (temp, storage)
    }

    fn script_storage(temp: &tempfile::TempDir, body: &str) -> RcloneStorage {
        let script = temp.path().join("rclone");
        fs::write(&script, body).unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        RcloneStorage {
            folder_id: String::new(),
            remote: RcloneStorage::REMOTE.to_string(),
            binary: script.to_string_lossy().into_owned(),
        }
    }

    #[test]
    fn only_backup_names_are_prunable() {
        assert!(is_backup_name("termux_backup_20260101_020000"));
        assert!(is_backup_name("termux_backup_20260101_020000_auto"));
        assert!(!is_backup_name("Photos"));
        assert!(!is_backup_name("termux_backup_x/.."));
    }

    #[test]
    fn folder_id_rejects_path_segments() {
        assert!(valid_folder_id(""));
        assert!(valid_folder_id("1AbCdEf"));
        assert!(!valid_folder_id(".."));
        assert!(!valid_folder_id("/root"));
        assert!(!valid_folder_id("a:b"));
    }

    #[test]
    fn delete_old_prunes_only_backups_and_keeps_one() {
        let temp = tempfile::tempdir().unwrap();
        let log = temp.path().join("purged.log");
        let storage = script_storage(
            &temp,
            &format!(
                r#"#!/bin/sh
case "$1" in
  lsjson) printf '[{{"Name":"Photos","ModTime":"2025-01-03T00:00:00"}},{{"Name":"termux_backup_20250101_120000","ModTime":"2025-01-01T00:00:00"}},{{"Name":"termux_backup_20250102_120000","ModTime":"2025-01-02T00:00:00"}}]';;
  purge) printf '%s\n' "$2" >> "{}";;
esac
"#,
                log.display()
            ),
        );
        storage.delete_old(0).unwrap();
        let purged = fs::read_to_string(&log).unwrap();
        assert_eq!(
            purged.trim(),
            "termux-sync-gdrive:/termux_backup_20250101_120000"
        );
        assert!(!purged.contains("Photos"), "{purged}");
    }

    #[test]
    fn delete_old_reports_failed_purge() {
        let temp = tempfile::tempdir().unwrap();
        let storage = script_storage(
            &temp,
            r#"#!/bin/sh
case "$1" in
  lsjson) printf '[{"Name":"termux_backup_20250101_120000","ModTime":"2025-01-01T00:00:00"},{"Name":"termux_backup_20250102_120000","ModTime":"2025-01-02T00:00:00"}]';;
  purge) printf 'denied' >&2; exit 1;;
esac
"#,
        );
        let error = storage.delete_old(1).unwrap_err();
        assert!(error.contains("denied"), "{error}");
    }

    #[test]
    fn run_reports_stderr() {
        let temp = tempfile::tempdir().unwrap();
        let storage = script_storage(
            &temp,
            r#"#!/bin/sh
printf 'boom' >&2
exit 1
"#,
        );
        let error = storage.run(&["lsjson", "remote:"]).unwrap_err();
        assert!(error.contains("boom"), "{error}");
    }

    #[test]
    fn run_truncates_long_stderr() {
        let temp = tempfile::tempdir().unwrap();
        let storage = script_storage(
            &temp,
            r#"#!/bin/sh
i=0
while [ "$i" -lt 600 ]; do printf x >&2; i=$((i + 1)); done
exit 1
"#,
        );
        let error = storage.run(&["lsjson", "remote:"]).unwrap_err();
        let text = error.to_string();
        assert!(text.ends_with("..."), "{text}");
        assert!(text.len() < 560, "{} characters", text.len());
    }

    #[test]
    fn list_backups_propagates_a_failed_lsjson() {
        let temp = tempfile::tempdir().unwrap();
        let storage = script_storage(
            &temp,
            r#"#!/bin/sh
case "$1" in
  lsjson) printf 'remote unreachable' >&2; exit 1;;
esac
"#,
        );
        let error = storage.list_backups().unwrap_err();
        assert!(error.contains("remote unreachable"), "{error}");
    }

    #[test]
    fn fetch_manifest_treats_a_missing_file_as_none() {
        let temp = tempfile::tempdir().unwrap();
        let storage = script_storage(
            &temp,
            r#"#!/bin/sh
case "$1" in
  cat) exit 4;;
esac
"#,
        );
        assert!(
            storage
                .fetch_manifest("termux_backup_20250101_120000")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn new_rejects_bad_folder_id() {
        let cfg = serde_json::json!({"gdrive_folder_id": "/root"});
        let Err(error) = RcloneStorage::new(&cfg) else {
            panic!("expected an error for a folder ID with a leading slash");
        };
        let text = error.to_string();
        assert!(
            text.starts_with("termux_sync.UserError: Invalid Google Drive folder ID"),
            "{text}"
        );
    }

    #[test]
    fn rclone_roundtrip_smoke() {
        let (temp, storage) = fixture();
        assert!(
            storage
                .fetch_manifest("termux_backup_20250101_120000")
                .unwrap()
                .is_some()
        );
        assert_eq!(storage.list_backups().unwrap().len(), 1);
        let dest = temp.path().join("dest");
        storage
            .download("termux_backup_20250101_120000", &dest)
            .unwrap();
        assert!(dest.join("copied").is_file());
        let source = temp.path().join("source");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("x"), b"x").unwrap();
        assert!(
            storage
                .upload(&source, "termux_backup_20250101_120000")
                .unwrap()
                .contains("termux-sync-gdrive:")
        );
        storage.delete_old(0).unwrap();
        assert!(
            storage
                .delete_backup("termux_backup_20250101_120000")
                .is_ok()
        );
    }
}
