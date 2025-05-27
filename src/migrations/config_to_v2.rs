use crate::config::{Channel, Config, Source};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::SystemTime;
use tracing::info;

#[derive(Debug, Serialize, Deserialize)]
struct LegacyChannel {
    id: String,
    handle: String,
    name: String,
    max_videos: Option<usize>,
    max_age_days: Option<u32>,
    last_checked: SystemTime,
    media_dir: PathBuf,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ConfigV1 {
    channels: Vec<LegacyChannel>,
    check_interval: u64, // In minutes
    jellyfin_media_path: PathBuf,
    server_address: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ConfigV2 {
    channels: Vec<Channel>,
    check_interval: u64, // In minutes
    jellyfin_media_path: PathBuf,
    server_address: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ConfigV3 {
    channels: Vec<Channel>,
    check_interval: u64, // In minutes
    jellyfin_media_path: PathBuf,
    server_address: String,
    background_tasks_paused: bool,
}

pub fn migrate_config(config_dir: &PathBuf) -> Result<()> {
    info!("Migrating config from v1 to v2...");

    let config_path = config_dir.join("config.json");
    let content = std::fs::read_to_string(config_path)?;

    if let Ok(_) = serde_json::from_str::<Config>(&content) {
        info!("Config is already in proper format");
        return Ok(());
    }

    if let Ok(config_v3) = serde_json::from_str::<ConfigV3>(&content) {
        let new_config = Config {
            jellyfin_media_path: config_v3.jellyfin_media_path.clone(),
            server_address: config_v3.server_address.clone(),
            check_interval: config_v3.check_interval,
            channels: config_v3.channels,
            background_tasks_paused: config_v3.background_tasks_paused,
            maintain_manifest_cache: false,
        };
        new_config.save()?;
        info!("Successfully migrated config from v3 format");
        return Ok(());
    }

    if let Ok(config_v2) = serde_json::from_str::<ConfigV2>(&content) {
        let new_config = Config {
            jellyfin_media_path: config_v2.jellyfin_media_path.clone(),
            server_address: config_v2.server_address.clone(),
            check_interval: config_v2.check_interval,
            channels: config_v2.channels,
            background_tasks_paused: false,
            maintain_manifest_cache: false,
        };
        new_config.save()?;
        info!("Successfully migrated config from v2 format");
        return Ok(());
    }

    let old_config: ConfigV1 = serde_json::from_str(&content)?;
    let mut new_config = Config {
        jellyfin_media_path: old_config.jellyfin_media_path.clone(),
        server_address: old_config.server_address.clone(),
        check_interval: old_config.check_interval,
        channels: Vec::new(),
        background_tasks_paused: false,
        maintain_manifest_cache: false,
    };
    new_config.channels = old_config
        .channels
        .into_iter()
        .map(|channel| {
            let legacy: LegacyChannel =
                serde_json::from_value(serde_json::to_value(channel).unwrap())
                    .expect("Failed to convert to legacy channel format");

            Channel {
                id: legacy.handle.clone(),
                source: Source::Channel {
                    handle: legacy.handle,
                    name: legacy.name,
                    max_videos: legacy.max_videos,
                    max_age_days: legacy.max_age_days,
                },
                last_checked: legacy.last_checked,
                media_dir: legacy.media_dir,
            }
        })
        .collect();

    new_config.save()?;
    info!("Successfully migrated config to v2 format");

    Ok(())
}
