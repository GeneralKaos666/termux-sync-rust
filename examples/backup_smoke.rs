//! Smoke test for creating and listing a local backup.
use serde_json::json;
use termux_sync::core::backup;
use termux_sync::core::errors::CoreError;

fn main() -> Result<(), CoreError> {
    let cfg = json!({
        "storage": "local",
        "local_path": "~/termux-backups",
        "compression": "gz",
        "exclude_patterns": [".cache", "node_modules"],
        "max_backups": 5
    });
    backup::backup(&cfg, "smoke")?;
    backup::list(&cfg)?;
    Ok(())
}
