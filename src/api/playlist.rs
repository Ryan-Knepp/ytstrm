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
        last_checked: SystemTime::now(),
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
    let config = state.config.read().await;

    if let Some(channel) = config.channels.iter().find(|c| c.id == id) {
        match channel.process_new_videos(&config).await {
            Ok(new_videos) => {
                Html(format!(r#"
                <div class="flex justify-between items-center border border-slate-200 rounded p-4 hover:bg-slate-50">
                    <div>
                        <h3 class="font-medium text-slate-800">{}</h3>
                        <p class="text-sm text-slate-500">Playlist: {}</p>
                        <p class="text-sm text-green-600 mt-1">Found {} new videos</p>
                    </div>
                    <div class="flex items-center gap-4">
                        <button
                            class="text-purple-600 hover:text-purple-800"
                            hx-post="/api/playlists/{}/load"
                            hx-swap="outerHTML"
                            hx-target="closest div"
                        >
                            <span>Load Videos</span>
                        </button>
                        <a 
                            href="/playlists/{}"
                            class="text-purple-600 hover:text-purple-800"
                        >
                            Edit
                        </a>
                    </div>
                </div>
                "#, channel.get_name(), channel.get_handle_or_id(), new_videos, channel.id, channel.id)).into_response()
            }
            Err(e) => {
                error!("Failed to scan playlist: {}", e);
                (StatusCode::INTERNAL_SERVER_ERROR, "Failed to scan playlist").into_response()
            }
        }
    } else {
        (StatusCode::NOT_FOUND, "Playlist not found").into_response()
    }
}
