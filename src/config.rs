use anyhow::{Result, anyhow};
use chrono::{DateTime, TimeZone};
use serde::{Deserialize, Serialize};
use std::time::SystemTime;
use std::{path::PathBuf, time::Duration};
use tokio::process::Command;
use tracing::{error, info};

use crate::ConfigState;
use crate::manifest::fetch_and_filter_manifest;

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "type")]
pub enum Source {
    Channel {
        handle: String,
        name: String,
        max_videos: Option<usize>,
        max_age_days: Option<u32>,
    },
    Playlist {
        id: String,
        name: String,
    },
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Channel {
    pub id: String,
    pub source: Source,
    pub last_checked: SystemTime,
    pub media_dir: PathBuf,
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
    pub background_tasks_paused: bool,
    pub maintain_manifest_cache: bool,
}

pub struct VideoInfo {
    pub id: String,
    pub title: String,
    pub description: String,
    pub upload_date: String,
    pub thumbnail_url: String,
}

impl Channel {
    pub async fn process_new_videos(
        &self,
        jellyfin_media_path: &PathBuf,
        server_address: &str,
        config_state: &ConfigState,
    ) -> Result<usize> {
        // Create channel structure once before processing videos
        self.create_channel_structure().await?;

        let videos = self.scan_videos().await?;
        let mut new_videos = 0;

        for video in &videos {
            match self
                .process_video(video, jellyfin_media_path, server_address)
                .await
            {
                Ok(true) => {
                    new_videos += 1;
                    tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                }
                Ok(false) => {} // Video already exists
                Err(e) => error!("Failed to process video {}: {}", video.id, e),
            }
        }

        info!(
            "Processed {} new videos for channel {}",
            new_videos,
            self.get_name()
        );

        // Always update last_checked time
        let mut config = config_state.write().await;
        if let Some(channel) = config.channels.iter_mut().find(|c| c.id == self.id) {
            let now = chrono::Utc::now();
            channel.last_checked = SystemTime::from(now);
            config.save()?;
        }

        Ok(new_videos)
    }

