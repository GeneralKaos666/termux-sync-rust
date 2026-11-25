//! Small filesystem helpers shared by maintenance commands.

use std::env;
use std::fs;
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
