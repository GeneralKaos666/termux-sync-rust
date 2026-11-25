//! Small filesystem helpers shared by maintenance commands.

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

pub fn try_lock_exclusive(path: &Path) -> Result<File, String> {
    let file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .mode(0o600)
        .open(path)
        .map_err(|error| format!("OSError: {error}"))?;
    use fs4::fs_std::FileExt;
    if !file
        .try_lock_exclusive()
        .map_err(|error| format!("OSError: {error}"))?
    {
        return Err("termux_sync.UserError: Another daemon is already running".to_string());
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
}
