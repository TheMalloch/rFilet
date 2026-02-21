use axum::{
    extract::{Path, State, WebSocketUpgrade},
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Json},
};
use serde_json::json;

use crate::state::AppState;
use crate::ws;

pub async fn health() -> impl IntoResponse {
    (StatusCode::OK, Json(json!({ "ok": true, "service": "filet" })))
}

pub async fn download_blob(Path(id): Path<String>, State(state): State<AppState>) -> impl IntoResponse {
    let Some(entry) = state.blobs.get(&id) else {
        return StatusCode::NOT_FOUND.into_response();
    };

    let filename = entry.filename.clone();
    let size = entry.size;
    let bytes = entry.bytes.clone();
    drop(entry);

    let mut response = (StatusCode::OK, bytes).into_response();
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static("application/octet-stream"));
    if let Ok(value) = HeaderValue::from_str(&format!("attachment; filename=\"{filename}\"")) {
        response
            .headers_mut()
            .insert(header::CONTENT_DISPOSITION, value);
    }
    if let Ok(value) = HeaderValue::from_str(&size.to_string()) {
        response.headers_mut().insert(header::CONTENT_LENGTH, value);
    }
    response
}

pub async fn ws_send(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let ws = ws
        .max_message_size(1024 * 1024 * 1024)
        .max_frame_size(16 * 1024 * 1024);
    ws.on_upgrade(move |socket| ws::handle_sender(socket, state))
}
