//! Config loading and saving. Unknown user keys are preserved in insertion order.

use std::fs::{self, OpenOptions};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::fs::PermissionsExt;

use serde_json::{Map, Value};

use crate::core::errors::CoreError;
use crate::core::fmt;
use crate::core::json;
use crate::core::paths;

/// Fresh config in the order the Python source wrote it, filled with defaults
/// (local storage, gzip, five backups, no encryption).
#[must_use]
pub fn default_config() -> Map<String, Value> {
    let mut map = Map::new();
    map.insert("storage".to_string(), Value::String("local".to_string()));
    map.insert(
        "local_path".to_string(),
        Value::String(paths::default_dest().to_string_lossy().into_owned()),
    );
    map.insert("gdrive_folder_id".to_string(), Value::String(String::new()));
    map.insert("github_token".to_string(), Value::String(String::new()));
    map.insert("github_repo".to_string(), Value::String(String::new()));
    map.insert(
        "github_branch".to_string(),
        Value::String("main".to_string()),
    );
    map.insert("compression".to_string(), Value::String("gz".to_string()));
    map.insert(
        "exclude_patterns".to_string(),
        Value::Array(
            [
                ".cache",
                ".npm",
                "__pycache__",
                "node_modules",
                ".gradle",
                ".android",
                ".thumbnails",
                "DCIM",
                "Movies",
                "Music",
                ".local/share/Trash",
                ".local/lib",
                ".java",
                ".m2",
                ".ivy2",
                "go/pkg",
                ".rustup/toolchains",
                ".cargo/registry",
                ".cargo/git",
            ]
            .iter()
            .map(|item| Value::String((*item).to_string()))
            .collect(),
        ),
    );
    map.insert("max_backups".to_string(), Value::from(5));
    map.insert("encrypt".to_string(), Value::Bool(false));
    map.insert("encrypt_password".to_string(), Value::String(String::new()));
    map
}

/// Load `config.json`, creating the config directory when missing. A missing
/// file yields the defaults; unknown keys survive and missing keys are filled in.
pub fn load_config() -> Result<Value, CoreError> {
    let dir = paths::config_dir();
    fs::create_dir_all(&dir).map_err(CoreError::os)?;
    let file = paths::config_file();
    if !file.exists() {
        return Ok(Value::Object(default_config()));
    }
    let bytes = fs::read(&file).map_err(CoreError::os)?;
    let bytes = json::strip_utf8_bom(&bytes);
    let mut data: Value = serde_json::from_slice(bytes).map_err(|error| {
        let mut column = error.column();
        if column == 0 {
            column = 1;
        }
        CoreError::user(format!(
            "The config file is not valid JSON (line {}, column {}).\nFile: {}",
            error.line(),
            column,
            file.display()
        ))
    })?;
    let Value::Object(ref mut map) = data else {
        return Err(CoreError::attribute(
            "config.json must contain a JSON object",
        ));
    };
    for (key, value) in default_config() {
        map.entry(key).or_insert(value);
    }
    Ok(data)
}

fn patch_error(detail: &str) -> CoreError {
    CoreError::user(format!("Invalid config: {detail}"))
}

fn require_string(key: &str, value: &Value) -> Result<(), CoreError> {
    if value.is_string() {
        Ok(())
    } else {
        Err(patch_error(&format!("{key} must be a string")))
    }
}

