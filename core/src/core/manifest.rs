//! Backup names, labels, manifest shape and restore limits.

use std::path::Path;

use serde_json::Value;

use crate::core::paths;
use crate::core::text::py_strip;

/// Reduce a user-supplied backup name to a single path component, or the empty
/// string when it is blank, `.`, `..` or still contains a separator.
#[must_use]
pub fn clean_backup_name(name: &str) -> String {
    let normalized = name.replace('\\', "/");
    let base = normalized.rsplit('/').next().unwrap_or("");
    let cleaned = py_strip(base);
    if cleaned.is_empty() || cleaned == "." || cleaned == ".." || cleaned.contains('/') {
        String::new()
    } else {
        cleaned.to_string()
    }
}

/// Whether `name` is already a clean, single-component backup name.
#[must_use]
pub fn valid_backup_name(name: &str) -> bool {
    !name.is_empty() && clean_backup_name(name) == name
}

/// Sanitise a label: keep Unicode alphanumerics, `_`, `.` and `-`, replace every
/// other character with `_`, then trim leading and trailing underscores.
#[must_use]
pub fn clean_label(label: &str) -> String {
    // Python's `\w` is Unicode alphanumeric plus underscore.
    let replaced: String = py_strip(label)
        .chars()
        .map(|ch| {
            if ch.is_alphanumeric() || matches!(ch, '_' | '.' | '-') {
                ch
            } else {
                '_'
            }
        })
        .collect();
    replaced.trim_matches('_').to_string()
}

/// Validate a manifest document: a backup name, a `files` object whose entries
/// carry string `archive` and `checksum`, and an optional `packages` array.
/// When `require_files` is set the `files` object must not be empty.
pub fn manifest_shape(manifest: &Value, require_files: bool) -> bool {
    let Some(map) = manifest.as_object() else {
        return false;
    };
    let name = map.get("name").map_or_default(crate::core::text::py_str);
    if !valid_backup_name(&name) {
        return false;
    }
    let Some(Value::Object(files)) = map.get("files") else {
        return false;
    };
    if require_files && files.is_empty() {
        return false;
    }
    for info in files.values() {
        let Some(info) = info.as_object() else {
            return false;
        };
        if !info.get("archive").is_some_and(Value::is_string)
            || !info.get("checksum").is_some_and(Value::is_string)
        {
            return false;
        }
    }
    map.get("packages").is_none_or(Value::is_array)
}

/// Exclude patterns that keep the config directory out of a backup of the whole
/// home directory. Only applies when `source` is the home directory itself.
#[must_use]
pub fn self_excludes(source: &Path) -> Vec<String> {
    let source = paths::resolve_lenient(source);
    let home = paths::resolve_lenient(&paths::termux_files());
    if source == home {
        vec![".config/termux-sync".to_string()]
    } else {
        Vec::new()
    }
}

/// Exclude the local destination when it sits inside the backup source, so a
/// backup never archives itself. Returns nothing for non-local storage.
pub fn storage_excludes(cfg: &Value, source: &Path) -> Vec<String> {
    if cfg
        .get("storage")
        .and_then(Value::as_str)
        .unwrap_or("local")
        != "local"
    {
        return Vec::new();
    }
    let local_path = cfg.get("local_path").map_or_else(
        || paths::default_dest().to_string_lossy().into_owned(),
        crate::core::text::py_str,
    );
    let root = paths::resolve_lenient(&paths::expand_user(&local_path));
    let source = paths::resolve_lenient(source);
    let Ok(relative) = root.strip_prefix(&source) else {
        return Vec::new();
    };
    if relative.as_os_str().is_empty() {
        return Vec::new();
    }
    let name = source
        .file_name()
        .map_or_default(|name| name.to_string_lossy().into_owned());
    vec![format!("{name}/{}", relative.to_string_lossy())]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backup_names_match_python() {
        assert_eq!(clean_backup_name("a/b"), "b");
        assert_eq!(clean_backup_name("a\\b"), "b");
        assert_eq!(clean_backup_name(".."), "");
        assert_eq!(clean_backup_name("  name  "), "name");
        assert!(valid_backup_name("name"));
        assert!(!valid_backup_name("a/b"));
    }

    #[test]
    fn labels_match_python() {
        assert_eq!(clean_label("before reset"), "before_reset");
        assert_eq!(clean_label("../a/b"), ".._a_b");
        assert_eq!(clean_label("héllo wörld"), "héllo_wörld");
    }

    #[test]
    fn manifest_shape_checks_files() {
        let manifest = serde_json::json!({
            "name": "backup",
            "files": {"home": {"archive": "home.tar.gz", "checksum": "abc"}},
            "packages": [],
        });
        assert!(manifest_shape(&manifest, true));
        assert!(!manifest_shape(
            &serde_json::json!({"name": "backup", "files": {}}),
            true
        ));
        assert!(!manifest_shape(
            &serde_json::json!({"name": "a/b", "files": {}}),
            false
        ));
    }
}
