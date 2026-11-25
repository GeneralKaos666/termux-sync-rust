//! Storage backends. W3 implements local storage; W4 adds rclone and GitHub.

use std::fs::{self, File, FileTimes};
use std::io::Read;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::core::events;
use crate::core::github::GitHubStorage;
use crate::core::manifest;
use crate::core::paths;
use crate::core::rclone::RcloneStorage;
use crate::core::text;

#[derive(Debug, Clone)]
pub struct UploadResult {
    pub location: String,
    pub checksums: Vec<(String, String)>,
}

pub struct LocalStorage {
    pub root: PathBuf,
}

impl LocalStorage {
    pub fn new(cfg: &Value) -> Self {
        let local_path = cfg
            .get("local_path")
            .map(text::py_str)
            .filter(|path| !path.is_empty())
            .unwrap_or_else(|| paths::default_dest().to_string_lossy().into_owned());
        Self {
            root: paths::expand_user(&local_path),
        }
    }

    pub fn upload(&self, backup_dir: &Path, backup_name: &str) -> Result<UploadResult, String> {
        let dest = self.root.join(backup_name);
        if paths::resolve_lenient(&dest) == paths::resolve_lenient(backup_dir) {
            return Err(
                "RuntimeError: local_path points at the temporary backup directory; refusing to copy onto itself"
                    .to_string(),
            );
        }
        fs::create_dir_all(&dest).map_err(|error| format!("OSError: {error}"))?;
        let mut files: Vec<PathBuf> = fs::read_dir(backup_dir)
            .map_err(|error| format!("OSError: {error}"))?
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.is_file())
            .collect();
        files.sort();
        let total: u64 = files
            .iter()
            .filter_map(|path| fs::metadata(path).ok())
            .map(|meta| meta.len())
            .sum();
        let mut done = 0u64;
        let mut checksums = Vec::new();
        for source in files {
            let name = source
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            let target = dest.join(&name);
            let checksum = copy_with_sha(&source, &target)?;
            checksums.push((name.clone(), checksum));
            done += fs::metadata(&source).map(|meta| meta.len()).unwrap_or(0);
            events::emit(
                "progress",
                vec![
                    ("stage", Value::String("upload".to_string())),
                    ("current", Value::from(done)),
                    ("total", Value::from(total)),
                    ("unit", Value::String("bytes".to_string())),
                    ("detail", Value::String(name)),
                ],
            );
        }
        Ok(UploadResult {
            location: dest.to_string_lossy().into_owned(),
            checksums,
        })
    }

    pub fn fetch_manifest(&self, backup_name: &str) -> Result<Option<Value>, String> {
        let name = manifest::clean_backup_name(backup_name);
        if name.is_empty() {
            return Ok(None);
        }
        let dir = self.root.join(name);
        read_manifest(&dir)
    }

    pub fn list_backups(&self) -> Vec<Value> {
        if !self.root.exists() {
            return Vec::new();
        }
        let Ok(entries) = fs::read_dir(&self.root) else {
            return Vec::new();
        };
        let mut dirs: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.is_dir())
            .collect();
        dirs.sort_by(|left, right| right.cmp(left));
        dirs.into_iter()
            .filter_map(|dir| read_manifest(&dir).ok().flatten())
            .filter(|manifest| manifest::manifest_shape(manifest, false))
            .collect()
    }

    pub fn download(&self, backup_name: &str, dest: &Path) -> Result<(), String> {
        let name = manifest::clean_backup_name(backup_name);
        let source = self.root.join(&name);
        if name.is_empty() || !source.exists() {
            return Err(format!(
                "FileNotFoundError: Backup not found: {}",
                source.display()
            ));
        }
        fs::create_dir_all(dest).map_err(|error| format!("OSError: {error}"))?;
        let mut files: Vec<PathBuf> = fs::read_dir(&source)
            .map_err(|error| format!("OSError: {error}"))?
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.is_file())
            .collect();
        files.sort();
        let total: u64 = files
            .iter()
            .filter_map(|path| fs::metadata(path).ok())
            .map(|meta| meta.len())
            .sum();
        let mut done = 0u64;
        for file in files {
            let name = file
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            copy_with_stat(&file, &dest.join(&name))?;
            done += fs::metadata(&file).map(|meta| meta.len()).unwrap_or(0);
            events::emit(
                "progress",
                vec![
                    ("stage", Value::String("download".to_string())),
                    ("current", Value::from(done)),
                    ("total", Value::from(total)),
                    ("unit", Value::String("bytes".to_string())),
                    ("detail", Value::String(name)),
                ],
            );
        }
        Ok(())
    }

    pub fn delete_old(&self, max_keep: i64) {
        if !self.root.exists() {
            return;
        }
        let Ok(entries) = fs::read_dir(&self.root) else {
            return;
        };
        let mut dirs: Vec<(PathBuf, std::time::SystemTime)> = entries
            .flatten()
            .filter_map(|entry| {
                let path = entry.path();
                if !path.is_dir() || !is_backup_dir(&path) {
                    return None;
                }
                let modified = fs::metadata(&path).ok()?.modified().ok()?;
                Some((path, modified))
            })
            .collect();
        dirs.sort_by_key(|item| std::cmp::Reverse(item.1));
        let keep = max_keep.max(0) as usize;
        for (path, _) in dirs.into_iter().skip(keep) {
            let _ = fs::remove_dir_all(path);
        }
    }

    pub fn delete_backup(&self, backup_name: &str) -> Result<(), String> {
        let cleaned = manifest::clean_backup_name(backup_name);
        if cleaned.is_empty() {
            return Err(format!("ValueError: Invalid backup name: {backup_name:?}"));
        }
        let target = self.root.join(cleaned);
        if !target.exists() {
            return Err(format!(
                "FileNotFoundError: Backup not found: {}",
                target.display()
            ));
        }
        fs::remove_dir_all(target).map_err(|error| format!("OSError: {error}"))
    }
}