/// Validate an imported config patch. Accepts only known keys, each with its own
/// rule. Unknown keys are rejected so a stale or hostile export cannot smuggle
/// settings in.
pub fn validate_patch(value: &Value) -> Result<(), CoreError> {
    let Value::Object(map) = value else {
        return Err(patch_error("config must be a JSON object"));
    };
    for (key, value) in map {
        match key.as_str() {
            "storage" => {
                let Some(storage) = value.as_str() else {
                    return Err(patch_error("storage must be a string"));
                };
                if !matches!(storage, "local" | "gdrive" | "github") {
                    return Err(patch_error(&format!(
                        "storage must be one of local, gdrive, github (got '{storage}')"
                    )));
                }
            }
            "local_path" | "github_token" | "encrypt_password" => require_string(key, value)?,
            "gdrive_folder_id" => {
                let Some(folder) = value.as_str() else {
                    return Err(patch_error("gdrive_folder_id must be a string"));
                };
                if !folder.is_empty()
                    && (folder.starts_with('/')
                        || folder.split('/').any(|segment| segment == "..")
                        || folder.contains(':'))
                {
                    return Err(patch_error(
                        "gdrive_folder_id must not start with '/', contain a '..' segment, or contain ':'",
                    ));
                }
            }
            "github_repo" => {
                let Some(repo) = value.as_str() else {
                    return Err(patch_error("github_repo must be a string"));
                };
                if !repo.is_empty() {
                    let mut parts = repo.split('/');
                    let owner = parts.next().unwrap_or("");
                    let name = parts.next().unwrap_or("");
                    let valid_char =
                        |c: char| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-');
                    if parts.next().is_some()
                        || owner.is_empty()
                        || name.is_empty()
                        || !owner.chars().all(valid_char)
                        || !name.chars().all(valid_char)
                    {
                        return Err(patch_error(
                            "github_repo must be 'owner/name' using letters, digits, '.', '_' or '-'",
                        ));
                    }
                }
            }
            "github_branch" => {
                let Some(branch) = value.as_str() else {
                    return Err(patch_error("github_branch must be a string"));
                };
                if branch.is_empty() || branch.chars().any(|c| c.is_whitespace() || c.is_control())
                {
                    return Err(patch_error(
                        "github_branch must be non-empty with no whitespace or control characters",
                    ));
                }
            }
            "compression" => {
                let Some(compression) = value.as_str() else {
                    return Err(patch_error("compression must be a string"));
                };
                if !matches!(compression, "gz" | "bz2" | "xz") {
                    return Err(patch_error(&format!(
                        "compression must be one of gz, bz2, xz (got '{compression}')"
                    )));
                }
            }
            "max_backups" => {
                let Some(number) = value.as_i64() else {
                    return Err(patch_error("max_backups must be an integer"));
                };
                if number < 1 {
                    return Err(patch_error("max_backups must be at least 1"));
                }
            }
            "exclude_patterns" => {
                let Some(items) = value.as_array() else {
                    return Err(patch_error("exclude_patterns must be an array of strings"));
                };
                if items.iter().any(|item| !item.is_string()) {
                    return Err(patch_error("exclude_patterns must be an array of strings"));
                }
            }
            "encrypt" => {
                if !value.is_boolean() {
                    return Err(patch_error("encrypt must be a boolean"));
                }
            }
            _ => return Err(patch_error(&format!("Unknown config key: {key}"))),
        }
    }
    Ok(())
}

/// Maximum backups to keep, taken from `max_backups` and clamped to at least 1;
/// defaults to 5 when the key is absent or not an integer.
pub fn retention_limit(cfg: &Value) -> i64 {
    cfg.get("max_backups")
        .and_then(fmt::py_int)
        .unwrap_or(5)
        .max(1)
}

/// Write `value` to `config.json` as ASCII-only pretty JSON with mode 0600.
pub fn save_config(value: &Value) -> Result<(), CoreError> {
    let dir = paths::config_dir();
    fs::create_dir_all(&dir).map_err(CoreError::os)?;
    let file = paths::config_file();
    let text = json::to_string_pretty_ascii(value)?;
    let mut handle = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&file)
        .map_err(CoreError::os)?;
    use std::io::Write;
    handle.write_all(text.as_bytes()).map_err(CoreError::os)?;
    let _ = fs::set_permissions(&file, fs::Permissions::from_mode(0o600));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn defaults_keep_python_order() {
        let config = default_config();
        let keys: Vec<&String> = config.keys().collect();
        assert_eq!(keys.first().map(|key| key.as_str()), Some("storage"));
        assert_eq!(
            keys.last().map(|key| key.as_str()),
            Some("encrypt_password")
        );
    }

    #[test]
    fn retention_is_at_least_one() {
        assert_eq!(retention_limit(&json!({"max_backups": 0})), 1);
        assert_eq!(retention_limit(&json!({"max_backups": -3})), 1);
        assert_eq!(retention_limit(&json!({})), 5);
    }

    #[test]
    fn validate_patch_rejects_bad_values() {
        assert!(validate_patch(&json!({"max_backups": 0})).is_err());
        assert!(validate_patch(&json!({"storage": "dropbox"})).is_err());
        assert!(validate_patch(&json!({"compression": "zip"})).is_err());
        assert!(validate_patch(&json!({"github_repo": "owner"})).is_err());
        assert!(validate_patch(&json!({"gdrive_folder_id": "../x"})).is_err());
        assert!(validate_patch(&json!({"unknown_key": 1})).is_err());
        assert!(validate_patch(&json!({"max_backups": 3, "storage": "local"})).is_ok());
    }
}
