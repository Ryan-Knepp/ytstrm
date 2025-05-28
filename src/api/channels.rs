use axum::{
    Form,
    extract::{Path, State},
    http::StatusCode,
    response::{Html, IntoResponse, Response},
};
use minijinja::context;
use serde::Deserialize;
use serde_with::{NoneAsEmptyString, serde_as};
use std::time::SystemTime;
use tracing::error;

use crate::AppStateArc;
use crate::config::{Channel, Source};

#[serde_as]
#[derive(Deserialize)]
pub struct ChannelForm {
    name: String,
    handle: String,
    #[serde_as(as = "NoneAsEmptyString")]
    max_videos: Option<usize>,
    #[serde_as(as = "NoneAsEmptyString")]
    max_age_days: Option<u32>,
}

pub async fn create_channel(
    State(state): State<AppStateArc>,
    Form(form): Form<ChannelForm>,
) -> Response {
    let mut config = state.config.write().await;

    // Check if channel already exists
    if config
        .channels
        .iter()
        .any(|c| matches!(&c.source, Source::Channel { handle, .. } if handle == &form.handle))
    {
        return (
            StatusCode::BAD_REQUEST,
            "Channel with this handle already exists",
        )
            .into_response();
    }

    // Set initial last_checked based on max_age_days
    let last_checked = match form.max_age_days {
        Some(days) => {
            let now = chrono::Utc::now();
            let past_date = now - chrono::Duration::days(days as i64);
            SystemTime::from(past_date)
        }
        None => {
            // Set to Unix epoch (1970-01-01) to get all available videos
            SystemTime::UNIX_EPOCH
        }
    };

    let new_channel = Channel {
        id: form.handle.clone(),
        source: Source::Channel {
            handle: form.handle.clone(),
            name: form.name,
            max_videos: form.max_videos,
            max_age_days: form.max_age_days,
        },
        last_checked,
        media_dir: config.jellyfin_media_path.join(&form.handle),
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

pub async fn update_channel(
    State(state): State<AppStateArc>,
    Path(id): Path<String>,
    Form(form): Form<ChannelForm>,
) -> Response {
    let mut config = state.config.write().await;

    if let Some(channel) = config.channels.iter_mut().find(|c| c.id == id) {
        if let Source::Channel {
            handle,
            name,
            max_videos,
            max_age_days,
            ..
        } = &mut channel.source
        {
            *handle = form.handle;
            *name = form.name;
            *max_videos = form.max_videos;
            *max_age_days = form.max_age_days;

            if let Err(e) = config.save() {
                error!("Failed to save config: {}", e);
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Failed to save configuration",
                )
                    .into_response();
            }
        } else {
            return (StatusCode::BAD_REQUEST, "Not a channel entry").into_response();
        }
    }

    (StatusCode::SEE_OTHER, [("HX-Redirect", "/")]).into_response()
}

pub async fn delete_channel(State(state): State<AppStateArc>, Path(id): Path<String>) -> Response {
    let mut config = state.config.write().await;

    // Only delete if it's a channel
    config
        .channels
        .retain(|c| !matches!(&c.source, Source::Channel { .. }) || c.id != id);

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

pub async fn reset_channel(
    State(state): State<AppStateArc>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let mut config = state.config.write().await;

    if let Some(channel) = config.channels.iter_mut().find(|c| c.id == id) {
        // Set last_checked based on channel configuration
        channel.last_checked = match &channel.source {
            Source::Channel { max_age_days, .. } => match max_age_days {
                Some(days) => {
                    let now = chrono::Utc::now();
                    let past_date = now - chrono::Duration::days(*days as i64);
                    SystemTime::from(past_date)
                }
                None => SystemTime::UNIX_EPOCH,
            },
            _ => return (StatusCode::BAD_REQUEST, "Not a channel entry").into_response(),
        };

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

        Html(format!(r#"<span>Reset Channel</span>"#)).into_response()
    } else {
        (StatusCode::NOT_FOUND, "Channel not found").into_response()
    }
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
