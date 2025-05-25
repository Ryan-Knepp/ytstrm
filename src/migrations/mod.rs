use anyhow::Result;
use std::path::PathBuf;

mod config_to_v2;

pub fn run_migrations() -> Result<()> {
    let config_path = dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("/etc"))
        .join("ytstrm");
    if config_path.exists() {
        config_to_v2::migrate_config(&config_path)?;
    }

    Ok(())
}
