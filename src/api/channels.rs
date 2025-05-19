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

    let new_channel = Channel {
        id: form.handle.clone(),
        source: Source::Channel {
            handle: form.handle.clone(),
            name: form.name,
            max_videos: form.max_videos,
            max_age_days: form.max_age_days,
        },
        last_checked: SystemTime::now(),
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
    let config = state.config.read().await;

    if let Some(channel) = config.channels.iter().find(|c| c.id == id) {
        if !matches!(&channel.source, Source::Channel { .. }) {
            return (StatusCode::BAD_REQUEST, "Not a channel entry").into_response();
        }

        match channel.process_new_videos(&config).await {
            Ok(new_videos) => {
                Html(format!(r#"
                <div class="flex justify-between items-center border border-slate-200 rounded p-4 hover:bg-slate-50">
                    <div>
                        <h3 class="font-medium text-slate-800">{}</h3>
                        <p class="text-sm text-slate-500">{}</p>
                        <p class="text-sm text-green-600 mt-1">Found {} new videos</p>
                    </div>
                    <div class="flex items-center gap-4">
                        <button
                            class="text-purple-600 hover:text-purple-800"
                            hx-post="/api/channels/{}/load"
                            hx-swap="outerHTML"
                            hx-target="closest div"
                        >
                            <span>Load Videos</span>
                        </button>
                        <a 
                            href="/channels/{}"
                            class="text-purple-600 hover:text-purple-800"
                        >
                            Edit
                        </a>
                    </div>
                </div>
                "#, channel.get_name(), channel.get_handle_or_id(), new_videos, channel.id, channel.id)).into_response()
            }
            Err(e) => {
                error!("Failed to scan channel: {}", e);
                (StatusCode::INTERNAL_SERVER_ERROR, "Failed to scan channel").into_response()
            }
        }
    } else {
        (StatusCode::NOT_FOUND, "Channel not found").into_response()
    }
}
