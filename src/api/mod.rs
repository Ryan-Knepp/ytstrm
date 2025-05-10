pub mod settings;

use crate::AppStateArc;
use axum::{Router, routing::put};

pub fn routes() -> Router<AppStateArc> {
    Router::new()
        .route(
            "/config/server-address",
            put(settings::update_server_address),
        )
        .route(
            "/config/check-interval",
            put(settings::update_check_interval),
        )
        .route("/config/media-path", put(settings::update_media_path))
}
