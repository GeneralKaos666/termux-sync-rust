//! Package collection and reinstall.

use std::process::Command;

use crate::core::events;
use crate::core::logging;

pub fn installed_packages() -> Vec<String> {
    let Ok(output) = Command::new("dpkg").arg("--get-selections").output() else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut packages = Vec::new();
    for line in text.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 2 && parts[1] == "install" {
            packages.push(parts[0].to_string());
        }
    }
    packages
}

fn valid_name(package: &str) -> bool {
    package
        .chars()
        .next()
        .map(|ch| ch.is_ascii_alphanumeric())
        .unwrap_or(false)
        && package
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '+' | '.' | '_' | '-'))
}

pub fn install(packages: &[String]) -> Vec<String> {
    if packages.is_empty() {
        return Vec::new();
    }
    let mut failed = Vec::new();
    let total = packages.len();
    for (index, package) in packages.iter().enumerate() {
        events::emit(
            "progress",
            vec![
                ("stage", serde_json::json!("packages")),
                ("current", serde_json::json!(index + 1)),
                ("total", serde_json::json!(total)),
                ("unit", serde_json::json!("packages")),
                ("detail", serde_json::json!(package)),
            ],
        );
        if !valid_name(package) {
            logging::log(
                "WARNING",
                &format!("Skipped invalid package name in manifest: {package:?}"),
            );
            failed.push(package.clone());
            continue;
        }
        let ok = Command::new("pkg")
            .args(["install", "-y", "--", package])
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false);
        if !ok {
            failed.push(package.clone());
        }
    }
    failed
}
