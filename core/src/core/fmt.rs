//! Formatting helpers copied from the Python source.

use crate::core::paths;
use crate::core::term;
use crate::core::text::head_chars;
use serde_json::Value;

pub fn human_size(size_bytes: i64) -> String {
    let mut size = size_bytes as f64;
    for unit in ["B", "KB", "MB", "GB"] {
        if size < 1024.0 {
            return format!("{size:.1} {unit}");
        }
        size /= 1024.0;
    }
    format!("{size:.1} TB")
}

/// Python `int(seconds)` truncates toward zero; `//` and `%` floor.
pub fn fmt_elapsed(seconds: f64) -> String {
    let seconds = seconds as i64;
    let hours = seconds.div_euclid(3600);
    let rem = seconds.rem_euclid(3600);
    format!("{hours}:{:02}:{:02}", rem / 60, rem % 60)
}

pub fn status_icon(ok: bool) -> String {
    if ok {
        "[bold green]✓[/bold green]".to_string()
    } else {
        "[bold red]✗[/bold red]".to_string()
    }
}

pub fn pct_bar(part: i64, total: i64, width: usize) -> String {
    let pct = if total == 0 {
        0.0
    } else {
        part as f64 / total as f64
    };
    let filled = (width as f64 * pct) as usize;
    let color = if pct < 0.5 {
        "green"
    } else if pct < 0.8 {
        "yellow"
    } else {
        "red"
    };
    format!(
        "[{color}]{}{}[/{color}] [dim]{:.1}%[/dim]",
        "█".repeat(filled.min(width)),
        "░".repeat(width.saturating_sub(filled)),
        pct * 100.0
    )
}

pub fn is_narrow() -> bool {
    term::width() < 72
}

pub fn short_path(path: &str) -> String {
    let prefix = paths::prefix();
    let home = paths::home_dir();
    let prefix_text = prefix.to_string_lossy();
    let home_text = home.to_string_lossy();
    if path == prefix_text {
        return "$PREFIX".to_string();
    }
    if let Some(rest) = path.strip_prefix(&format!("{prefix_text}/")) {
        return format!("$PREFIX/{rest}");
    }
    if path == home_text {
        return "~".to_string();
    }
    if let Some(rest) = path.strip_prefix(&format!("{home_text}/")) {
        return format!("~/{rest}");
    }
    path.to_string()
}

/// Python `int(size_bytes)` on a JSON value, truncating toward zero.
pub fn py_int(value: &Value) -> Option<i64> {
    match value {
        Value::Number(number) => number
            .as_i64()
            .or_else(|| number.as_u64().and_then(|n| i64::try_from(n).ok()))
            .or_else(|| number.as_f64().map(|n| n as i64)),
        Value::Bool(flag) => Some(i64::from(*flag)),
        _ => None,
    }
}

pub fn backup_date(backup: &Value) -> String {
    let date = backup
        .get("date")
        .map(crate::core::text::py_str)
        .unwrap_or_else(|| "—".to_string());
    head_chars(&date, 16).replace('T', " ")
}

pub fn backup_label(backup: &Value) -> String {
    let label = backup
        .get("label")
        .map(crate::core::text::py_str)
        .unwrap_or_default();
    if label
        == backup
            .get("name")
            .map(crate::core::text::py_str)
            .unwrap_or_default()
    {
        String::new()
    } else {
        label
    }
}

pub fn backup_size(backup: &Value) -> i64 {
    backup
        .get("files")
        .and_then(Value::as_object)
        .map(|files| {
            files
                .values()
                .filter_map(|info| info.get("size"))
                .filter_map(py_int)
                .sum()
        })
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_size_matches_python() {
        assert_eq!(human_size(0), "0.0 B");
        assert_eq!(human_size(1), "1.0 B");
        assert_eq!(human_size(1023), "1023.0 B");
        assert_eq!(human_size(1024), "1.0 KB");
        assert_eq!(human_size(1536), "1.5 KB");
        assert_eq!(human_size(1_048_576), "1.0 MB");
        assert_eq!(human_size(5_368_709_120), "5.0 GB");
        assert_eq!(human_size(2_199_023_255_552), "2.0 TB");
    }

    #[test]
    fn elapsed_matches_python() {
        assert_eq!(fmt_elapsed(0.0), "0:00:00");
        assert_eq!(fmt_elapsed(59.0), "0:00:59");
        assert_eq!(fmt_elapsed(61.0), "0:01:01");
        assert_eq!(fmt_elapsed(3600.0), "1:00:00");
        assert_eq!(fmt_elapsed(3725.8), "1:02:05");
        assert_eq!(fmt_elapsed(-1.0), "-1:59:59");
        assert_eq!(fmt_elapsed(-3661.9), "-2:58:59");
    }
}