fn is_backup_dir(path: &Path) -> bool {
    read_manifest(path)
        .ok()
        .flatten()
        .and_then(|manifest| manifest.get("name").cloned())
        .is_some()
}

fn read_manifest(dir: &Path) -> Result<Option<Value>, String> {
    let file = dir.join("manifest.json");
    if !file.exists() {
        return Ok(None);
    }
    crate::core::json::read_json(&file).map(Some)
}

pub fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file = File::open(path).map_err(|error| format!("OSError: {error}"))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1 << 20];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| format!("OSError: {error}"))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

pub fn copy_with_sha(src: &Path, dst: &Path) -> Result<String, String> {
    if dst.exists() {
        let left = fs::metadata(src).map_err(|error| format!("OSError: {error}"))?;
        let right = fs::metadata(dst).map_err(|error| format!("OSError: {error}"))?;
        if left.dev() == right.dev() && left.ino() == right.ino() {
            return Err(format!(
                "SameFileError: source and destination are the same file: {}",
                src.display()
            ));
        }
    }
    copy_with_stat(src, dst)?;
    sha256_file(dst)
}

pub fn copy_with_stat(src: &Path, dst: &Path) -> Result<(), String> {
    let meta = fs::metadata(src).map_err(|error| format!("OSError: {error}"))?;
    fs::copy(src, dst).map_err(|error| format!("OSError: {error}"))?;
    let _ = fs::set_permissions(dst, fs::Permissions::from_mode(meta.permissions().mode()));
    if let (Ok(accessed), Ok(modified)) = (meta.accessed(), meta.modified()) {
        let times = FileTimes::new()
            .set_accessed(accessed)
            .set_modified(modified);
        let _ = fs::set_times(dst, times);
    }
    Ok(())
}

pub fn verify_local_backup(root: &Path, files: &Value) -> (bool, String) {
    let Some(files) = files.as_object() else {
        return (false, "local checksums do not match".to_string());
    };
    for (key, info) in files {
        let archive = info.get("archive").map(text::py_str).unwrap_or_default();
        let path = root.join(&archive);
        if !path.is_file() {
            return (
                false,
                format!(
                    "missing archive: {}",
                    if archive.is_empty() { key } else { &archive }
                ),
            );
        }
        if let Some(size) = info.get("size").and_then(crate::core::fmt::py_int)
            && path.metadata().map(|meta| meta.len() as i64).unwrap_or(-1) != size
        {
            return (
                false,
                format!(
                    "size mismatch for {}",
                    if archive.is_empty() { key } else { &archive }
                ),
            );
        }
        let expected = info.get("checksum").map(text::py_str).unwrap_or_default();
        let Ok(actual) = sha256_file(&path) else {
            return (
                false,
                format!(
                    "checksum failed for {}",
                    if archive.is_empty() { key } else { &archive }
                ),
            );
        };
        if expected.is_empty() || actual != expected {
            return (
                false,
                format!(
                    "checksum failed for {}",
                    if archive.is_empty() { key } else { &archive }
                ),
            );
        }
    }
    (true, "local checksums match".to_string())
}

