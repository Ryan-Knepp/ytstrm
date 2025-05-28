use axum::{
    Form,
    extract::{Path, State},
    http::StatusCode,
    response::{Html, IntoResponse, Response},
};
use minijinja::context;
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

pub async fn reset_playlist(
    State(state): State<AppStateArc>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let mut config = state.config.write().await;

    if let Some(channel) = config.channels.iter_mut().find(|c| c.id == id) {
        // Reset last_checked time
        channel.last_checked = SystemTime::UNIX_EPOCH;

        // Delete media directory if it exists
        if let Err(e) = tokio::fs::remove_dir_all(&channel.media_dir).await {
            error!("Failed to delete directory: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "error occurred").into_response();
        }

        // Save config
        if let Err(e) = config.save() {
            error!("Failed to save config: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "error occurred").into_response();
        }

        return Html(format!(r#"<span>Reset Playlist</span>"#)).into_response();
    }

    (StatusCode::NOT_FOUND, "Playlist not found").into_response()
}

pub async fn progress_view(
    State(state): State<AppStateArc>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    Html(
        state
            .templates
            .render(
                "partials/load_video_sse.html",
                context! {
                    channel_id => id,
                },
            )
            .unwrap(),
    )
}