    pub async fn scan_videos(&self) -> Result<Vec<VideoInfo>> {
        let url = self.get_url("videos");

        info!("Fetching videos from URL: {}", url);

        let mut args = vec![
            "--compat-options".to_string(),
            "no-youtube-channel-redirect".to_string(),
            "--compat-options".to_string(),
            "no-youtube-unavailable-videos".to_string(),
            "--no-warnings".to_string(),
            "--dump-json".to_string(),
            "--ignore-errors".to_string(),
            "--cookies".to_string(),
            "cookies.txt".to_string(),
            "--sleep-interval".to_string(),
            "8".to_string(), // 8 seconds between requests
            "--max-sleep-interval".to_string(),
            "60".to_string(), // Up to 1 minute if rate limited
            "--sleep-subtitles".to_string(),
            "5".to_string(), // 5 seconds between subtitle requests
            "--retries".to_string(),
            "infinite".to_string(), // Keep retrying on rate limit
        ];

        // Set date filtering based on last_checked for both channels and playlists
        let mut date_after = None;

        // Check last_checked date (minus 2 days for safety)
        if let Ok(duration) = self.last_checked.elapsed() {
            if duration.as_secs() > 0 {
                let last_check_date = chrono::DateTime::from(self.last_checked);
                date_after = Some(last_check_date - chrono::Duration::days(2));
            }
        }

        // For channels, also consider max_age_days
        if let Source::Channel { max_age_days, .. } = &self.source {
            if let Some(days) = max_age_days {
                let now = chrono::Utc::now();
                let max_age_date = now - chrono::Duration::days(*days as i64);

                // Use max_age_date if it's more recent than last_checked
                if let Some(current_date) = date_after {
                    if max_age_date > current_date {
                        date_after = Some(max_age_date);
                    }
                } else {
                    date_after = Some(max_age_date);
                }
            }
        }

        // Add the date filter if we have one
        if let Some(date) = date_after {
            args.push("--dateafter".to_string());
            args.push(date.format("%Y%m%d").to_string());
        }

        // Apply max_videos limit for channels
        if let Source::Channel { max_videos, .. } = &self.source {
            if let Some(count) = max_videos {
                args.push("--playlist-start".to_string());
                args.push("1".to_string());
                args.push("--playlist-end".to_string());
                args.push(count.to_string());
            }
        }

        args.push(url);

        // print out the command for debugging
        info!("Executing yt-dlp with args: {:?}", args);

        let output = Command::new("yt-dlp")
            .args(&args)
            .output()
            .await
            .map_err(|e| anyhow!("Failed to execute yt-dlp: {}", e))?;

        // Save output for debugging
        // let debug_dir = PathBuf::from("debug");
        // std::fs::create_dir_all(&debug_dir)?;
        // std::fs::write(
        //     debug_dir.join(format!("{}_video_list.json", self.get_handle_or_id())),
        //     &output.stdout,
        // )?;

        // Save errors for debugging but don't fail
        if !output.stderr.is_empty() {
            // std::fs::write(
            //     debug_dir.join(format!("{}_video_list_error.txt", self.get_handle_or_id())),
            //     &output.stderr,
            // )?;
            info!(
                "Some videos were skipped: {}",
                String::from_utf8_lossy(&output.stderr)
            );
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
        if let Source::Channel { max_videos, .. } = &self.source {
            if let Some(max_videos) = max_videos {
                videos.truncate(*max_videos);
            }
        }

        if videos.is_empty() {
            return Err(anyhow!("No videos found for channel {}", self.get_name()));
        }

        Ok(videos)
    }

    pub fn get_name(&self) -> &str {
        match &self.source {
            Source::Channel { name, .. } => name,
            Source::Playlist { name, .. } => name,
        }
    }

    pub fn get_handle_or_id(&self) -> &str {
        match &self.source {
            Source::Channel { handle, .. } => handle,
            Source::Playlist { id, .. } => id,
        }
    }

    pub fn get_url(&self, command_type: &str) -> String {
        match &self.source {
            Source::Channel { handle, .. } => {
                let handle = handle.trim_start_matches('@');
                match command_type {
                    "videos" => format!("https://www.youtube.com/@{}/videos", handle),
                    "channel" => format!("https://www.youtube.com/@{}", handle),
                    _ => panic!("Invalid command type"),
                }
            }
            Source::Playlist { id, .. } => {
                format!("https://www.youtube.com/playlist?list={}", id)
            }
        }
    }

    pub fn get_season_from_date(&self, upload_date: &str) -> Result<u32> {
        // upload_date format: YYYYMMDD
        upload_date
            .get(0..4)
            .and_then(|year| year.parse().ok())
            .ok_or_else(|| anyhow!("Invalid upload date format"))
    }

    pub async fn get_channel_images(&self) -> Result<ChannelImages> {
        let url = match &self.source {
            Source::Channel { .. } => self.get_url("channel"),
            Source::Playlist { id, .. } => format!("https://www.youtube.com/playlist?list={}", id),
        };

        let output = Command::new("yt-dlp")
            .args([
                "--list-thumbnails",
                "--restrict-filenames",
                "--ignore-errors",
                "--no-warnings",
                "--playlist-items",
                "0",
                &url,
            ])
            .output()
            .await
            .map_err(|e| anyhow!("Failed to execute yt-dlp: {}", e))?;

        let output_str = String::from_utf8_lossy(&output.stdout);

        // Save output for debugging
        // info!("yt-dlp output:\n{}", output_str);

        let mut poster = None;
        let mut landscape = None;

        // Parse thumbnail lines
        for line in output_str.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 4 {
                if let Some(url) = parts.last() {
                    match &self.source {
                        Source::Channel { .. } => {
                            // Channel image logic
                            if parts[0] == "avatar_uncropped" {
                                poster = Some(url.to_string());
                            } else if parts[0] == "banner_uncropped" {
                                landscape = Some(url.to_string());
                            }
                        }
                        Source::Playlist { .. } => {
                            // For playlists, use the highest resolution thumbnail
                            if let Ok(width) = parts[1].parse::<u32>() {
                                if width >= 1280 {
                                    poster = Some(url.to_string());
                                    landscape = Some(url.to_string());
                                }
                            }
                        }
                    }
                }
            }
        }

        // info!("Found poster URL: {:?}", poster);
        // info!("Found landscape URL: {:?}", landscape);

        Ok(ChannelImages { landscape, poster })
    }

