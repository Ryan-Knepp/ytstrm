use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::SystemTime;
use std::{path::PathBuf, time::Duration};
use tokio::process::Command;
use tokio::sync::RwLock;
use tracing::{error, info};

use crate::ConfigState;
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

#[derive(Debug)]
pub struct ChannelImages {
    pub landscape: Option<String>,
    pub poster: Option<String>,
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
        let handle = self.handle.trim_start_matches('@');
        let url = format!("https://www.youtube.com/@{}/videos", handle);
        info!("Fetching videos from URL: {}", url);

        let output = Command::new("yt-dlp")
            .args([
                "--compat-options",
                "no-youtube-channel-redirect",
                "--compat-options",
                "no-youtube-unavailable-videos",
                "--dateafter",
                &self
                    .max_age_days
                    .map(|days| format!("today-{}days", days))
                    .unwrap_or_else(|| "19700101".to_string()),
                "--playlist-start",
                "1",
                "--playlist-end",
                &self.max_videos.unwrap_or(50).to_string(),
                "--no-warnings",
                "--dump-json", // Changed from -j to --dump-json
                "--cookies",
                "cookies.txt",
                &url,
            ])
            .output()
            .await
            .map_err(|e| anyhow!("Failed to execute yt-dlp: {}", e))?;

        // Save output for debugging
        let debug_dir = PathBuf::from("debug");
        std::fs::create_dir_all(&debug_dir)?;
        std::fs::write(
            debug_dir.join(format!("{}_video_list.json", self.handle)),
            &output.stdout,
        )?;

