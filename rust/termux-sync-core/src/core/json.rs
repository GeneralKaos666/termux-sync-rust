//! JSON reading and writing with Python's on-disk conventions.

use std::fs;
use std::path::Path;

use serde_json::Value;

pub fn read_json(path: &Path) -> Result<Value, String> {
    let bytes = fs::read(path).map_err(|error| format!("OSError: {error}"))?;
    let bytes = strip_utf8_bom(&bytes).to_vec();
    serde_json::from_slice(&bytes).map_err(|error| {
        let mut column = error.column();
        if column == 0 {
            column = 1;
        }
        format!(
            "JSONDecodeError: {} at line {}, column {}",
            error,
            error.line(),
            column
        )
    })
}

pub fn strip_utf8_bom(bytes: &[u8]) -> &[u8] {
    bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes)
}

/// Python `json.dump(..., indent=2)` escapes non-ASCII by default.
pub fn to_string_pretty_ascii(value: &Value) -> Result<String, String> {
    let text =
        serde_json::to_string_pretty(value).map_err(|error| format!("ValueError: {error}"))?;
    Ok(escape_non_ascii(&text))
}

pub fn write_json_pretty_ascii(path: &Path, value: &Value) -> Result<(), String> {
    let text = to_string_pretty_ascii(value)?;
    fs::write(path, text).map_err(|error| format!("OSError: {error}"))
}

/// Escape non-ASCII code points inside JSON string literals, leaving structure alone.
fn escape_non_ascii(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_string = false;
    let mut escaped = false;
    for ch in text.chars() {
        if !in_string {
            out.push(ch);
            if ch == '"' {
                in_string = true;
            }
            continue;
        }
        if escaped {
            out.push(ch);
            escaped = false;
            continue;
        }
        match ch {
            '\\' => {
                out.push(ch);
                escaped = true;
            }
            '"' => {
                out.push(ch);
                in_string = false;
            }
            ch if ch.is_ascii() => out.push(ch),
            ch => {
                let code = ch as u32;
                if code <= 0xffff {
                    out.push_str(&format!("\\u{code:04x}"));
                } else {
                    let value = code - 0x1_0000;
                    let high = 0xd800 + (value >> 10);
                    let low = 0xdc00 + (value & 0x3ff);
                    out.push_str(&format!("\\u{high:04x}\\u{low:04x}"));
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn ascii_escapes_like_python() {
        let text = to_string_pretty_ascii(&json!({"label": "café"})).unwrap();
        assert_eq!(text, "{\n  \"label\": \"caf\\u00e9\"\n}");
    }

    #[test]
    fn strips_bom() {
        assert_eq!(strip_utf8_bom(&[0xef, 0xbb, 0xbf, b'{']), b"{");
    }
}
