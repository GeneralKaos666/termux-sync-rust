//! Package collection and reinstall.

use std::path::Path;
use std::process::Command;

use crate::core::events;
use crate::core::logging;

pub fn installed_packages() -> Result<Vec<String>, String> {
    installed_packages_with(Path::new("dpkg"))
}

fn installed_packages_with(program: &Path) -> Result<Vec<String>, String> {
    let output = Command::new(program)
        .arg("--get-selections")
        .output()
        .map_err(|error| format!("OSError: could not run dpkg: {error}"))?;
    if !output.status.success() {
        return Err(format!("RuntimeError: dpkg exited with {}", output.status));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut packages = Vec::new();
    for line in text.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 2 && parts[1] == "install" {
            packages.push(parts[0].to_string());
        }
    }
    Ok(packages)
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn failed_dpkg_is_an_error() {
        let temp = tempfile::tempdir().unwrap();
        let script = temp.path().join("dpkg");
        fs::write(&script, "#!/bin/sh\nexit 1\n").unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(installed_packages_with(&script).is_err());
    }
}
