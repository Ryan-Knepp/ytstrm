pub mod channels;
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
        // Channel routes
        .route("/channels/new", post(channels::create_channel))
        .route("/channels/{id}", put(channels::update_channel))
        .route("/channels/{id}", delete(channels::delete_channel))
        .route("/channels/{id}/load", post(channels::load_channel_videos))
}
