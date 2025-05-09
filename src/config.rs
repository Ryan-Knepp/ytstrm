use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::SystemTime;
use tracing::info;

#[derive(Debug, Serialize, Deserialize)]
pub struct Channel {
    pub id: String,
    pub handle: String,
    pub name: String,
    pub last_checked: SystemTime,
    pub media_dir: PathBuf,
    pub max_videos: Option<usize>, // Maximum number of videos to keep
    pub max_age_days: Option<u32>, // Maximum age of videos in days
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Config {
    pub channels: Vec<Channel>,
    pub check_interval: u64, // In minutes
    pub jellyfin_media_path: PathBuf,
    pub server_address: String,
}

impl Config {
    pub fn load() -> Result<Self> {
        let config_dir = dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("/etc"))
            .join("ytstrm");
        std::fs::create_dir_all(&config_dir)
            .map_err(|e| anyhow!("Failed to create config directory: {}", e))?;

        let config_path = config_dir.join("config.json");
        if !config_path.exists() {
            let default_config = Config {
                channels: Vec::new(),
                check_interval: 240, // 4 hours in minutes
                jellyfin_media_path: PathBuf::from("/media/youtube"),
                server_address: String::from("localhost:8080"),
            };
            let json = serde_json::to_string_pretty(&default_config)
                .map_err(|e| anyhow!("Failed to serialize default config: {}", e))?;
            std::fs::write(&config_path, json)
                .map_err(|e| anyhow!("Failed to write default config: {}", e))?;
            info!("Created default config at {:?}", config_path);
            return Ok(default_config);
        }

        let content = std::fs::read_to_string(&config_path)
            .map_err(|e| anyhow!("Failed to read config file: {}", e))?;
        serde_json::from_str(&content).map_err(|e| anyhow!("Failed to parse config file: {}", e))
    }

    pub fn save(&self) -> Result<()> {
        let config_path = dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("/etc"))
            .join("ytstrm/config.json");
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| anyhow!("Failed to serialize config: {}", e))?;
        std::fs::write(&config_path, json)
            .map_err(|e| anyhow!("Failed to write config file: {}", e))?;
        Ok(())
    }
}
