pub mod channels;
pub mod playlist;
pub mod settings;

use crate::AppStateArc;
use axum::{
    Router,
    routing::{delete, post, put},
};

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
        // Channel routes
        .route("/channels/new", post(channels::create_channel))
        .route("/channels/{id}", put(channels::update_channel))
        .route("/channels/{id}", delete(channels::delete_channel))
        .route("/channels/{id}/load", post(channels::load_channel_videos))
        .route("/playlists/new", post(playlist::create_playlist))
        .route("/playlists/{id}", put(playlist::update_playlist))
        .route("/playlists/{id}", delete(playlist::delete_playlist))
        .route("/playlists/{id}/load", post(playlist::load_playlist_videos))
}
