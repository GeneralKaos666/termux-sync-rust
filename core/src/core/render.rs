//! Human console output. Rich markup is replaced by plain text; ADR-003 allows styling
//! differences, not wording differences.

use std::io::Write;

use serde_json::Value;

use crate::core::errors::CoreError;
use crate::core::events;
use crate::core::fmt;
use crate::core::paths;
use crate::core::term;
use crate::core::text::py_str;

const BANNER: &str = r"
  ████████╗███████╗██████╗ ███╗   ███╗██╗   ██╗██╗  ██╗
  ╚══██╔══╝██╔════╝██╔══██╗████╗ ████║██║   ██║╚██╗██╔╝
     ██║   █████╗  ██████╔╝██╔████╔██║██║   ██║ ╚███╔╝
     ██║   ██╔══╝  ██╔══██╗██║╚██╔╝██║██║   ██║ ██╔██╗
     ██║   ███████╗██║  ██║██║ ╚═╝ ██║╚██████╔╝██╔╝ ██╗
     ╚═╝   ╚══════╝╚═╝  ╚═╝╚═╝     ╚═╝ ╚═════╝ ╚═╝  ╚═╝
";

fn canonical_line(line: &str) -> String {
    const BOX_CHARS: &str = "─═╭╮╰╯├┤┬┴┼│";
    let mut out = String::with_capacity(line.len());
    let mut index = 0usize;
    while index < line.len() {
        let rest = &line[index..];
        let Some(ch) = rest.chars().next() else {
            break;
        };
        if ch == '['
            && let Some(offset) = rest.find(']')
        {
            let close = index + offset;
            let tag = &line[index + 1..close];
            let name = tag
                .trim_start_matches('/')
                .split([' ', '='])
                .next()
                .unwrap_or("");
            if matches!(
                name,
                "bold"
                    | "dim"
                    | "red"
                    | "green"
                    | "yellow"
                    | "cyan"
                    | "white"
                    | "blue"
                    | "magenta"
                    | "italic"
                    | "underline"
                    | "reverse"
                    | "blink"
                    | "strike"
                    | "link"
            ) {
                index = close + 1;
                continue;
            }
        }
        let mapped = if BOX_CHARS.contains(ch) { ' ' } else { ch };
        if mapped.is_whitespace() {
            if !out.is_empty() && !out.ends_with(' ') {
                out.push(' ');
            }
        } else {
            out.push(mapped);
        }
        index += ch.len_utf8();
    }
    while out.ends_with(' ') {
        out.pop();
    }
    out
}

pub fn print_line(text: &str) {
    let text = canonical_line(text);
    if text.is_empty() {
        return;
    }
    if events::enabled() {
        let mut stream = std::io::stderr().lock();
        let _ = writeln!(stream, "{text}");
    } else {
        let mut stream = std::io::stdout().lock();
        let _ = writeln!(stream, "{text}");
    }
}

pub fn print_raw(text: &str) {
    if events::enabled() {
        let mut stream = std::io::stderr().lock();
        let _ = write!(stream, "{text}");
    } else {
        let mut stream = std::io::stdout().lock();
        let _ = write!(stream, "{text}");
    }
}

fn centered(text: &str, width: usize) -> String {
    let chars = text.chars().count();
    let pad = width.saturating_sub(chars) / 2;
    format!("{}{text}", " ".repeat(pad))
}

pub fn print_banner() {
    let width = term::width();
    let art_width = BANNER
        .lines()
        .map(|line| line.chars().count())
        .max()
        .unwrap_or(0);
    if width >= art_width + 2 {
        for line in BANNER.trim_matches('\n').split('\n') {
            print_line(line);
        }
    } else {
        print_line("");
        print_line(&centered("T E R M U X - S Y N C", width));
        print_line("");
    }
    for line in [
        format!("Version: v{}", paths::VERSION),
        "Source: https://github.com/GeneralKaos666/termux-sync-rust".to_string(),
        " • Termux Backup & Restore".to_string(),
        " • Opensource & License MIT".to_string(),
    ] {
        print_line(&centered(&line, width));
    }
    print_line("");
}

