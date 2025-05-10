use axum::{
    Router,
    extract::{Path, State},
    response::{Html, IntoResponse},
    routing::get,
};
use minijinja::context;

use crate::AppStateArc;

pub fn routes() -> Router<AppStateArc> {
    Router::new()
        .route("/channels/new", get(new_channel_page))
        .route("/channels/{id}", get(edit_channel_page))
}

pub async fn new_channel_page(State(state): State<AppStateArc>) -> impl IntoResponse {
    Html(
        state
            .templates
            .render(
                "channel.html",
                context! {
                    channel => None::<&str>,
                },
            )
            .unwrap(),
    )
}

pub async fn edit_channel_page(
    State(state): State<AppStateArc>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let config = state.config.read().await;
    let channel = config.channels.iter().find(|c| c.id == id);

    Html(
        state
            .templates
            .render(
                "channel.html",
                context! {
                    channel => channel,
                },
            )
            .unwrap(),
    )
}
