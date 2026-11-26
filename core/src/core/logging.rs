//! Log file, matching Python's `[timestamp] [LEVEL] message` format.

use std::fs::{self, OpenOptions};
use std::io::Write;

use chrono::Local;
use serde_json::Value;

use crate::core::events;
use crate::core::paths;

/// Append a redacted `[timestamp] [LEVEL] message` line to the log file and
/// emit it as an `event=log` event. Failures to write are ignored.
pub fn log(level: &str, message: &str) {
    let dir = paths::config_dir();
    let _ = fs::create_dir_all(&dir);
    let message = events::redact_message(message);
    let stamp = Local::now().format("%Y-%m-%d %H:%M:%S");
    let level = level.to_uppercase();
    if let Ok(mut file) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(paths::log_file())
    {
        let _ = writeln!(file, "[{stamp}] [{level}] {message}");
    }
    events::emit(
        "log",
        vec![
            ("level", Value::String(level)),
            ("message", Value::String(message)),
        ],
    );
}
