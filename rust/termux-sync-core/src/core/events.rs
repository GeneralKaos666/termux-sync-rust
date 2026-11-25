//! Line-delimited JSON events for the TUI, protocol version 1.

use std::io::{BufRead, Write};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value};

#[derive(Debug)]
struct EventState {
    secrets: Vec<String>,
    stdout_events: bool,
}

static EVENTS: OnceLock<Mutex<Option<EventState>>> = OnceLock::new();

fn events() -> &'static Mutex<Option<EventState>> {
    EVENTS.get_or_init(|| Mutex::new(None))
}

pub fn enable(secrets: Vec<String>) {
    let secrets = secrets
        .into_iter()
        .filter(|secret| !secret.is_empty())
        .collect();
    if let Ok(mut guard) = events().lock() {
        *guard = Some(EventState {
            secrets,
            stdout_events: true,
        });
    }
}

/// Store redaction secrets without turning on event output. This lets logging
/// redact even when the command itself does not need a full config load.
pub fn set_secrets(secrets: Vec<String>) {
    let secrets = secrets
        .into_iter()
        .filter(|secret| !secret.is_empty())
        .collect();
    if let Ok(mut guard) = events().lock() {
        if let Some(state) = guard.as_mut() {
            state.secrets = secrets;
        } else {
            *guard = Some(EventState {
                secrets,
                stdout_events: false,
            });
        }
    }
}

pub fn enabled() -> bool {
    events()
        .lock()
        .map(|guard| guard.as_ref().is_some_and(|state| state.stdout_events))
        .unwrap_or(false)
}

fn redact_text(text: &str, secrets: &[String]) -> String {
    let mut out = text.to_string();
    for secret in secrets {
        out = out.replace(secret, "***");
    }
    out
}

fn redact(value: Value, secrets: &[String]) -> Value {
    match value {
        Value::String(text) => Value::String(redact_text(&text, secrets)),
        Value::Array(items) => Value::Array(
            items
                .into_iter()
                .map(|item| redact(item, secrets))
                .collect(),
        ),
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(key, item)| (key, redact(item, secrets)))
                .collect(),
        ),
        other => other,
    }
}

/// Emit one event. Fields keep the caller's order. Write errors are swallowed.
pub fn emit(event: &str, fields: Vec<(&str, Value)>) {
    let Ok(guard) = events().lock() else {
        return;
    };
    let Some(state) = guard.as_ref() else {
        return;
    };
    let mut payload = Map::new();
    payload.insert("v".to_string(), Value::from(1));
    payload.insert("event".to_string(), Value::String(event.to_string()));
    payload.insert("ts".to_string(), Value::from(now_millis()));
    for (key, value) in fields {
        payload.insert(key.to_string(), value);
    }
    let payload = redact(Value::Object(payload), &state.secrets);
    let Ok(line) = serde_json::to_string(&payload) else {
        return;
    };
    if state.stdout_events {
        let mut stdout = std::io::stdout().lock();
        let _ = writeln!(stdout, "{line}");
        let _ = stdout.flush();
    }
}

pub fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

/// Throttle high-frequency progress callbacks into event lines.
#[derive(Debug)]
pub struct EventProgress {
    stage: String,
    interval: std::time::Duration,
    last: std::time::Instant,
}

impl EventProgress {
    pub fn new(stage: impl Into<String>) -> Self {
        Self {
            stage: stage.into(),
            interval: std::time::Duration::from_millis(500),
            last: std::time::Instant::now(),
        }
    }

    pub fn emit(
        &mut self,
        current: u64,
        total: Option<u64>,
        unit: &str,
        detail: Option<&str>,
        force: bool,
    ) {
        if !enabled() {
            return;
        }
        let now = std::time::Instant::now();
        if !force && now.duration_since(self.last) < self.interval {
            return;
        }
        self.last = now;
        let mut fields = vec![
            ("stage", Value::String(self.stage.clone())),
            ("current", Value::from(current)),
            ("unit", Value::String(unit.to_string())),
        ];
        if let Some(total) = total {
            fields.push(("total", Value::from(total)));
        }
        if let Some(detail) = detail {
            fields.push(("detail", Value::String(detail.to_string())));
        }
        emit("progress", fields);
    }
}

/// Read one JSON confirmation line from stdin for restore.
pub fn confirmation() -> bool {
    let mut line = String::new();
    let stdin = std::io::stdin();
    if stdin.lock().read_line(&mut line).unwrap_or(0) == 0 {
        return false;
    }
    let Ok(value) = serde_json::from_str::<Value>(&line) else {
        return false;
    };
    value
        .as_object()
        .and_then(|map| map.get("confirm"))
        .map(crate::core::text::py_str)
        .map(|text| crate::core::text::py_strip(&text) == "RESTORE")
        .unwrap_or(false)
}
