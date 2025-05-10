mod api;
mod channel;
mod config;
mod manifest;
mod templates;

use axum::extract::State;
use axum::response::Html;
use axum::{Router, extract::Path, response::Response, routing::get};
use config::{Config, check_channels};
use std::process::Stdio;
use std::{path::PathBuf, sync::Arc};
use tokio::net::TcpListener;
use tokio::process::Command;
use tokio::sync::RwLock;
use tokio_util::io::ReaderStream;
use tracing::info;

use manifest::{ManifestCache, fetch_and_filter_manifest, maintain_manifest_cache};
use templates::{TemplateState, Templates};

const IS_DEV: bool = cfg!(debug_assertions);

pub type ConfigState = Arc<RwLock<Config>>;

pub struct AppState {
    config: ConfigState,
    templates: TemplateState,
}
pub type AppStateArc = Arc<AppState>;

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

    let config = Arc::new(RwLock::new(Config::load().unwrap()));

    // Spawn background maintenance task
    let config_clone = config.clone();
    tokio::spawn(maintain_manifest_cache(config_clone));

    let config_clone = config.clone();
    tokio::spawn(async move {
        let _ = check_channels(config_clone).await;
    });

    let templates = Arc::new(Templates::new().unwrap());

    let app_state = Arc::new(AppState {
        config: config.clone(),
        templates: templates.clone(),
    });

    let app = Router::new()
        .route("/", get(index_handler))
        .merge(channel::routes())
        .route("/stream/{id}", get(stream_youtube))
        .route("/youtube/direct/{id}", get(stream_youtube))
        .nest("/api", api::routes())
        .with_state(app_state);

    info!("Starting server on 127.0.0.1:8080");
    let listener = TcpListener::bind("0.0.0.0:8080").await.unwrap();
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

    match fetch_and_filter_manifest(&video_id, &cache_dir, true).await {
        Ok(manifest) => {
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
        }
        Err(e) => {
            info!(
                "Failed to fetch/filter manifest: {}, falling back to MP4",
                e
            );
            direct_mp4_streaming(
                &format!("https://www.youtube.com/watch?v={}", video_id),
                &video_id,
            )
            .await
        }
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

async fn index_handler(State(state): State<AppStateArc>) -> Result<Html<String>, ()> {
    let config_guard = state.config.read().await;
    let html = state
        .templates
        .render(
            "config.html",
            minijinja::context! {
                config => &*config_guard,
            },
        )
        .map_err(|_| ())?;
    Ok(Html(html))
}
