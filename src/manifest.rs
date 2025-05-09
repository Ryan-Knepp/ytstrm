use reqwest::Client;
use serde_json::Value;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::process::Command;
use tracing::info;

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

    pub fn save(&self, cache_dir: &PathBuf) -> std::io::Result<()> {
        fs::create_dir_all(cache_dir)?;
        let path = cache_dir.join(format!("{}.m3u8", self.video_id));
        fs::write(path, &self.content)
    }

    pub fn save_original(&self, cache_dir: &PathBuf) -> std::io::Result<()> {
        fs::create_dir_all(cache_dir)?;
        let path = cache_dir.join(format!("{}.original.m3u8", self.video_id));
        fs::write(path, &self.content)
    }

    pub fn load(video_id: &str, cache_dir: &PathBuf) -> std::io::Result<Self> {
        let path = cache_dir.join(format!("{}.m3u8", video_id));
        let content = fs::read_to_string(path)?;
        Ok(Self::new(video_id, content))
    }
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
            } else {
                if is_default {
                    sd_audio_default = Some(line);
                } else if sd_audio_default.is_none() {
                    sd_audio_backup = Some(line);
                }
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

pub async fn maintain_manifest_cache() {
    let cache_dir = dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("ytstrm/manifests");

    loop {
        info!("Starting manifest cache maintenance...");

        // Read all cache files
        if let Ok(entries) = fs::read_dir(&cache_dir) {
            for entry in entries.flatten() {
                if let Some(file_name) = entry.file_name().to_str() {
                    if !file_name.ends_with(".m3u8") {
                        continue;
                    }

                    let video_id = file_name.trim_end_matches(".m3u8");

                    // Check if manifest is near expiration
                    if let Ok(cache) = ManifestCache::load(video_id, &cache_dir) {
                        let now = SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .unwrap()
                            .as_secs();

                        // Refresh if expires within 30 minutes
                        if cache.expires < (now + 1800) {
                            info!("Refreshing manifest for {}", video_id);

                            // Reuse stream_youtube logic but only for manifest fetching
                            let url = format!("https://www.youtube.com/watch?v={}", video_id);

                            match refresh_manifest(video_id, &url, &cache_dir).await {
                                Ok(_) => info!("Successfully refreshed manifest for {}", video_id),
                                Err(e) => {
                                    info!("Failed to refresh manifest for {}: {}", video_id, e)
                                }
                            }
                        }
                    }
                }
            }
        }

        // Sleep for 15 minutes before next check
        tokio::time::sleep(tokio::time::Duration::from_secs(900)).await;
    }
}

async fn refresh_manifest(
    video_id: &str,
    url: &str,
    cache_dir: &PathBuf,
) -> Result<(), Box<dyn std::error::Error>> {
    let output = Command::new("yt-dlp")
        .args(["-j", "--no-playlist", "--cookies", "cookies.txt", url])
        .output()
        .await?;

    let metadata: Value = serde_json::from_slice(&output.stdout)?;

    if let Some(manifest_url) = metadata["formats"].as_array().and_then(|formats| {
        formats
            .iter()
            .find(|f| f["manifest_url"].is_string())
            .and_then(|f| f["manifest_url"].as_str())
    }) {
        let client = Client::new();
        let response = client.get(manifest_url).send().await?;

        let content = response.text().await?;

        if content.contains("#EXTM3U") {
            // Save original manifest first
            let original_cache = ManifestCache::new(video_id, content.clone());
            original_cache.save_original(cache_dir)?;

            let manifest = filter_and_modify_manifest(content);
            let cache = ManifestCache::new(video_id, manifest);
            cache.save(cache_dir)?;
            Ok(())
        } else {
            Err("Invalid manifest format".into())
        }
    } else {
        Err("No manifest URL found".into())
    }
}
