//! Runtime paths. Python computed these at import time; Rust computes them per call so
//! tests and the front door can change `HOME` and `PREFIX`.

use std::env;
use std::path::{Component, Path, PathBuf};

/// Program name used in the config directory and user-facing text.
pub const APP_NAME: &str = "termux-sync";
/// Version reported by `--version` and the TUI handshake.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// `$HOME` made absolute, falling back to `/` when unset or empty.
#[must_use]
pub fn home_dir() -> PathBuf {
    let path = match env::var_os("HOME") {
        Some(value) if !value.is_empty() => PathBuf::from(value),
        _ => PathBuf::from("/"),
    };
    if path.is_absolute() {
        path
    } else {
        std::path::absolute(&path).unwrap_or(path)
    }
}

/// Termux prefix (`$PREFIX`, usually `/data/data/com.termux/files/usr`), made
/// absolute.
#[must_use]
pub fn prefix() -> PathBuf {
    let path = match env::var_os("PREFIX") {
        Some(value) if !value.is_empty() => PathBuf::from(value),
        _ => PathBuf::from("/data/data/com.termux/files/usr"),
    };
    if path.is_absolute() {
        path
    } else {
        std::path::absolute(&path).unwrap_or(path)
    }
}

/// The directory treated as the Termux home for backups, i.e. `$HOME`.
#[must_use]
pub fn termux_files() -> PathBuf {
    match env::var_os("HOME") {
        Some(value) if !value.is_empty() => PathBuf::from(value),
        _ => home_dir(),
    }
}

/// `~/.config/termux-sync`.
#[must_use]
pub fn config_dir() -> PathBuf {
    home_dir().join(".config").join(APP_NAME)
}

/// `config_dir()/config.json`.
#[must_use]
pub fn config_file() -> PathBuf {
    config_dir().join("config.json")
}

/// `config_dir()/cron.json`, holding scheduled backup settings.
#[must_use]
pub fn cron_file() -> PathBuf {
    config_dir().join("cron.json")
}

/// `config_dir()/sync.log`, the append-only log file.
#[must_use]
pub fn log_file() -> PathBuf {
    config_dir().join("sync.log")
}

/// Default local destination for backups: `~/termux-backups`.
#[must_use]
pub fn default_dest() -> PathBuf {
    home_dir().join("termux-backups")
}

/// Expand a leading `~` or `~/`. Other `~` characters stay literal, like
/// `Path.expanduser()` on POSIX.
#[must_use]
pub fn expand_user(path: &str) -> PathBuf {
    if path == "~" {
        return home_dir();
    }
    if let Some(rest) = path.strip_prefix("~/") {
        return home_dir().join(rest);
    }
    PathBuf::from(path)
}

/// Scratch directory: `$TMPDIR` when set, else `$PREFIX/tmp` if it exists, else
/// the OS temp directory.
#[must_use]
pub fn tmp_root() -> PathBuf {
    if let Some(value) = env::var_os("TMPDIR")
        && !value.is_empty()
    {
        return PathBuf::from(value);
    }
    let prefix_tmp = prefix().join("tmp");
    if prefix_tmp.is_dir() {
        return prefix_tmp;
    }
    env::temp_dir()
}

/// `Path.resolve(strict=False)`: absolute, symlinks resolved as far as possible, `..`
/// collapsed after resolution, missing tails kept.
pub fn resolve_lenient(path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("/"))
            .join(path)
    };

    let mut todo: Vec<ComponentOwned> = absolute
        .components()
        .map(ComponentOwned::from)
        .collect::<Vec<_>>();
    todo.reverse();

    let mut out = PathBuf::new();
    let mut hops = 0usize;
    while let Some(part) = todo.pop() {
        match part {
            ComponentOwned::Prefix | ComponentOwned::RootDir => out.push("/"),
            ComponentOwned::CurDir => {}
            ComponentOwned::ParentDir => {
                out.pop();
            }
            ComponentOwned::Normal(name) => {
                let candidate = out.join(&name);
                match std::fs::symlink_metadata(&candidate) {
                    Ok(meta) if meta.file_type().is_symlink() => {
                        hops += 1;
                        if hops > 40 {
                            out.push(&name);
                            continue;
                        }
                        let Ok(target) = std::fs::read_link(&candidate) else {
                            out.push(&name);
                            continue;
                        };
                        let target = if target.is_absolute() {
                            target
                        } else {
                            out.join(target)
                        };
                        let mut rest: Vec<ComponentOwned> =
                            target.components().map(ComponentOwned::from).collect();
                        rest.reverse();
                        for item in rest {
                            todo.push(item);
                        }
                    }
                    _ => out.push(&name),
                }
            }
        }
    }
    out
}

#[derive(Clone, Debug)]
enum ComponentOwned {
    Prefix,
    RootDir,
    CurDir,
    ParentDir,
    Normal(std::ffi::OsString),
}

impl From<Component<'_>> for ComponentOwned {
    fn from(value: Component<'_>) -> Self {
        match value {
            Component::Prefix(_) => Self::Prefix,
            Component::RootDir => Self::RootDir,
            Component::CurDir => Self::CurDir,
            Component::ParentDir => Self::ParentDir,
            Component::Normal(name) => Self::Normal(name.to_os_string()),
        }
    }
}

/// Python `Path.relative_to` is lexical. `strip_prefix` has the same part-boundary rule.
#[must_use]
pub fn relative_to<'a>(path: &'a Path, root: &Path) -> Option<&'a Path> {
    path.strip_prefix(root).ok()
}

/// Whether `path` equals `root` or lies below it, comparing whole components.
#[must_use]
pub fn is_within(path: &Path, root: &Path) -> bool {
    path == root || path.strip_prefix(root).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expand_only_a_leading_tilde() {
        let joined = expand_user("a~b");
        assert_eq!(joined, PathBuf::from("a~b"));
    }

    #[test]
    fn relative_to_respects_part_boundaries() {
        assert!(relative_to(Path::new("/a/b"), Path::new("/a/b")).is_some());
        assert!(relative_to(Path::new("/a/b2"), Path::new("/a/b")).is_none());
        assert!(!is_within(Path::new("/a/b2"), Path::new("/a/b")));
    }

    #[test]
    fn resolve_keeps_a_missing_tail() {
        let resolved = resolve_lenient(Path::new("/definitely/not/here/tail"));
        assert_eq!(resolved, PathBuf::from("/definitely/not/here/tail"));
    }
}
