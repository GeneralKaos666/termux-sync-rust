//! Google Drive through rclone.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

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

    pub fn new(cfg: &Value) -> Result<Self, String> {
        let folder_id = cfg
            .get("gdrive_folder_id")
            .map(text::py_str)
            .unwrap_or_default();
        if find_in_path("rclone").is_none() {
            return Err(
                "termux_sync.UserError: rclone not found: Google Drive storage needs rclone"
                    .to_string(),
            );
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

    fn run(&self, args: &[&str]) -> Result<std::process::Output, String> {
        Command::new(&self.binary)
            .args(args)
            .output()
            .map_err(|error| format!("RuntimeError: rclone failed: {error}"))
    }

    pub fn upload(&self, backup_dir: &Path, backup_name: &str) -> Result<String, String> {
        let remote = self.remote_path(backup_name);
        let output = self.run(&[
            "copy",
            &backup_dir.to_string_lossy(),
            &remote,
            "--stats",
            "500ms",
            "--stats-one-line",
        ])?;
        if !output.status.success() {
            return Err(format!(
                "RuntimeError: rclone exited with {}",
                output.status.code().unwrap_or(1)
            ));
        }
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

    pub fn fetch_manifest(&self, backup_name: &str) -> Option<Value> {
        let name = manifest::clean_backup_name(backup_name);
        if name.is_empty() {
            return None;
        }
        let output = self
            .run(&["cat", &self.remote_path(&format!("{name}/manifest.json"))])
            .ok()?;
        if !output.status.success() {
            return None;
        }
        serde_json::from_slice(&output.stdout).ok()
    }

    pub fn list_backups(&self) -> Vec<Value> {
        let Ok(output) = self.run(&["lsjson", &self.remote_path(""), "--dirs-only"]) else {
            return Vec::new();
        };
        if !output.status.success() {
            return Vec::new();
        }
        let Ok(items) = serde_json::from_slice::<Value>(&output.stdout) else {
            return Vec::new();
        };
        let Some(items) = items.as_array() else {
            return Vec::new();
        };
        let mut sorted = items.clone();
        sorted.sort_by(|left, right| {
            let left = left.get("ModTime").map(text::py_str).unwrap_or_default();
            let right = right.get("ModTime").map(text::py_str).unwrap_or_default();
            right.cmp(&left)
        });
        sorted
            .into_iter()
            .filter_map(|item| {
                let name = item.get("Name").map(text::py_str).unwrap_or_default();
                let output = self
                    .run(&["cat", &self.remote_path(&format!("{name}/manifest.json"))])
                    .ok()?;
                if output.status.success()
                    && let Ok(value) = serde_json::from_slice(&output.stdout)
                {
                    Some(value)
                } else {
                    Some(serde_json::json!({
                        "name": name,
                        "date": item.get("ModTime").map(text::py_str).unwrap_or_default(),
                    }))
                }
            })
            .collect()
    }

    pub fn download(&self, backup_name: &str, dest: &Path) -> Result<(), String> {
        fs::create_dir_all(dest).map_err(|error| format!("OSError: {error}"))?;
        let output = self.run(&[
            "copy",
            &self.remote_path(backup_name),
            &dest.to_string_lossy(),
            "--stats",
            "500ms",
            "--stats-one-line",
        ])?;
        if !output.status.success() {
            return Err(format!(
                "RuntimeError: rclone exited with {}",
                output.status.code().unwrap_or(1)
            ));
        }
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

    pub fn delete_old(&self, max_keep: i64) -> Result<(), String> {
        let Ok(output) = self.run(&["lsjson", &self.remote_path(""), "--dirs-only"]) else {
            return Ok(());
        };
        if !output.status.success() {
            return Ok(());
        }
        let Ok(items) = serde_json::from_slice::<Value>(&output.stdout) else {
            return Ok(());
        };
        let Some(items) = items.as_array() else {
            return Ok(());
        };
        let mut sorted = items.clone();
        sorted.sort_by(|left, right| {
            let left = left.get("ModTime").map(text::py_str).unwrap_or_default();
            let right = right.get("ModTime").map(text::py_str).unwrap_or_default();
            right.cmp(&left)
        });
        let keep = max_keep.max(0) as usize;
        for item in sorted.into_iter().skip(keep) {
            let name = item.get("Name").map(text::py_str).unwrap_or_default();
            let _ = self.run(&["purge", &self.remote_path(&name)]);
        }
        Ok(())
    }

    pub fn delete_backup(&self, backup_name: &str) -> Result<(), String> {
        let output = self.run(&["purge", &self.remote_path(backup_name)])?;
        if !output.status.success() {
            return Err(format!(
                "RuntimeError: Failed to delete backup from Google Drive: rclone exited with {}",
                output.status.code().unwrap_or(1)
            ));
        }
        Ok(())
    }
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

    #[test]
    fn rclone_roundtrip_smoke() {
        let (temp, storage) = fixture();
        assert!(
            storage
                .fetch_manifest("termux_backup_20250101_120000")
                .is_some()
        );
        assert_eq!(storage.list_backups().len(), 1);
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
