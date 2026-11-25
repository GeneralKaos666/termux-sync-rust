use serde_json::json;
use termux_sync_core::core::backup;

fn main() -> Result<(), String> {
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
