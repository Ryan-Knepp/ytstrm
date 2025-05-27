use anyhow::{Result, anyhow};
use reqwest::Client;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::process::Command;
use tracing::info;

use crate::ConfigState;

pub struct ManifestCache {
    pub video_id: String,
    pub content: String,
    pub expires: u64,
}

impl ManifestCache {
    pub fn new(video_id: &str, content: String) -> Self {
        // Extract expiration from manifest URL
        let expires = if let Some(exp) = content
            .lines()
            .find(|l| l.contains("expire/"))
            .and_then(|l| l.split("expire/").nth(1))
            .and_then(|l| l.split('/').next())
            .and_then(|exp| exp.parse().ok())
        {
            exp
        } else {
            // Default to 6 hours from now if we can't find expiration
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs()
                + (6 * 60 * 60)
        };

        Self {
            video_id: video_id.to_string(),
            content,
            expires,
        }
    }

    pub fn is_valid(&self) -> bool {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        // Consider it invalid 5 minutes before actual expiration
        self.expires > (now + 300)
    }

    pub fn save(&self, cache_dir: &Path) -> std::io::Result<()> {
        fs::create_dir_all(cache_dir)?;
        let path = cache_dir.join(format!("{}.m3u8", self.video_id));
        fs::write(path, &self.content)
    }

    pub fn save_original(&self, cache_dir: &Path) -> std::io::Result<()> {
        fs::create_dir_all(cache_dir)?;
        let path = cache_dir.join(format!("{}.original.m3u8", self.video_id));
        fs::write(path, &self.content)
    }

    pub fn load(video_id: &str, cache_dir: &Path) -> std::io::Result<Self> {
        let path = cache_dir.join(format!("{}.m3u8", video_id));
        let content = fs::read_to_string(path)?;
        Ok(Self::new(video_id, content))
    }
}

pub async fn fetch_and_filter_manifest(
    video_id: &str,
    cache_dir: &Path,
    save_cache: bool,
) -> Result<String> {
    let url = format!("https://www.youtube.com/watch?v={}", video_id);

    // Get video metadata as JSON
    let output = Command::new("yt-dlp")
        .args(["-j", "--no-playlist", "--cookies", "cookies.txt", &url])
        .output()
        .await
        .map_err(|e| anyhow!("Failed to execute yt-dlp: {}", e))?;

    // Check if yt-dlp succeeded and output isn't empty
    if !output.status.success() {
        return Err(anyhow!(
            "yt-dlp failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    if output.stdout.is_empty() {
        return Err(anyhow!("yt-dlp returned no data"));
    }

    // Debug log the output
    info!("yt-dlp stdout: {}", String::from_utf8_lossy(&output.stdout));
    if !output.stderr.is_empty() {
        info!("yt-dlp stderr: {}", String::from_utf8_lossy(&output.stderr));
    }

    let metadata: Value = serde_json::from_slice(&output.stdout).map_err(|e| {
        anyhow!(
            "Failed to parse metadata JSON: {} (stdout: {:?})",
            e,
            String::from_utf8_lossy(&output.stdout)
        )
    })?;

    // Get first manifest URL
    let manifest_url = metadata["formats"]
        .as_array()
        .and_then(|formats| {
            formats
                .iter()
                .find(|f| f["manifest_url"].is_string())
                .and_then(|f| f["manifest_url"].as_str())
        })
        .ok_or_else(|| anyhow!("No HLS manifest URL found"))?;

    info!("Found HLS manifest URL: {}", manifest_url);

    let client = Client::new();
    let content = client
        .get(manifest_url)
        .send()
        .await
        .map_err(|e| anyhow!("Failed to fetch manifest: {}", e))?
        .text()
        .await
        .map_err(|e| anyhow!("Failed to read manifest content: {}", e))?;

    if !content.contains("#EXTM3U") {
        return Err(anyhow!("Invalid manifest format"));
    }

    // Save original manifest if requested
    // if save_cache {
    //     let original_cache = ManifestCache::new(video_id, content.clone());
    //     if let Err(e) = original_cache.save_original(cache_dir) {
    //         info!("Failed to save original manifest: {}", e);
    //     }
    // }

    // Filter and modify the manifest
    let manifest = filter_and_modify_manifest(content);

    // Ensure manifest ends with newline
    let manifest = if !manifest.ends_with('\n') {
        format!("{}\n", manifest)
    } else {
        manifest
    };

    // Cache the filtered manifest if requested
    if save_cache {
        let cache = ManifestCache::new(video_id, manifest.clone());
        if let Err(e) = cache.save(cache_dir) {
            info!("Failed to cache manifest: {}", e);
        }
    }

    Ok(manifest)
}

pub fn filter_and_modify_manifest(content: String) -> String {
    let lines: Vec<&str> = content.lines().collect();
    let mut video_streams = Vec::new();
    let mut high_audio_default = None;
    let mut high_audio_backup = None;
    let mut sd_audio_default = None;
    let mut sd_audio_backup = None;

    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];

        if line.starts_with("#EXT-X-STREAM-INF:") {
            let info = line;
            let url = lines[i + 1];

            if let Some(bandwidth_str) = info
                .split("BANDWIDTH=")
                .nth(1)
                .and_then(|s| s.split(',').next())
            {
                if let Ok(bandwidth) = bandwidth_str.parse::<u32>() {
                    video_streams.push((bandwidth, info, url));
                }
            }
            i += 1; // Skip the URL line
        } else if line.starts_with("#EXT-X-MEDIA:") && line.contains("URI") {
            let is_default = line.contains("DEFAULT=YES");
            if line.contains("234") {
                if is_default {
                    high_audio_default = Some(line);
                } else if high_audio_default.is_none() {
                    high_audio_backup = Some(line);
                }
            } else if is_default {
                sd_audio_default = Some(line);
            } else if sd_audio_default.is_none() {
                sd_audio_backup = Some(line);
            }
        }
        i += 1;
    }

    // Sort streams by bandwidth (highest to lowest) and take top 3
    video_streams.sort_by(|a, b| b.0.cmp(&a.0));
    video_streams.truncate(3);

    // Build final manifest
    let mut final_manifest = String::from("#EXTM3U\n#EXT-X-INDEPENDENT-SEGMENTS\n");

    // Add audio track (using existing priority order)
    if let Some(audio) = high_audio_default
        .or(sd_audio_default)
        .or(high_audio_backup)
        .or(sd_audio_backup)
    {
        final_manifest.push_str(audio);
        final_manifest.push('\n');
    }

    // Add top 3 video streams
    for (_bandwidth, info, url) in video_streams {
        final_manifest.push_str(info);
        final_manifest.push('\n');
        final_manifest.push_str(url);
        final_manifest.push('\n');
    }

    final_manifest
}