        if !output.status.success() {
            std::fs::write(
                debug_dir.join(format!("{}_video_list_error.txt", self.handle)),
                &output.stderr,
            )?;
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!("yt-dlp failed: {}", stderr));
        }

        let mut videos: Vec<VideoInfo> = output
            .stdout
            .split(|&b| b == b'\n')
            .filter(|line| !line.is_empty())
            .filter_map(|line| {
                serde_json::from_slice::<serde_json::Value>(line)
                    .ok()
                    .and_then(|v| {
                        let upload_date = v["upload_date"].as_str()?;

                        // Get only the first paragraph of the description
                        let full_description = v["description"].as_str()?.trim();
                        let description = full_description
                            .split('\n')
                            .next()
                            .unwrap_or("")
                            .trim()
                            .to_string();

                        Some(VideoInfo {
                            id: v["id"].as_str()?.to_string(),
                            title: v["title"].as_str()?.to_string(),
                            description, // Now using only first paragraph
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

    pub fn get_season_from_date(&self, upload_date: &str) -> Result<u32> {
        // upload_date format: YYYYMMDD
        upload_date
            .get(0..4)
            .and_then(|year| year.parse().ok())
            .ok_or_else(|| anyhow!("Invalid upload date format"))
    }

    pub async fn get_channel_images(&self) -> Result<ChannelImages> {
        let handle = self.handle.trim_start_matches('@');
        let channel_url = format!("https://www.youtube.com/@{}", handle);

        let output = Command::new("yt-dlp")
            .args([
                "--list-thumbnails",
                "--restrict-filenames",
                "--ignore-errors",
                "--no-warnings",
                "--playlist-items",
                "0",
                &channel_url,
            ])
            .output()
            .await
            .map_err(|e| anyhow!("Failed to execute yt-dlp: {}", e))?;

        let output_str = String::from_utf8_lossy(&output.stdout);

        // Save output for debugging
        info!("yt-dlp output:\n{}", output_str);

        let mut poster = None;
        let mut landscape = None;

        for line in output_str.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 4 {
                // Find the URL part - it's always the last part
                if let Some(url) = parts.last() {
                    if parts[0] == "avatar_uncropped" {
                        poster = Some(url.to_string());
                    } else if parts[0] == "banner_uncropped" {
                        landscape = Some(url.to_string());
                    }
                }
            }
        }

        info!("Found poster URL: {:?}", poster);
        info!("Found landscape URL: {:?}", landscape);

        Ok(ChannelImages { landscape, poster })
    }

    async fn create_channel_structure(&self) -> Result<()> {
        // Create main channel directory if it doesn't exist
        std::fs::create_dir_all(&self.media_dir)?;

        // Get channel images
        if let Ok(images) = self.get_channel_images().await {
            // Download poster
            if let Some(poster_url) = images.poster {
                let client = reqwest::Client::new();
                if let Ok(response) = client.get(&poster_url).send().await {
                    if let Ok(bytes) = response.bytes().await {
                        let _ = std::fs::write(self.media_dir.join("poster.jpg"), bytes);
                    }
                }
            }

            // Download landscape/banner
            if let Some(landscape_url) = images.landscape {
                let client = reqwest::Client::new();
                if let Ok(response) = client.get(&landscape_url).send().await {
                    if let Ok(bytes) = response.bytes().await {
                        let _ = std::fs::write(self.media_dir.join("landscape.jpg"), bytes);
                    }
                }
            }
        }

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
        self.create_channel_structure().await?;

        // Get season number from upload date
        let season = self.get_season_from_date(&video.upload_date)?;
        let season_dir = self.media_dir.join(format!("Season {}", season));
        std::fs::create_dir_all(&season_dir)
            .map_err(|e| anyhow!("Failed to create season directory: {}", e))?;

        // Parse upload date (YYYYMMDD) into SYearEMonthDay format
        let year = &video.upload_date[0..4];
        let month = &video.upload_date[4..6];
        let day = &video.upload_date[6..8];
        let episode_prefix = format!("S{}E{}{}", year, month, day);

        // Create episode filename from upload date and title
        // Format: YYYYMMDD - Title.strm
        let episode_base = format!("{} - {}", episode_prefix, video.title);
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

        // Create episode image file
        let client = reqwest::Client::new();
        let img_bytes = client
            .get(&video.thumbnail_url)
            .send()
            .await
            .map_err(|e| anyhow!("Failed to fetch episode thumbnail: {}", e))?
            .bytes()
            .await
            .map_err(|e| anyhow!("Failed to read thumbnail bytes: {}", e))?;

        std::fs::write(
            season_dir.join(format!("{}-thumb.jpg", safe_filename)),
            img_bytes,
        )
        .map_err(|e| anyhow!("Failed to write episode thumbnail: {}", e))?;

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
        let strm_content = format!(
            "http://{}/stream/{}",
            config.server_address.trim_start_matches("http://"),
            video.id
        );
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
        self.cache_manifest(video.id.as_str(), &config)
            .await
            .map_err(|e| anyhow!("Failed to cache manifest: {}", e))?;

        Ok(())
    }

    pub async fn cache_manifest(&self, video_id: &str, config: &Config) -> Result<()> {
        let manifests_dir = PathBuf::from(&config.jellyfin_media_path).join("manifests");
        fetch_and_filter_manifest(video_id, &manifests_dir, true)
            .await
            .map(|_| ())
    }

    pub async fn process_new_videos(&self, config: &Config) -> Result<usize> {
        let mut new_videos = 0;
        let videos = self.scan_videos().await?;

        for video in videos {
            let season = self.get_season_from_date(&video.upload_date)?;
            let season_dir = self.media_dir.join(format!("Season {}", season));
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

            if !season_dir.join(format!("{}.strm", safe_filename)).exists() {
                self.create_media_files(&video, config).await?;
                new_videos += 1;
            }

            // Cache manifest regardless of whether the video is new
            self.cache_manifest(&video.id, &config).await?;
        }

        Ok(new_videos)
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

pub async fn check_channels(config: ConfigState) -> Result<()> {
    loop {
        let config_guard = config.read().await;
        info!(
            "Checking {} channels for new videos",
            config_guard.channels.len()
        );

        for channel in &config_guard.channels {
            match channel.process_new_videos(&config_guard).await {
                Ok(count) => {
                    if count > 0 {
                        info!("Added {} new videos for channel {}", count, channel.handle);
                    }
                }
                Err(e) => error!("Failed to process channel {}: {}", channel.handle, e),
            }
        }

        let sleep_duration = config_guard.check_interval * 60;
        drop(config_guard);
        tokio::time::sleep(Duration::from_secs(sleep_duration)).await;
    }
}
