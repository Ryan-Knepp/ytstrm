mod manifest;

use axum::{Router, extract::Path, response::Response, routing::get};
use reqwest::Client;
use serde_json::Value;
use std::path::PathBuf;
use std::process::Stdio;
use tokio::net::TcpListener;
use tokio::process::Command;
use tokio_util::io::ReaderStream;
use tracing::info;

use manifest::{ManifestCache, filter_and_modify_manifest, maintain_manifest_cache};

const IS_DEV: bool = cfg!(debug_assertions);

#[tokio::main]
async fn main() {
    // Initialize logging
    if IS_DEV {
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .with_target(true)
            .with_thread_ids(true)
            .with_file(true)
            .with_line_number(true)
            .init();
    } else {
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::INFO)
            .init();
    }

    // Spawn background maintenance task
    tokio::spawn(maintain_manifest_cache());

    let app = Router::new().route("/stream/{id}", get(stream_youtube));

    info!("Starting server on 127.0.0.1:8000");
    let listener = TcpListener::bind("127.0.0.1:8000").await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

async fn stream_youtube(Path(video_id): Path<String>) -> Response {
    info!("Streaming video: {}", video_id);

    let cache_dir = dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("ytstrm/manifests");
    info!("Cache directory: {:?}", cache_dir);

    // Try to load from cache first
    if let Ok(cache) = ManifestCache::load(&video_id, &cache_dir) {
        if cache.is_valid() {
            info!("Serving cached manifest for {}", video_id);
            return Response::builder()
                .status(200)
                .header("Content-Type", "application/vnd.apple.mpegurl")
                .header("Access-Control-Allow-Origin", "*")
                .header("Content-Length", cache.content.len().to_string())
                .header(
                    "Content-Disposition",
                    "attachment; filename=\"playlist.m3u8\"",
                )
                .header("Cache-Control", "no-cache")
                .header("Pragma", "no-cache")
                .header("Expires", "0")
                .body(axum::body::Body::from(cache.content))
                .unwrap();
        }
    }

    let url = format!("https://www.youtube.com/watch?v={}", video_id);

    // Get video metadata as JSON
    let output = match Command::new("yt-dlp")
        .args([
            "-j",
            "--no-playlist",
            "--cookies",
            "cookies.txt", // Add cookies file
            &url,
        ])
        .output()
        .await
    {
        Ok(output) => {
            info!("Successfully fetched metadata");
            output
        }
        Err(e) => {
            info!("Failed to execute yt-dlp: {}", e);
            return Response::builder()
                .status(500)
                .body(axum::body::Body::empty())
                .unwrap();
        }
    };

    let metadata: Value = match serde_json::from_slice(&output.stdout) {
        Ok(metadata) => {
            info!("Successfully parsed metadata JSON");
            metadata
        }
        Err(e) => {
            info!("Failed to parse metadata JSON: {}", e);
            return Response::builder()
                .status(500)
                .body(axum::body::Body::empty())
                .unwrap();
        }
    };

    // Get first manifest URL
    if let Some(manifest_url) = metadata["formats"].as_array().and_then(|formats| {
        formats
            .iter()
            .find(|f| f["manifest_url"].is_string())
            .and_then(|f| f["manifest_url"].as_str())
    }) {
        info!("Found HLS manifest URL: {}", manifest_url);
        let client = Client::new();
        let manifest = match async {
            let response = client
                .get(manifest_url)
                .header(
                    "User-Agent",
                    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)",
                )
                .send()
                .await?;
            response.text().await
        }
        .await
        {
            Ok(content) => {
                if !content.contains("#EXTM3U") {
                    info!("Invalid manifest format, falling back to MP4");
                    return direct_mp4_streaming(&url, &video_id).await;
                }
                // Filter and modify the manifest
                filter_and_modify_manifest(content)
            }
            Err(e) => {
                info!("Failed to fetch manifest: {}", e);
                return Response::builder()
                    .status(500)
                    .body(axum::body::Body::empty())
                    .unwrap();
            }
        };

        // Ensure manifest ends with a newline
        let manifest = if !manifest.ends_with('\n') {
            format!("{}\n", manifest)
        } else {
            manifest
        };

        // After filtering manifest, cache it
        let cache = ManifestCache::new(&video_id, manifest.clone());
        if let Err(e) = cache.save(&cache_dir) {
            info!("Failed to cache manifest: {}", e);
        }

        info!("Sending manifest response with length: {}", manifest.len());
        Response::builder()
            .status(200)
            .header("Content-Type", "application/vnd.apple.mpegurl")
            .header("Access-Control-Allow-Origin", "*")
            .header("Content-Length", manifest.len().to_string())
            .header(
                "Content-Disposition",
                "attachment; filename=\"playlist.m3u8\"",
            )
            .header(
                "Cache-Control",
                "no-cache, no-store, must-revalidate, must-validate",
            )
            .header("Pragma", "no-cache")
            .header("Expires", "0")
            .body(axum::body::Body::from(manifest))
            .unwrap()
    } else {
        // Fallback to direct MP4 streaming
        direct_mp4_streaming(&url, &video_id).await
    }
}

async fn direct_mp4_streaming(url: &str, video_id: &str) -> Response {
    info!("Attempting direct MP4 streaming");
    let process = match Command::new("yt-dlp")
        .args([
            "-o",
            "-",
            "-f",
            "22/18/best[ext=mp4]",
            "--no-playlist",
            "--cookies",
            "cookies.txt",
        ])
        .arg(if IS_DEV { "-v" } else { "--no-warnings" })
        .arg(url)
        .stdout(Stdio::piped())
        .spawn()
    {
        Ok(process) => process,
        Err(e) => {
            info!("Failed to spawn yt-dlp: {}", e);
            return Response::builder()
                .status(500)
                .body(axum::body::Body::empty())
                .unwrap();
        }
    };

    let stdout = process.stdout.unwrap();
    let stream = ReaderStream::new(stdout);

    Response::builder()
        .header("Content-Type", "video/mp4")
        .header(
            "Content-Disposition",
            format!("inline; filename=\"{}.mp4\"", video_id),
        )
        .header("Accept-Ranges", "none")
        .header("Cache-Control", "no-cache")
        .body(axum::body::Body::from_stream(stream))
        .unwrap()
}
