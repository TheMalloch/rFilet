use axum::{
    extract::{Path, State, WebSocketUpgrade},
    http::{header, StatusCode},
    response::{IntoResponse, Json},
};

use crate::cli_state::CliState;
use crate::cli_ws;
use crate::static_assets::CLI_RECEIVER_HTML;

pub async fn download_page(
    Path(token): Path<String>,
    State(state): State<CliState>,
) -> impl IntoResponse {
    if !state.files.contains_key(&token) {
        return (
            StatusCode::NOT_FOUND,
            [(header::CONTENT_TYPE, "text/html")],
            "<!DOCTYPE html><html><body style='background:#0a0a0a;color:#e0e0e0;font-family:monospace;display:flex;align-items:center;justify-content:center;height:100vh'><p>file not found</p></body></html>".to_string(),
        );
    }

    let html = CLI_RECEIVER_HTML.replace("{{TOKEN}}", &token);
    (StatusCode::OK, [(header::CONTENT_TYPE, "text/html")], html)
}

pub async fn file_metadata(
    Path(token): Path<String>,
    State(state): State<CliState>,
) -> impl IntoResponse {
    match state.files.get(&token) {
        Some(entry) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "filename": entry.filename,
                "size": entry.size,
                "mime_type": entry.mime_type,
            })),
        )
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

pub async fn direct_download(
    Path(token): Path<String>,
    State(state): State<CliState>,
) -> impl IntoResponse {
    let entry = match state.files.get(&token) {
        Some(e) => e,
        None => return StatusCode::NOT_FOUND.into_response(),
    };

    let filename = entry.filename.clone();
    let mime_type = entry.mime_type.clone();
    let path = entry.path.clone();
    let size = entry.size;
    drop(entry);

    let file = match tokio::fs::File::open(&path).await {
        Ok(f) => f,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };

    let stream = tokio_util::io::ReaderStream::new(file);
    let body = axum::body::Body::from_stream(stream);

    let disposition = format!(
        "attachment; filename=\"{}\"",
        filename.replace('"', "\\\"")
    );

    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, mime_type),
            (header::CONTENT_DISPOSITION, disposition),
            (header::CONTENT_LENGTH, size.to_string()),
        ],
        body,
    )
        .into_response()
}

pub async fn ws_download(
    ws: WebSocketUpgrade,
    Path(token): Path<String>,
    State(state): State<CliState>,
) -> impl IntoResponse {
    if !state.files.contains_key(&token) {
        return StatusCode::NOT_FOUND.into_response();
    }
    ws.on_upgrade(move |socket| cli_ws::handle_ws_download(socket, token, state))
        .into_response()
}