pub fn note(text: &str) {
    print_line(&format!("  {text}"));
}

pub fn print_section(title: &str, subtitle: &str) {
    let width = term::width();
    let label = format!(" {title} ");
    let pad = width.saturating_sub(label.chars().count());
    let left = pad / 2;
    let right = pad.saturating_sub(left);
    print_line("");
    print_line(&format!(
        "{}{}{}",
        "─".repeat(left),
        label,
        "─".repeat(right)
    ));
    if !subtitle.is_empty() {
        note(subtitle);
    }
    print_line("");
}

pub fn print_error(title: &str, message: &str, hint: &str, r#where: &str) {
    let width = term::width().clamp(40, 100);
    print_line("");
    print_line("ERROR");
    let border = "─".repeat(width.saturating_sub(2));
    print_line(&format!("╭{border}╮"));
    print_line(&format!(
        "│  {title:<width$}│",
        width = width.saturating_sub(4)
    ));
    print_line(&format!(
        "│  {:<width$}│",
        "",
        width = width.saturating_sub(4)
    ));
    for line in message.split('\n') {
        print_line(&format!(
            "│  {line:<width$}│",
            width = width.saturating_sub(4)
        ));
    }
    if !r#where.is_empty() {
        print_line(&format!(
            "│  {:<width$}│",
            "",
            width = width.saturating_sub(4)
        ));
        print_line(&format!(
            "│  at {where_text:<width$}│",
            where_text = r#where,
            width = width.saturating_sub(6)
        ));
    }
    if !hint.is_empty() {
        print_line(&format!(
            "│  {:<width$}│",
            "",
            width = width.saturating_sub(4)
        ));
        print_line(&format!(
            "│  How to fix{:<width$}│",
            "",
            width = width.saturating_sub(13)
        ));
        for line in hint.split('\n') {
            print_line(&format!(
                "│  {line:<width$}│",
                width = width.saturating_sub(4)
            ));
        }
    }
    print_line(&format!("╰{border}╯"));
    print_line("");
}

pub fn print_backup_list(backups: &[Value], title: &str) {
    print_line("");
    print_line(&format!("  {title}"));
    print_line("");
    if fmt::is_narrow() {
        for (index, backup) in backups.iter().enumerate() {
            let name = backup
                .get("name")
                .map(py_str)
                .unwrap_or_else(|| "—".to_string());
            print_line(&format!("  {:>2}  {name}", index + 1));
            let mut bits = vec![fmt::backup_date(backup)];
            if let Some(packages) = backup.get("packages").and_then(Value::as_array)
                && !packages.is_empty()
            {
                bits.push(format!("{} pkgs", packages.len()));
            }
            let size = fmt::backup_size(backup);
            if size != 0 {
                bits.push(fmt::human_size(size));
            }
            let label = fmt::backup_label(backup);
            if !label.is_empty() {
                bits.push(label);
            }
            print_line(&format!("      {}", bits.join(" · ")));
            print_line("");
        }
        return;
    }
    print_line(&format!(
        "  {:>3}  {:<32}  {:<16}  {:>5}  {:>10}  {}",
        "#", "Name", "Date", "Pkgs", "Size", "Label"
    ));
    for (index, backup) in backups.iter().enumerate() {
        let name = backup
            .get("name")
            .map(py_str)
            .unwrap_or_else(|| "—".to_string());
        let date = fmt::backup_date(backup);
        let packages = backup
            .get("packages")
            .and_then(Value::as_array)
            .map(|items| items.len().to_string())
            .unwrap_or_else(|| "—".to_string());
        let size = fmt::backup_size(backup);
        let size = if size == 0 {
            "—".to_string()
        } else {
            fmt::human_size(size)
        };
        let label = fmt::backup_label(backup);
        print_line(&format!(
            "  {:>3}  {:<32}  {:<16}  {:>5}  {:>10}  {}",
            index + 1,
            name,
            date,
            packages,
            size,
            if label.is_empty() { "—" } else { &label }
        ));
    }
    print_line("");
}

