use serde_json::json;
use termux_sync_core::core::backup;

fn main() -> Result<(), String> {
    let name = std::env::args().nth(1).unwrap_or_default();
    let cfg = json!({
        "storage": "local",
        "local_path": "~/termux-backups",
        "compression": "gz",
        "max_backups": 5
    });
    let _ = backup::restore(&cfg, &name)?;
    Ok(())
}
