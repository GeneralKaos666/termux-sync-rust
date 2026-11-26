use termux_sync_core::core::errors::CoreError;
use termux_sync_core::core::{config, maintenance};

fn main() -> Result<(), CoreError> {
    let cfg = config::load_config()?;
    maintenance::status(&cfg)?;
    maintenance::logs(3)?;
    maintenance::export_config("out-config.json")?;
    maintenance::import_config("out-config.json")?;
    maintenance::clear_cache(true)?;
    Ok(())
}