pub fn confirm(prompt: &str, default: bool) -> bool {
    print_line(&format!("  {prompt} [y/N]"));
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).unwrap_or(0) == 0 {
        return false;
    }
    match line.trim().to_lowercase().as_str() {
        "y" | "yes" => true,
        "n" | "no" => false,
        "" => default,
        _ => default,
    }
}

pub fn ask(prompt: &str, default: &str) -> Result<String, CoreError> {
    let shown = if default.is_empty() {
        format!("  {prompt}: ")
    } else {
        format!("  {prompt} [{default}]: ")
    };
    print_raw(&shown);
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).unwrap_or(0) == 0 {
        return Err(CoreError::user(
            "Input was closed: termux-sync asked a question, but no answer could be read.",
        ));
    }
    let value = line.trim();
    if value.is_empty() {
        Ok(default.to_string())
    } else {
        Ok(value.to_string())
    }
}

pub fn print_help() {
    print_banner();
    print_line("Usage: termux-sync <command> [target] [options]");
    print_line("");
    let groups: &[(&str, &[(&str, &str)])] = &[
        (
            "Backup & Restore",
            &[
                ("backup", "Create a new backup"),
                ("restore", "Restore from a backup"),
                ("list", "List all available backups"),
                ("delete", "Delete a specific backup"),
            ],
        ),
        (
            "Configuration",
            &[
                ("setup", "Interactive configuration wizard"),
                ("schedule", "Configure auto-backup schedule"),
                ("daemon", "Run background scheduler (used by Termux:Boot)"),
                ("export-config", "Export config to a file (tokens redacted)"),
                ("import-config", "Import config from a file"),
            ],
        ),
        (
            "Maintenance",
            &[
                ("status", "Show current config and environment"),
                ("logs", "Show recent log entries"),
                ("clear-logs", "Clear / reset the log file"),
                ("check", "Show disk usage (~, $PREFIX, or a custom path)"),
                ("clear-cache", "Remove temporary and cached files"),
                ("help", "Show this help"),
                ("tui", "Launch the operations TUI"),
            ],
        ),
    ];
    for (group, items) in groups {
        print_line(&format!("  {group}"));
        for (name, description) in *items {
            print_line(&format!("    {name:<16} {description}"));
        }
        print_line("");
    }
    print_line("  Options");
    for (flag, description) in [
        ("-l, --label LABEL", "Label for this backup"),
        ("-n, --name NAME", "Backup name to restore / delete"),
        ("--lines N", "Lines to show in logs (default 40)"),
        (
            "--json-events",
            "Emit JSON progress events (used by the TUI)",
        ),
        (
            "--yes",
            "Skip delete / clear-logs / clear-cache confirmations",
        ),
        ("--debug", "Show the full traceback if something fails"),
        ("-v, --version", "Print version and exit"),
        ("-h, --help", "Show this help"),
    ] {
        print_line(&format!("    {flag:<24} {description}"));
    }
    print_line("");
    print_line("  Examples");
    for example in [
        "termux-sync setup",
        "termux-sync backup --label before-new-phone",
        "termux-sync list",
        "termux-sync restore",
        "termux-sync delete --name <backup-name>",
        "termux-sync logs --lines 100",
        "termux-sync check ~",
        "termux-sync import-config <file>",
    ] {
        print_line(&format!("    $ {example}"));
    }
    print_line("");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_rich_markup_tags() {
        assert_eq!(canonical_line("[bold green]✓[/bold green] done"), "✓ done");
    }

    #[test]
    fn replaces_box_drawing_and_collapses_spaces() {
        assert_eq!(canonical_line("  a ──  b │ c  "), "a b c");
    }

    #[test]
    fn leaves_unknown_brackets_alone() {
        assert_eq!(canonical_line("[note] keep"), "[note] keep");
    }
}