pub enum Storage {
    Local(LocalStorage),
    Rclone(RcloneStorage),
    GitHub(GitHubStorage),
}

impl Storage {
    pub fn from_config(cfg: &Value) -> Result<Self, String> {
        match cfg
            .get("storage")
            .and_then(Value::as_str)
            .unwrap_or("local")
        {
            "local" => Ok(Self::Local(LocalStorage::new(cfg))),
            "gdrive" => Ok(Self::Rclone(RcloneStorage::new(cfg)?)),
            "github" => Ok(Self::GitHub(GitHubStorage::new(cfg)?)),
            _ => Ok(Self::Local(LocalStorage::new(cfg))),
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Self::Local(_) => "local",
            Self::Rclone(_) => "gdrive",
            Self::GitHub(_) => "github",
        }
    }

    pub fn preflight(&self) -> Result<(), String> {
        match self {
            Self::Local(_) | Self::Rclone(_) => Ok(()),
            Self::GitHub(github) => github.preflight(),
        }
    }

    pub fn list_backups(&self) -> Vec<Value> {
        match self {
            Self::Local(local) => local.list_backups(),
            Self::Rclone(rclone) => rclone.list_backups(),
            Self::GitHub(github) => github.list_backups(),
        }
    }

    pub fn fetch_manifest(&self, name: &str) -> Result<Option<Value>, String> {
        match self {
            Self::Local(local) => local.fetch_manifest(name),
            Self::Rclone(rclone) => Ok(rclone.fetch_manifest(name)),
            Self::GitHub(github) => Ok(github.fetch_manifest(name)),
        }
    }

    pub fn download(&self, name: &str, dest: &Path) -> Result<(), String> {
        match self {
            Self::Local(local) => local.download(name, dest),
            Self::Rclone(rclone) => rclone.download(name, dest),
            Self::GitHub(github) => github.download(name, dest),
        }
    }

    pub fn upload(&self, dir: &Path, name: &str) -> Result<UploadResult, String> {
        match self {
            Self::Local(local) => local.upload(dir, name),
            Self::Rclone(rclone) => rclone.upload(dir, name).map(|location| UploadResult {
                location,
                checksums: Vec::new(),
            }),
            Self::GitHub(github) => github.upload(dir, name).map(|location| UploadResult {
                location,
                checksums: Vec::new(),
            }),
        }
    }

    pub fn delete_old(&self, max_keep: i64) {
        match self {
            Self::Local(local) => local.delete_old(max_keep),
            Self::Rclone(rclone) => rclone.delete_old(max_keep),
            Self::GitHub(github) => github.delete_old(max_keep),
        }
    }

    pub fn delete_backup(&self, name: &str) -> Result<(), String> {
        match self {
            Self::Local(local) => local.delete_backup(name),
            Self::Rclone(rclone) => rclone.delete_backup(name),
            Self::GitHub(github) => github.delete_backup(name),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_backup(dir: &Path) -> Value {
        fs::create_dir_all(dir).unwrap();
        fs::write(dir.join("home.tar.gz"), b"archive").unwrap();
        let manifest = json!({
            "name": "termux_backup_20250101_120000",
            "date": "2025-01-01T12:00:00",
            "label": "",
            "files": {"home": {"archive": "home.tar.gz", "size": 7, "checksum": sha256_file(&dir.join("home.tar.gz")).unwrap()}},
            "packages": [],
        });
        crate::core::json::write_json_pretty_ascii(&dir.join("manifest.json"), &manifest).unwrap();
        manifest
    }

    #[test]
    fn local_upload_list_verify_and_prune() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let manifest = make_backup(&source);
        let local = LocalStorage {
            root: temp.path().join("root"),
        };
        let uploaded = local
            .upload(&source, "termux_backup_20250101_120000")
            .unwrap();
        assert!(Path::new(&uploaded.location).join("home.tar.gz").is_file());
        assert_eq!(uploaded.checksums.len(), 2);
        let backups = local.list_backups();
        assert_eq!(backups.len(), 1);
        assert_eq!(backups[0]["name"], manifest["name"]);
        let files = manifest.get("files").unwrap();
        let (ok, _) = verify_local_backup(Path::new(&uploaded.location), files);
        assert!(ok);
        local.delete_old(0);
        assert!(local.list_backups().is_empty());
    }

    #[test]
    fn same_file_copy_is_refused() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("file");
        fs::write(&path, b"x").unwrap();
        let error = copy_with_sha(&path, &path).unwrap_err();
        assert!(error.contains("SameFileError"));
    }
}
