use axum::{Router, extract::Path, response::Response, routing::get};
use std::env;
use std::process::Stdio;
use tokio::net::TcpListener;
use tokio::process::Command;
use tokio_util::io::ReaderStream;
use tracing::info;

const IS_DEV: bool = cfg!(debug_assertions);

#[tokio::main]
async fn main() {
    // Initialize logging
    tracing_subscriber::fmt::init();

    let app = Router::new().route("/stream/{id}", get(stream_youtube));

    info!("Starting server on 127.0.0.1:8080");
    let listener = TcpListener::bind("127.0.0.1:8080").await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

async fn stream_youtube(Path(video_id): Path<String>) -> Response {
    info!("Streaming video: {}", video_id);

    let url = format!("https://www.youtube.com/watch?v={}", video_id);

    // build args for yt-dlp
    // let mut args = vec![
    //     "-o",
    //     "-", // Output to stdout
    //     "-f",
    //     "bestvideo[ext=mp4][vcodec^=avc]+bestaudio[ext=m4a]/best[ext=mp4]",
    //     "--merge-output-format",
    //     "mp4",
    //     "--no-playlist",
    // ];
    let mut args = vec![
        "-o",
        "-", // Output to stdout
        "-f",
        "22/18", // Try format 22 (720p) or 18 (360p) - these are pre-merged formats
        "--no-playlist",
    ];
    if IS_DEV {
        args.push("-v");
    } else {
        args.push("--no-warnings");
    }
    args.push(&url);

    // Start yt-dlp process
    let process = match Command::new("yt-dlp")
        .args(&args)
        .stdout(Stdio::piped())
        .spawn()
    {
        Ok(process) => process,
        Err(e) => {
            tracing::error!("Failed to spawn yt-dlp: {}", e);
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
        // Add these headers to help with streaming
        .header("Accept-Ranges", "none") // Tell client seeking isn't supported
        .header("Cache-Control", "no-cache") // Prevent caching issues
        .header("X-Content-Type-Options", "nosniff") // Prevent content type sniffing
        .header("Transfer-Encoding", "chunked") // Indicate streaming content
        .body(axum::body::Body::from_stream(stream))
        .unwrap()
}
