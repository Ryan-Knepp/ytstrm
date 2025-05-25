use axum::{
    Form,
    extract::{Path, State},
    http::StatusCode,
    response::{Html, IntoResponse, Response},
};
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

pub async fn load_channel_videos(
    State(state): State<AppStateArc>,
    Path(id): Path<String>,
) -> Response {
    // Get channel info and config values needed for processing
    let (channel, media_path, server_addr) = {
        let mut config = state.config.write().await;

        if let Some(channel) = config.channels.iter().find(|c| c.id == id) {
            if !matches!(&channel.source, Source::Channel { .. }) {
                return (StatusCode::BAD_REQUEST, "Not a channel entry").into_response();
            }

            // Delete existing channel directory
            if let Err(e) = std::fs::remove_dir_all(&channel.media_dir) {
                error!("Failed to delete channel directory: {}", e);
            }

            let mut channel = channel.clone();

            // Reset last_checked on our local copy
            if let Source::Channel { max_age_days, .. } = &channel.source {
                channel.last_checked = match max_age_days {
                    Some(days) => {
                        let now = chrono::Utc::now();
                        let past_date = now - chrono::Duration::days(*days as i64);
                        SystemTime::from(past_date)
                    }
                    None => SystemTime::UNIX_EPOCH,
                };
            }

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
            return (StatusCode::NOT_FOUND, "Channel not found").into_response();
        }
    }; // Config lock is dropped here

    // Process videos without holding any locks
    match channel
        .process_new_videos(&media_path, &server_addr, &state.config)
        .await
    {
        Ok(new_videos) => Html(format!("{} videos", new_videos)).into_response(),
        Err(e) => {
            error!("Failed to scan channel: {}", e);
            Html("Failed to load videos").into_response()
        }
    }
}

// Executing yt-dlp with args: ["--compat-options", "no-youtube-channel-redirect", "--compat-options", "no-youtube-unavailable-videos", "--no-warnings", "--dump-json", "--ignore-errors", "--cookies", "cookies.txt", "--dateafter", "20240109", "--dateafter", "today-500days", "--playlist-start", "1", "--playlist-end", "5", "https://www.youtube.com/@dudeperfect/videos"]

// Executing yt-dlp with args: ["--compat-options", "no-youtube-channel-redirect", "--compat-options", "no-youtube-unavailable-videos", "--no-warnings", "--dump-json", "--ignore-errors", "--cookies", "cookies.txt", "--dateafter", "19691230", "https://www.youtube.com/playlist?list=PLCsuqbR8ZoiAkjk2dD10u-gigxGZw3am5"]
