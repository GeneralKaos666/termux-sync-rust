//! Config loading and saving. Unknown user keys are preserved in insertion order.

use std::fs::{self, OpenOptions};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::fs::PermissionsExt;

use serde_json::{Map, Value};

use crate::core::json;
use crate::core::paths;

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

pub fn load_config() -> Result<Value, String> {
    let dir = paths::config_dir();
    fs::create_dir_all(&dir).map_err(|error| format!("OSError: {error}"))?;
    let file = paths::config_file();
    if !file.exists() {
        return Ok(Value::Object(default_config()));
    }
    let bytes = fs::read(&file).map_err(|error| format!("OSError: {error}"))?;
    let bytes = json::strip_utf8_bom(&bytes);
    let mut data: Value = serde_json::from_slice(bytes).map_err(|error| {
        let mut column = error.column();
        if column == 0 {
            column = 1;
        }
        format!(
            "termux_sync.UserError: The config file is not valid JSON (line {}, column {}).\nFile: {}",
            error.line(),
            column,
            file.display()
        )
    })?;
    let Value::Object(ref mut map) = data else {
        return Err("AttributeError: config.json must contain a JSON object".to_string());
    };
    for (key, value) in default_config() {
        map.entry(key).or_insert(value);
    }
    Ok(data)
}

pub fn save_config(value: &Value) -> Result<(), String> {
    let dir = paths::config_dir();
    fs::create_dir_all(&dir).map_err(|error| format!("OSError: {error}"))?;
    let file = paths::config_file();
    let text = json::to_string_pretty_ascii(value)?;
    let mut handle = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&file)
        .map_err(|error| format!("OSError: {error}"))?;
    use std::io::Write;
    handle
        .write_all(text.as_bytes())
        .map_err(|error| format!("OSError: {error}"))?;
    let _ = fs::set_permissions(&file, fs::Permissions::from_mode(0o600));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
