pub mod channels;
pub mod playlist;
pub mod settings;

use crate::AppStateArc;

use axum::{
    Router,
    extract::{Path, State},
    response::{Sse, sse::Event},
    routing::{delete, get, post, put},
};
use futures::{Stream, StreamExt, future, stream};
use percent_encoding::percent_decode_str;
use std::{borrow::Cow, convert::Infallible};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tracing::{error, info};

pub fn routes() -> Router<AppStateArc> {
    Router::new()
        // Settings routes
        .route(
            "/config/server-address",
            put(settings::update_server_address),
        )
        .route(
            "/config/check-interval",
            put(settings::update_check_interval),
        )
        .route("/config/media-path", put(settings::update_media_path))
        .route(
            "/config/toggle-background-tasks",
            post(settings::toggle_background_tasks),
        )
        .route(
            "/config/toggle-manifest-maintenance",
            post(settings::toggle_manifest_maintenance),
        )
        // Channel routes
        .route("/channels/new", post(channels::create_channel))
        .route("/channels/{id}", put(channels::update_channel))
        .route("/channels/{id}", delete(channels::delete_channel))
        .route("/channels/{id}/reset", post(channels::reset_channel))
        .route("/channels/{id}/progress-view", get(channels::progress_view))
        .route("/playlists/new", post(playlist::create_playlist))
        .route("/playlists/{id}", put(playlist::update_playlist))
        .route("/playlists/{id}", delete(playlist::delete_playlist))
        .route("/playlists/{id}/reset", post(playlist::reset_playlist))
        .route(
            "/playlists/{id}/progress-view",
            get(playlist::progress_view),
        )
        .route("/progress/{id}", get(progress_sse_handler))
}

async fn progress_sse_handler(
    State(state): State<AppStateArc>,
    Path(id): Path<String>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let decoded_id = percent_decode_str(&id)
        .decode_utf8()
        .unwrap_or(Cow::Borrowed(&id))
        .into_owned();
    info!("Creating progress SSE handler for channel {}", decoded_id);
    let (tx, rx) = mpsc::channel(100);
    info!("Created channel with capacity 100");

    let stream = ReceiverStream::new(rx)
        .map(|msg| {
            info!("Received message in stream: {}", msg);
            // Send all regular messages as "message" events instead of "progress"
            Ok(Event::default().data(msg))
        })
        .chain(stream::once(async {
            info!("Sending completion message");
            Ok(Event::default().event("complete").data("done"))
        }))
        .take_while(|msg| future::ready(msg.is_ok()));

    // Get required config values
    let config = state.config.read().await;
    let media_path = config.jellyfin_media_path.clone();
    let server_addr = config.server_address.clone();
    let channel = config
        .channels
        .iter()
        .find(|c| c.id == decoded_id)
        .cloned()
        .expect("Channel should exist at this point");
    drop(config);

    info!("Starting video processing task");
    // Spawn video loading task
    let state_clone = state.clone();
    tokio::spawn(async move {
        info!("Processing videos for channel {}", channel.get_name());
        if let Err(e) = channel
            .process_new_videos(&media_path, &server_addr, &state_clone.config, Some(tx))
            .await
        {
            error!("Error processing videos: {}", e);
        }
        info!("Finished processing videos");
    });

    info!("Returning SSE stream");
    Sse::new(stream)
}
