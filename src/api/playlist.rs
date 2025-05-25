use axum::{
    Form,
    extract::{Path, State},
    http::StatusCode,
    response::{Html, IntoResponse, Response},
};
use serde::Deserialize;
use std::time::SystemTime;
use tracing::error;

use crate::AppStateArc;
use crate::config::{Channel, Source};

#[derive(Deserialize)]
pub struct PlaylistForm {
    name: String,
    playlist_id: String,
}

pub async fn create_playlist(
    State(state): State<AppStateArc>,
    Form(form): Form<PlaylistForm>,
) -> Response {
    let mut config = state.config.write().await;

    // Check if playlist already exists
    if config.channels.iter().any(|c| match &c.source {
        Source::Playlist { id, .. } => id == &form.playlist_id,
        _ => false,
    }) {
        return (StatusCode::BAD_REQUEST, "Playlist already exists").into_response();
    }

    let new_channel = Channel {
        id: form.playlist_id.clone(),
        source: Source::Playlist {
            id: form.playlist_id.clone(),
            name: form.name,
        },
        last_checked: SystemTime::UNIX_EPOCH,
        media_dir: config.jellyfin_media_path.join(&form.playlist_id),
    };

    config.channels.push(new_channel);

    if let Err(e) = config.save() {
        error!("Failed to save config: {}", e);
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Failed to save configuration",
        )
            .into_response();
    }

    (StatusCode::SEE_OTHER, [("HX-Redirect", "/")]).into_response()
}

// ...existing code...

pub async fn update_playlist(
    State(state): State<AppStateArc>,
    Path(id): Path<String>,
    Form(form): Form<PlaylistForm>,
) -> Response {
    let mut config = state.config.write().await;

    if let Some(channel) = config.channels.iter_mut().find(|c| c.id == id) {
        if let Source::Playlist { id, name } = &mut channel.source {
            *id = form.playlist_id;
            *name = form.name;

            if let Err(e) = config.save() {
                error!("Failed to save config: {}", e);
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Failed to save configuration",
                )
                    .into_response();
            }
        } else {
            return (StatusCode::BAD_REQUEST, "Not a playlist entry").into_response();
        }
    } else {
        return (StatusCode::NOT_FOUND, "Playlist not found").into_response();
    }

    (StatusCode::SEE_OTHER, [("HX-Redirect", "/")]).into_response()
}

pub async fn delete_playlist(State(state): State<AppStateArc>, Path(id): Path<String>) -> Response {
    let mut config = state.config.write().await;

    // Only delete if it's a playlist
    config
        .channels
        .retain(|c| !matches!(&c.source, Source::Playlist { .. }) || c.id != id);

    if let Err(e) = config.save() {
        error!("Failed to save config: {}", e);
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Failed to save configuration",
        )
            .into_response();
    }

    (StatusCode::SEE_OTHER, [("HX-Redirect", "/")]).into_response()
}

pub async fn load_playlist_videos(
    State(state): State<AppStateArc>,
    Path(id): Path<String>,
) -> Response {
    // Get playlist info and config values needed for processing
    let (playlist, media_path, server_addr) = {
        let mut config = state.config.write().await;

        if let Some(channel) = config.channels.iter().find(|c| c.id == id) {
            if !matches!(&channel.source, Source::Playlist { .. }) {
                return (StatusCode::BAD_REQUEST, "Not a playlist entry").into_response();
            }

            // Delete existing playlist directory
            if let Err(e) = std::fs::remove_dir_all(&channel.media_dir) {
                error!("Failed to delete playlist directory: {}", e);
            }

            let mut channel = channel.clone();

            // Reset last_checked
            channel.last_checked = SystemTime::UNIX_EPOCH;

            // Save the updated channel
            if let Some(existing_channel) = config.channels.iter_mut().find(|c| c.id == id) {
                *existing_channel = channel.clone();
            }
            if let Err(e) = config.save() {
                error!("Failed to save config: {}", e);
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Failed to save configuration",
                )
                    .into_response();
            }

            (
                channel,
                config.jellyfin_media_path.clone(),
                config.server_address.clone(),
            )
        } else {
            return (StatusCode::NOT_FOUND, "Playlist not found").into_response();
        }
    }; // Config lock is dropped here

    // Process videos without holding any locks
    match playlist
        .process_new_videos(&media_path, &server_addr, &state.config)
        .await
    {
        Ok(new_videos) => Html(format!("{} videos", new_videos)).into_response(),
        Err(e) => {
            error!("Failed to scan playlist: {}", e);
            Html("Failed to load videos").into_response()
        }
    }
}
