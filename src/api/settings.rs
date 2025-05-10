use axum::response::Html;
use axum::{Form, extract::State, response::IntoResponse};
use minijinja::context;
use serde::Deserialize;
use std::path::PathBuf;
use tracing::error;
use url::Url;

use crate::AppStateArc;

#[derive(Deserialize)]
pub struct ServerAddress {
    server_address: String,
}

#[derive(Deserialize)]
pub struct CheckInterval {
    check_interval: u64,
}

#[derive(Deserialize)]
pub struct MediaPath {
    jellyfin_media_path: String,
}

pub async fn update_server_address(
    State(state): State<AppStateArc>,
    Form(form): Form<ServerAddress>,
) -> impl IntoResponse {
    let url_str = if !form.server_address.starts_with("http") {
        format!("http://{}", form.server_address)
    } else {
        form.server_address.clone()
    };

    if let Err(_) = Url::parse(&url_str) {
        return Html(
            state
                .templates
                .render(
                    "partials/settings/server_address_input.html",
                    context! {
                        value => form.server_address,
                        error => "Invalid server address format",
                    },
                )
                .unwrap(),
        )
        .into_response();
    }

    let mut config_guard = state.config.write().await;
    config_guard.server_address = url_str.clone();
    if let Err(e) = config_guard.save() {
        error!("Failed to save config: {}", e);
        return Html(
            state
                .templates
                .render(
                    "partials/settings/server_address_input.html",
                    context! {
                        value => url_str,
                        error => "Failed to save configuration",
                    },
                )
                .unwrap(),
        )
        .into_response();
    }

    Html(
        state
            .templates
            .render(
                "partials/settings/server_address_input.html",
                context! {
                    value => url_str,
                    error => None::<String>,
                },
            )
            .unwrap(),
    )
    .into_response()
}

pub async fn update_check_interval(
    State(state): State<AppStateArc>,
    Form(form): Form<CheckInterval>,
) -> impl IntoResponse {
    let mut config_guard = state.config.write().await;
    config_guard.check_interval = form.check_interval;
    if let Err(e) = config_guard.save() {
        error!("Failed to save config: {}", e);
        return Html(
            state
                .templates
                .render(
                    "partials/settings/check_interval_input.html",
                    context! {
                        value => form.check_interval,
                        error => "Failed to save configuration",
                    },
                )
                .unwrap(),
        )
        .into_response();
    }

    Html(
        state
            .templates
            .render(
                "partials/settings/check_interval_input.html",
                context! {
                    value => form.check_interval,
                    error => None::<String>,
                },
            )
            .unwrap(),
    )
    .into_response()
}

pub async fn update_media_path(
    State(state): State<AppStateArc>,
    Form(form): Form<MediaPath>,
) -> impl IntoResponse {
    let path = PathBuf::from(form.jellyfin_media_path.clone());

    if !path.exists() {
        return Html(
            state
                .templates
                .render(
                    "partials/settings/media_path_input.html",
                    context! {
                        value => form.jellyfin_media_path,
                        error => "Directory does not exist",
                    },
                )
                .unwrap(),
        )
        .into_response();
    }

    let mut config_guard = state.config.write().await;
    config_guard.jellyfin_media_path = path.clone();
    if let Err(e) = config_guard.save() {
        error!("Failed to save config: {}", e);
        return Html(
            state
                .templates
                .render(
                    "partials/settings/media_path_input.html",
                    context! {
                        value => path.display().to_string(),
                        error => "Failed to save configuration",
                    },
                )
                .unwrap(),
        )
        .into_response();
    }

    Html(
        state
            .templates
            .render(
                "partials/settings/media_path_input.html",
                context! {
                    value => path.display().to_string(),
                    error => None::<String>,
                },
            )
            .unwrap(),
    )
    .into_response()
}
