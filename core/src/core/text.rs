//! Python string semantics used by the port.

use serde_json::Value;

/// `str.strip()` also strips U+001C through U+001F, which Rust `trim` does not.
#[must_use]
pub fn py_strip(text: &str) -> &str {
    text.trim_matches(|c: char| c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c))
}

/// Python slicing by code point: `text[:n]`.
#[must_use]
pub fn head_chars(text: &str, n: usize) -> String {
    text.chars().take(n).collect()
}

/// Python `str.splitlines()`: splits on more separators than `lines()`, including
/// vertical tab, form feed and the Unicode line separators.
#[must_use]
pub fn py_splitlines(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0usize;
    let bytes = text.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        let ch = text[i..].chars().next().unwrap_or('\0');
        let len = ch.len_utf8();
        if matches!(
            ch,
            '\n' | '\r'
                | '\u{0b}'
                | '\u{0c}'
                | '\u{1c}'
                | '\u{1d}'
                | '\u{1e}'
                | '\u{85}'
                | '\u{2028}'
                | '\u{2029}'
        ) {
            out.push(&text[start..i]);
            i += len;
            if ch == '\r' && text[i..].starts_with('\n') {
                i += 1;
            }
            start = i;
            continue;
        }
        i += len;
    }
    if start < text.len() {
        out.push(&text[start..]);
    }
    out
}

/// Python `str(value)` for JSON values.
pub fn py_str(value: &Value) -> String {
    match value {
        Value::Null => "None".to_string(),
        Value::Bool(true) => "True".to_string(),
        Value::Bool(false) => "False".to_string(),
        Value::String(text) => text.clone(),
        Value::Number(number) => number.to_string(),
        Value::Array(items) => {
            let inner: Vec<String> = items.iter().map(py_repr).collect();
            format!("[{}]", inner.join(", "))
        }
        Value::Object(map) => {
            let inner: Vec<String> = map
                .iter()
                .map(|(key, value)| format!("'{}': {}", key, py_repr(value)))
                .collect();
            format!("{{{}}}", inner.join(", "))
        }
    }
}

fn py_repr(value: &Value) -> String {
    match value {
        Value::String(text) => format!("'{}'", text.replace('\\', "\\\\").replace('\'', "\\'")),
        other => py_str(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_matches_python() {
        assert_eq!(py_strip(" \t\n x \u{1c}"), "x");
        assert_eq!(py_strip("\u{1d}name\u{1f}"), "name");
    }

    #[test]
    fn head_counts_code_points() {
        assert_eq!(head_chars("a👨‍👩‍👧b", 2), "a👨");
        assert_eq!(head_chars("e\u{301}x", 2), "e\u{301}");
    }

    #[test]
    fn splitlines_matches_python() {
        let parts = py_splitlines("a\r\nb\u{0b}c\u{2028}d\n");
        assert_eq!(parts, vec!["a", "b", "c", "d"]);
    }

    #[test]
    fn py_str_uses_python_spellings() {
        assert_eq!(py_str(&serde_json::json!(null)), "None");
        assert_eq!(py_str(&serde_json::json!(true)), "True");
        assert_eq!(py_str(&serde_json::json!(123)), "123");
    }
}
