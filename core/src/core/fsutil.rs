//! Small filesystem helpers shared by maintenance commands.

use crate::core::errors::CoreError;
use std::env;
use std::fs::{self, File, OpenOptions};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

pub fn dir_size(path: &Path) -> i64 {
    let mut total = 0i64;
    let Ok(entries) = fs::read_dir(path) else {
        return 0;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.file_type().is_symlink() {
            continue;
        }
        if meta.is_dir() {
            total += dir_size(&path);
        } else {
            total += meta.len() as i64;
        }
    }
    total
}

pub fn which(name: &str) -> Option<PathBuf> {
    let path = env::var_os("PATH")?;
    env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

pub fn remove_tree_ignore(path: &Path) {
    let _ = fs::remove_dir_all(path);
}

pub fn required_tools(storage: &str) -> Vec<(&'static str, bool)> {
    let mut tools = vec![("dpkg", true)];
    if storage == "gdrive" {
        tools.push(("rclone", true));
    }
    tools
}

pub fn try_lock_exclusive(path: &Path) -> Result<File, CoreError> {
    let file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .mode(0o600)
        .open(path)
        .map_err(CoreError::os)?;
    use fs4::fs_std::FileExt;
    if !file.try_lock_exclusive().map_err(CoreError::os)? {
        return Err(CoreError::user("Another daemon is already running"));
    }
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_lock_fails() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("cron.lock");
        let first = try_lock_exclusive(&path).unwrap();
        assert!(try_lock_exclusive(&path).is_err());
        drop(first);
        assert!(try_lock_exclusive(&path).is_ok());
    }

    #[test]
    fn only_rust_tools_are_required() {
        let local: Vec<_> = required_tools("local")
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        assert_eq!(local, vec!["dpkg"]);
        let gdrive: Vec<_> = required_tools("gdrive")
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        assert_eq!(gdrive, vec!["dpkg", "rclone"]);
    }
}
