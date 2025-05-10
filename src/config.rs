use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::SystemTime;
use std::{path::PathBuf, time::Duration};
use tokio::process::Command;
use tokio::sync::RwLock;
use tracing::{error, info};

use crate::manifest::fetch_and_filter_manifest;

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

pub struct VideoInfo {
    pub id: String,
    pub title: String,
    pub description: String,
    pub upload_date: String,
    pub thumbnail_url: String,
}

impl Channel {
    pub async fn scan_videos(&self) -> Result<Vec<VideoInfo>> {
        let output = Command::new("yt-dlp")
            .args([
                "--flat-playlist",
                "-j",
                "--dateafter",
                // If max_age_days is set, only get videos from that period
                &self
                    .max_age_days
                    .map(|days| format!("today-{}days", days))
                    .unwrap_or_else(|| "19700101".to_string()),
                "--cookies",
                "cookies.txt",
                &format!("https://www.youtube.com/{}", self.handle),
            ])
            .output()
            .await
            .map_err(|e| anyhow!("Failed to execute yt-dlp: {}", e))?;

        let mut videos: Vec<VideoInfo> = output
            .stdout
            .split(|&b| b == b'\n')
            .filter(|line| !line.is_empty())
            .filter_map(|line| {
                serde_json::from_slice::<serde_json::Value>(line)
                    .ok()
                    .and_then(|v| {
                        // Parse upload date
                        let upload_date = v["upload_date"].as_str()?;

                        Some(VideoInfo {
                            id: v["id"].as_str()?.to_string(),
                            title: v["title"].as_str()?.to_string(),
                            description: v["description"].as_str()?.to_string(),
                            upload_date: upload_date.to_string(),
                            thumbnail_url: v["thumbnail"].as_str()?.to_string(),
                        })
                    })
            })
            .collect();

        // Sort by upload date (newest first)
        videos.sort_by(|a, b| b.upload_date.cmp(&a.upload_date));

        // Limit number of videos if max_videos is set
        if let Some(max_videos) = self.max_videos {
            videos.truncate(max_videos);
        }

        if videos.is_empty() {
            return Err(anyhow!("No videos found for channel {}", self.handle));
        }

        Ok(videos)
    }

    fn get_season_from_date(&self, upload_date: &str) -> Result<u32> {
        // upload_date format: YYYYMMDD
        upload_date
            .get(0..4)
            .and_then(|year| year.parse().ok())
            .ok_or_else(|| anyhow!("Invalid upload date format"))
    }

    fn create_channel_structure(&self) -> Result<()> {
        // Create main channel directory if it doesn't exist
        std::fs::create_dir_all(&self.media_dir)?;

        // Create channel NFO file
        let channel_nfo = format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<tvshow>
    <title>{}</title>
    <plot>Videos from YouTube channel {}</plot>
</tvshow>"#,
            self.name, self.handle
        );
        std::fs::write(self.media_dir.join("tvshow.nfo"), channel_nfo)
            .map_err(|e| anyhow!("Failed to write channel NFO file: {}", e))?;

        Ok(())
    }

    pub async fn create_media_files(&self, video: &VideoInfo, config: &Config) -> Result<()> {
        // Ensure channel structure exists
        self.create_channel_structure()?;

        // Get season number from upload date
        let season = self.get_season_from_date(&video.upload_date)?;
        let season_dir = self.media_dir.join(format!("Season {}", season));
        std::fs::create_dir_all(&season_dir)
            .map_err(|e| anyhow!("Failed to create season directory: {}", e))?;

        // Create episode filename from upload date and title
        // Format: YYYYMMDD - Title.strm
        let episode_base = format!("{} - {}", video.upload_date, video.title);
        let safe_filename = episode_base
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == ' ' {
                    c
                } else {
                    '_'
                }
            })
            .collect::<String>();

        // Create episode NFO
        let nfo_content = format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<episodedetails>
    <title>{}</title>
    <aired>{}</aired>
    <premiered>{}</premiered>
    <plot>{}</plot>
    <thumb>{}</thumb>
</episodedetails>"#,
            video.title,
            video.upload_date,
            video.upload_date,
            video.description,
            video.thumbnail_url
        );
        std::fs::write(
            season_dir.join(format!("{}.nfo", safe_filename)),
            nfo_content,
        )
        .map_err(|e| anyhow!("Failed to write episode NFO: {}", e))?;

        // Create STRM file
        let strm_content = format!("http://{}/stream/{}", config.server_address, video.id);
        std::fs::write(
            season_dir.join(format!("{}.strm", safe_filename)),
            strm_content,
        )
        .map_err(|e| anyhow!("Failed to write STRM file: {}", e))?;

        // Download channel thumbnail if it doesn't exist
        let poster_path = self.media_dir.join("poster.jpg");
        if !poster_path.exists() {
            let client = reqwest::Client::new();
            let img_bytes = client
                .get(&video.thumbnail_url)
                .send()
                .await
                .map_err(|e| anyhow!("Failed to fetch thumbnail: {}", e))?
                .bytes()
                .await
                .map_err(|e| anyhow!("Failed to read thumbnail bytes: {}", e))?;

            std::fs::write(&poster_path, img_bytes)
                .map_err(|e| anyhow!("Failed to write channel poster: {}", e))?;
        }

        // Pre-cache manifest
        self.cache_manifest(video.id.as_str(), &config.jellyfin_media_path)
            .await
            .map_err(|e| anyhow!("Failed to cache manifest: {}", e))?;

        Ok(())
    }

    async fn cache_manifest(&self, video_id: &str, cache_dir: &PathBuf) -> Result<()> {
        fetch_and_filter_manifest(video_id, cache_dir, true)
            .await
            .map(|_| ())
    }
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

pub async fn check_channels(config: Arc<RwLock<Config>>) -> Result<()> {
    loop {
        let config_guard = config.read().await;
        info!(
            "Checking {} channels for new videos",
            config_guard.channels.len()
        );

        for channel in &config_guard.channels {
            match channel.scan_videos().await {
                Ok(videos) => {
                    for video in videos {
                        // Get season directory from upload date
                        if let Ok(season) = channel.get_season_from_date(&video.upload_date) {
                            let season_dir = channel.media_dir.join(format!("Season {}", season));
                            let episode_base = format!("{} - {}", video.upload_date, video.title);
                            let safe_filename = episode_base
                                .chars()
                                .map(|c| {
                                    if c.is_ascii_alphanumeric() || c == '-' || c == ' ' {
                                        c
                                    } else {
                                        '_'
                                    }
                                })
                                .collect::<String>();

                            // Check if video exists in its season directory
                            if !season_dir.join(format!("{}.strm", safe_filename)).exists() {
                                info!("New video found: {} ({})", video.title, video.id);
                                if let Err(e) =
                                    channel.create_media_files(&video, &config_guard).await
                                {
                                    error!("Failed to create media files: {}", e);
                                }
                            }
                        }
                    }
                }
                Err(e) => error!("Failed to scan channel {}: {}", channel.handle, e),
            }
        }
        let sleep_duration = config_guard.check_interval * 60;
        drop(config_guard);
        tokio::time::sleep(Duration::from_secs(sleep_duration)).await;
    }
}