    fn create_safe_filename(&self, base: &str) -> String {
        base.chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == ' ' {
                    c
                } else {
                    '_'
                }
            })
            .collect()
    }

    async fn download_image(&self, url: &str) -> Result<Vec<u8>> {
        let client = reqwest::Client::new();
        client
            .get(url)
            .send()
            .await
            .map_err(|e| anyhow!("Failed to fetch image: {}", e))?
            .bytes()
            .await
            .map(|b| b.to_vec())
            .map_err(|e| anyhow!("Failed to read image bytes: {}", e))
    }

    fn write_file(&self, path: PathBuf, content: impl AsRef<[u8]>) -> Result<()> {
        std::fs::write(&path, content)
            .map_err(|e| anyhow!("Failed to write file {}: {}", path.display(), e))
    }

    async fn process_video(
        &self,
        video: &VideoInfo,
        jellyfin_media_path: &PathBuf,
        server_address: &str,
    ) -> Result<bool> {
        // Get season info and create directory
        let season = self.get_season_from_date(&video.upload_date)?;
        let season_dir = self.media_dir.join(format!("Season {}", season));

        // Create base filename
        let episode_base = format!("{} - {}", video.upload_date, video.title);
        let safe_filename = self.create_safe_filename(&episode_base);

        // Check if video already exists
        if season_dir.join(format!("{}.strm", safe_filename)).exists() {
            return Ok(false);
        }

        // Create season directory
        std::fs::create_dir_all(&season_dir)
            .map_err(|e| anyhow!("Failed to create season directory: {}", e))?;

        // Download and save thumbnail
        let img_bytes = self.download_image(&video.thumbnail_url).await?;
        self.write_file(
            season_dir.join(format!("{}-thumb.jpg", safe_filename)),
            img_bytes,
        )?;

        // Create episode NFO
        let nfo_content = self.create_episode_nfo(video)?;
        self.write_file(
            season_dir.join(format!("{}.nfo", safe_filename)),
            nfo_content,
        )?;

        // Create STRM file
        let strm_content = format!(
            "http://{}/stream/{}",
            server_address.trim_start_matches("http://"),
            video.id
        );
        self.write_file(
            season_dir.join(format!("{}.strm", safe_filename)),
            strm_content,
        )?;

        // Pre-cache manifest
        let manifests_dir = PathBuf::from(jellyfin_media_path).join("manifests");
        fetch_and_filter_manifest(&video.id, &manifests_dir, true).await?;

        Ok(true)
    }

    fn create_episode_nfo(&self, video: &VideoInfo) -> Result<String> {
        Ok(format!(
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
        ))
    }

    async fn create_channel_structure(&self) -> Result<()> {
        // Create main channel directory
        std::fs::create_dir_all(&self.media_dir)?;

        // Handle channel images
        if let Ok(images) = self.get_channel_images().await {
            if let Some(poster_url) = images.poster {
                if let Ok(bytes) = self.download_image(&poster_url).await {
                    let _ = self.write_file(self.media_dir.join("poster.jpg"), bytes);
                }
            }
            if let Some(landscape_url) = images.landscape {
                if let Ok(bytes) = self.download_image(&landscape_url).await {
                    let _ = self.write_file(self.media_dir.join("landscape.jpg"), bytes);
                }
            }
        }

        // Create channel NFO
        let channel_nfo = match &self.source {
            Source::Channel { name, handle, .. } => format!(
                r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
    <tvshow>
        <title>{}</title>
        <plot>Videos from YouTube channel {}</plot>
    </tvshow>"#,
                name, handle
            ),
            Source::Playlist { name, .. } => format!(
                r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
    <tvshow>
        <title>{}</title>
        <plot>Videos from YouTube playlist</plot>
    </tvshow>"#,
                name
            ),
        };

        self.write_file(self.media_dir.join("tvshow.nfo"), channel_nfo)
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
                background_tasks_paused: false,
                maintain_manifest_cache: false,
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

    pub fn set_background_tasks_paused(&mut self, paused: bool) -> Result<()> {
        self.background_tasks_paused = paused;
        self.save()
    }

    pub fn set_maintain_manifest_cache(&mut self, enabled: bool) -> Result<()> {
        self.maintain_manifest_cache = enabled;
        self.save()
    }
}

#[derive(Clone)]
struct ChannelCheckInfo {
    name: String,
    channel: Channel,
    jellyfin_media_path: PathBuf,
    server_address: String,
}

pub async fn check_channels(config: ConfigState) -> Result<()> {
    loop {
        // Get channels and config info with minimal lock time
        let check_info: Vec<ChannelCheckInfo> = {
            let config_guard = config.read().await;
            if config_guard.background_tasks_paused {
                info!("Background tasks are paused, sleeping for 10 minutes");
                drop(config_guard);
                tokio::time::sleep(Duration::from_secs(600)).await;
                continue;
            }
            config_guard
                .channels
                .iter()
                .map(|channel| ChannelCheckInfo {
                    name: channel.get_name().to_string(),
                    channel: channel.clone(),
                    jellyfin_media_path: config_guard.jellyfin_media_path.clone(),
                    server_address: config_guard.server_address.clone(),
                })
                .collect()
        };

        info!("Checking {} channels for new videos", check_info.len());

        // Process each channel with temporary config
        for info in check_info {
            let temp_config = Config {
                channels: vec![],  // Not needed for processing
                check_interval: 0, // Not needed for processing
                jellyfin_media_path: info.jellyfin_media_path,
                server_address: info.server_address,
                background_tasks_paused: false, // Not needed for processing
                maintain_manifest_cache: false, // Not needed for processing
            };

            match info
                .channel
                .process_new_videos(
                    &temp_config.jellyfin_media_path,
                    &temp_config.server_address,
                    &config,
                )
                .await
            {
                Ok(count) => {
                    if count > 0 {
                        info!("Added {} new videos for channel {}", count, info.name);
                    }
                }
                Err(e) => error!("Failed to process channel {}: {}", info.name, e),
            }
        }

        // Get sleep duration with minimal lock time
        let sleep_duration = {
            let config_guard = config.read().await;
            config_guard.check_interval * 60
        };

        tokio::time::sleep(Duration::from_secs(sleep_duration)).await;
    }
}