#[derive(Clone)]
struct ManifestMaintenanceInfo {
    jellyfin_media_path: PathBuf,
}

pub async fn maintain_manifest_cache(config: ConfigState) {
    loop {
        // Get config info with minimal lock time
        let maintenance_info = {
            let config_guard = config.read().await;

            // Skip maintenance if no channels are configured
            if config_guard.channels.is_empty() {
                info!("No channels configured, skipping manifest maintenance");
                drop(config_guard);
                tokio::time::sleep(tokio::time::Duration::from_secs(900)).await;
                continue;
            }

            if config_guard.maintain_manifest_cache == false {
                info!("Manifest maintenance is disabled, skipping");
                drop(config_guard);
                tokio::time::sleep(tokio::time::Duration::from_secs(900)).await;
                continue;
            }

            ManifestMaintenanceInfo {
                jellyfin_media_path: config_guard.jellyfin_media_path.clone(),
            }
        };

        let cache_dir = maintenance_info.jellyfin_media_path.join("manifests");

        // Create manifests directory and .ignore file if they don't exist
        if let Err(e) = fs::create_dir_all(&cache_dir) {
            info!("Failed to create manifests directory: {}", e);
            continue;
        }

        let ignore_file = cache_dir.join(".ignore");
        if !ignore_file.exists() {
            if let Err(e) = fs::write(&ignore_file, "") {
                info!("Failed to create .ignore file: {}", e);
            }
        }

        if let Ok(files) = fs::read_dir(&cache_dir) {
            let mut count = 0;
            let mut files_count = 0;
            for file in files.flatten() {
                if let Some(file_name) = file.file_name().to_str() {
                    if !file_name.ends_with(".m3u8") {
                        continue;
                    }

                    let video_id = file_name.trim_end_matches(".m3u8");
                    if let Ok(cache) = ManifestCache::load(video_id, &cache_dir) {
                        files_count += 1;
                        let now = SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .unwrap()
                            .as_secs();

                        if cache.expires < (now + 1800) {
                            info!("Refreshing manifest for {}", video_id);
                            count += 1;
                            if let Err(e) =
                                fetch_and_filter_manifest(video_id, &cache_dir, true).await
                            {
                                info!("Failed to refresh manifest for {}: {}", video_id, e);
                            }
                            tokio::time::sleep(Duration::from_secs(15)).await;
                        }
                    }
                }
            }
            info!(
                "Checked {} manifest files, refreshed {} expired manifests",
                files_count, count
            );
        }

        tokio::time::sleep(tokio::time::Duration::from_secs(1800)).await;
    }
}
