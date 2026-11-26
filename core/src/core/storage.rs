//! Storage backends. W3 implements local storage; W4 adds rclone and GitHub.

use std::fs::{self, File, FileTimes};
use std::io::Read;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::core::errors::CoreError;
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

    pub fn upload(&self, backup_dir: &Path, backup_name: &str) -> Result<UploadResult, CoreError> {
        let dest = self.root.join(backup_name);
        if paths::resolve_lenient(&dest) == paths::resolve_lenient(backup_dir) {
            return Err(CoreError::runtime(
                "local_path points at the temporary backup directory; refusing to copy onto itself",
            ));
        }
        let staging = self
            .root
            .join(format!(".{backup_name}.upload-{}", std::process::id()));
        let result = self.stage_upload(backup_dir, &staging, &dest);
        if result.is_err() {
            let _ = fs::remove_dir_all(&staging);
        }
        result
    }

    fn stage_upload(
        &self,
        backup_dir: &Path,
        staging: &Path,
        dest: &Path,
    ) -> Result<UploadResult, CoreError> {
        fs::create_dir_all(staging).map_err(CoreError::os)?;
        let mut files: Vec<PathBuf> = fs::read_dir(backup_dir)
            .map_err(CoreError::os)?
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.is_file())
            .collect();
        upload_order(&mut files);
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
            let target = staging.join(&name);
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
        fs::rename(staging, dest).map_err(CoreError::os)?;
        Ok(UploadResult {
            location: dest.to_string_lossy().into_owned(),
            checksums,
        })
    }

    pub fn fetch_manifest(&self, backup_name: &str) -> Result<Option<Value>, CoreError> {
        let name = manifest::clean_backup_name(backup_name);
        if name.is_empty() {
            return Ok(None);
        }
        let dir = self.root.join(name);
        read_manifest(&dir)
    }

    pub fn list_backups(&self) -> Result<Vec<Value>, CoreError> {
        if !self.root.exists() {
            return Ok(Vec::new());
        }
        let mut dirs: Vec<PathBuf> = fs::read_dir(&self.root)
            .map_err(CoreError::os)?
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.is_dir())
            .collect();
        dirs.sort_by(|left, right| right.cmp(left));
        Ok(dirs
            .into_iter()
            .filter_map(|dir| read_manifest(&dir).ok().flatten())
            .filter(|manifest| manifest::manifest_shape(manifest, false))
            .collect())
    }

    pub fn download(&self, backup_name: &str, dest: &Path) -> Result<(), CoreError> {
        let name = manifest::clean_backup_name(backup_name);
        let source = self.root.join(&name);
        if name.is_empty() || !source.exists() {
            return Err(CoreError::file_not_found(format!(
                "Backup not found: {}",
                source.display()
            )));
        }
        fs::create_dir_all(dest).map_err(CoreError::os)?;
        let mut files: Vec<PathBuf> = fs::read_dir(&source)
            .map_err(CoreError::os)?
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

    pub fn delete_old(&self, max_keep: i64) -> Result<(), CoreError> {
        if !self.root.exists() {
            return Ok(());
        }
        let Ok(entries) = fs::read_dir(&self.root) else {
            return Ok(());
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
        let keep = max_keep.max(1) as usize;
        let mut first_error = None;
        for (path, _) in dirs.into_iter().skip(keep) {
            if let Err(error) = fs::remove_dir_all(&path)
                && first_error.is_none()
            {
                first_error = Some(CoreError::os(error));
            }
        }
        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    pub fn delete_backup(&self, backup_name: &str) -> Result<(), CoreError> {
        let cleaned = manifest::clean_backup_name(backup_name);
        if cleaned.is_empty() {
            return Err(CoreError::value(format!(
                "Invalid backup name: {backup_name:?}"
            )));
        }
        let target = self.root.join(cleaned);
        if !target.exists() {
            return Err(CoreError::file_not_found(format!(
                "Backup not found: {}",
                target.display()
            )));
        }
        fs::remove_dir_all(target).map_err(CoreError::os)
    }
}

fn upload_order(files: &mut [PathBuf]) {
    files.sort_by(|left, right| {
        let left_manifest = left.file_name().is_some_and(|name| name == "manifest.json");
        let right_manifest = right
            .file_name()
            .is_some_and(|name| name == "manifest.json");
        left_manifest
            .cmp(&right_manifest)
            .then_with(|| left.cmp(right))
    });
}

fn is_backup_dir(path: &Path) -> bool {
    read_manifest(path)
        .ok()
        .flatten()
        .and_then(|manifest| manifest.get("name").cloned())
        .is_some()
}

fn read_manifest(dir: &Path) -> Result<Option<Value>, CoreError> {
    let file = dir.join("manifest.json");
    if !file.exists() {
        return Ok(None);
    }
    crate::core::json::read_json(&file).map(Some)
}

pub fn sha256_file(path: &Path) -> Result<String, CoreError> {
    let mut file = File::open(path).map_err(CoreError::os)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1 << 20];
    loop {
        let read = file.read(&mut buffer).map_err(CoreError::os)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

pub fn copy_with_sha(src: &Path, dst: &Path) -> Result<String, CoreError> {
    if dst.exists() {
        let left = fs::metadata(src).map_err(CoreError::os)?;
        let right = fs::metadata(dst).map_err(CoreError::os)?;
        if left.dev() == right.dev() && left.ino() == right.ino() {
            return Err(CoreError::same_file(format!(
                "source and destination are the same file: {}",
                src.display()
            )));
        }
    }
    copy_with_stat(src, dst)?;
    sha256_file(dst)
}

pub fn copy_with_stat(src: &Path, dst: &Path) -> Result<(), CoreError> {
    let meta = fs::metadata(src).map_err(CoreError::os)?;
    fs::copy(src, dst).map_err(CoreError::os)?;
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
    pub fn from_config(cfg: &Value) -> Result<Self, CoreError> {
        match cfg
            .get("storage")
            .and_then(Value::as_str)
            .unwrap_or("local")
        {
            "local" => Ok(Self::Local(LocalStorage::new(cfg))),
            "gdrive" => Ok(Self::Rclone(RcloneStorage::new(cfg)?)),
            "github" => Ok(Self::GitHub(GitHubStorage::new(cfg)?)),
            other => Err(CoreError::user(format!(
                "Unknown storage backend: {other:?}"
            ))),
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Self::Local(_) => "local",
            Self::Rclone(_) => "gdrive",
            Self::GitHub(_) => "github",
        }
    }

    fn backend(&self) -> &dyn Backend {
        match self {
            Self::Local(local) => local,
            Self::Rclone(rclone) => rclone,
            Self::GitHub(github) => github,
        }
    }

    pub fn preflight(&self) -> Result<(), CoreError> {
        self.backend().preflight()
    }

    pub fn list_backups(&self) -> Result<Vec<Value>, CoreError> {
        self.backend().list_backups()
    }

    pub fn fetch_manifest(&self, name: &str) -> Result<Option<Value>, CoreError> {
        self.backend().fetch_manifest(name)
    }

    pub fn download(&self, name: &str, dest: &Path) -> Result<(), CoreError> {
        self.backend().download(name, dest)
    }

    pub fn upload(&self, dir: &Path, name: &str) -> Result<UploadResult, CoreError> {
        self.backend().upload(dir, name)
    }

    pub fn delete_old(&self, max_keep: i64) -> Result<(), CoreError> {
        self.backend().delete_old(max_keep)
    }

    pub fn delete_backup(&self, name: &str) -> Result<(), CoreError> {
        self.backend().delete_backup(name)
    }
}

/// Operations every storage backend provides. `Storage` erases the concrete
/// backend behind this trait so each command is dispatched once, not per method.
pub trait Backend {
    fn preflight(&self) -> Result<(), CoreError> {
        Ok(())
    }
    fn list_backups(&self) -> Result<Vec<Value>, CoreError>;
    fn fetch_manifest(&self, name: &str) -> Result<Option<Value>, CoreError>;
    fn download(&self, name: &str, dest: &Path) -> Result<(), CoreError>;
    fn upload(&self, dir: &Path, name: &str) -> Result<UploadResult, CoreError>;
    fn delete_old(&self, max_keep: i64) -> Result<(), CoreError>;
    fn delete_backup(&self, name: &str) -> Result<(), CoreError>;
}

impl Backend for LocalStorage {
    fn list_backups(&self) -> Result<Vec<Value>, CoreError> {
        LocalStorage::list_backups(self)
    }
    fn fetch_manifest(&self, name: &str) -> Result<Option<Value>, CoreError> {
        LocalStorage::fetch_manifest(self, name)
    }
    fn download(&self, name: &str, dest: &Path) -> Result<(), CoreError> {
        LocalStorage::download(self, name, dest)
    }
    fn upload(&self, dir: &Path, name: &str) -> Result<UploadResult, CoreError> {
        LocalStorage::upload(self, dir, name)
    }
    fn delete_old(&self, max_keep: i64) -> Result<(), CoreError> {
        LocalStorage::delete_old(self, max_keep)
    }
    fn delete_backup(&self, name: &str) -> Result<(), CoreError> {
        LocalStorage::delete_backup(self, name)
    }
}

impl Backend for RcloneStorage {
    fn list_backups(&self) -> Result<Vec<Value>, CoreError> {
        RcloneStorage::list_backups(self)
    }
    fn fetch_manifest(&self, name: &str) -> Result<Option<Value>, CoreError> {
        RcloneStorage::fetch_manifest(self, name)
    }
    fn download(&self, name: &str, dest: &Path) -> Result<(), CoreError> {
        RcloneStorage::download(self, name, dest)
    }
    fn upload(&self, dir: &Path, name: &str) -> Result<UploadResult, CoreError> {
        RcloneStorage::upload(self, dir, name).map(|location| UploadResult {
            location,
            checksums: Vec::new(),
        })
    }
    fn delete_old(&self, max_keep: i64) -> Result<(), CoreError> {
        RcloneStorage::delete_old(self, max_keep)
    }
    fn delete_backup(&self, name: &str) -> Result<(), CoreError> {
        RcloneStorage::delete_backup(self, name)
    }
}

impl Backend for GitHubStorage {
    fn preflight(&self) -> Result<(), CoreError> {
        GitHubStorage::preflight(self)
    }
    fn list_backups(&self) -> Result<Vec<Value>, CoreError> {
        GitHubStorage::list_backups(self)
    }
    fn fetch_manifest(&self, name: &str) -> Result<Option<Value>, CoreError> {
        GitHubStorage::fetch_manifest(self, name)
    }
    fn download(&self, name: &str, dest: &Path) -> Result<(), CoreError> {
        GitHubStorage::download(self, name, dest)
    }
    fn upload(&self, dir: &Path, name: &str) -> Result<UploadResult, CoreError> {
        GitHubStorage::upload(self, dir, name).map(|location| UploadResult {
            location,
            checksums: Vec::new(),
        })
    }
    fn delete_old(&self, max_keep: i64) -> Result<(), CoreError> {
        GitHubStorage::delete_old(self, max_keep)
    }
    fn delete_backup(&self, name: &str) -> Result<(), CoreError> {
        GitHubStorage::delete_backup(self, name)
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
        let backups = local.list_backups().unwrap();
        assert_eq!(backups.len(), 1);
        assert_eq!(backups[0]["name"], manifest["name"]);
        let files = manifest.get("files").unwrap();
        let (ok, _) = verify_local_backup(Path::new(&uploaded.location), files);
        assert!(ok);
        let second = temp.path().join("second");
        make_backup(&second);
        local
            .upload(&second, "termux_backup_20250101_130000")
            .unwrap();
        assert_eq!(local.list_backups().unwrap().len(), 2);
        local.delete_old(1).unwrap();
        assert_eq!(local.list_backups().unwrap().len(), 1);
    }

    #[test]
    fn local_list_of_missing_root_is_empty_ok() {
        let temp = tempfile::tempdir().unwrap();
        let local = LocalStorage {
            root: temp.path().join("missing"),
        };
        assert!(local.list_backups().unwrap().is_empty());
    }

    #[test]
    fn same_file_copy_is_refused() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("file");
        fs::write(&path, b"x").unwrap();
        let error = copy_with_sha(&path, &path).unwrap_err();
        assert!(error.contains("SameFileError"));
    }

    #[test]
    fn upload_is_atomic_on_copy_failure() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("backups");
        let work = temp.path().join("work");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&work).unwrap();
        fs::write(work.join("home.tar.gz"), b"data").unwrap();
        fs::write(work.join("manifest.json"), b"{}").unwrap();
        fs::set_permissions(work.join("home.tar.gz"), fs::Permissions::from_mode(0o000)).unwrap();
        let cfg = json!({"storage": "local", "local_path": root.to_string_lossy()});
        let storage = LocalStorage::new(&cfg);
        assert!(storage.upload(&work, "termux_backup_x").is_err());
        assert!(!root.join("termux_backup_x").exists());
        let leftovers: Vec<_> = fs::read_dir(&root).unwrap().flatten().collect();
        assert!(leftovers.is_empty());
    }

    #[test]
    fn upload_places_manifest_last() {
        let mut files: Vec<PathBuf> = ["manifest.json", "usr_etc.tar.gz", "home.tar.gz"]
            .into_iter()
            .map(PathBuf::from)
            .collect();
        upload_order(&mut files);
        assert_eq!(files.last().unwrap(), Path::new("manifest.json"));
    }

    #[test]
    fn delete_old_keeps_at_least_one() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("backups");
        fs::create_dir_all(&root).unwrap();
        for name in ["termux_backup_a", "termux_backup_b", "termux_backup_c"] {
            let dir = root.join(name);
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("manifest.json"), format!(r#"{{"name":"{name}"}}"#)).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let cfg = json!({"storage": "local", "local_path": root.to_string_lossy()});
        LocalStorage::new(&cfg).delete_old(0).unwrap();
        let remaining: Vec<String> = fs::read_dir(&root)
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(remaining, vec!["termux_backup_c".to_string()]);
    }

    #[test]
    fn unknown_storage_is_an_error() {
        let err = Storage::from_config(&json!({"storage": "dropbox"}))
            .err()
            .expect("unknown backend must not fall back");
        assert!(err.contains("Unknown storage backend"));
    }
}
