use axum::{
    body::Body,
    extract::{Path, State, WebSocketUpgrade},
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Json},
};
use bytes::Bytes;
use futures_util::stream::Stream;
use serde_json::json;
use tokio::io::AsyncReadExt;

use crate::state::{AppState, unix_now};
use crate::ws;

pub async fn health() -> impl IntoResponse {
    (StatusCode::OK, Json(json!({ "ok": true, "service": "filet" })))
}

pub async fn download_blob(Path(id): Path<String>, State(state): State<AppState>) -> impl IntoResponse {
    let Some(manifest) = state.load_manifest(&id) else {
        return StatusCode::NOT_FOUND.into_response();
    };

    if !manifest.complete || manifest.received_size != manifest.size {
        return StatusCode::NOT_FOUND.into_response();
    }

    if manifest.expires_at_unix <= unix_now() {
        state.delete_transfer_files(&id);
        return StatusCode::NOT_FOUND.into_response();
    }

    let chunks_dir = state.chunk_dir(&id);
    let chunk_count = manifest.chunk_count;

    let stream = chunk_stream(chunks_dir, chunk_count);

    let mut response = (StatusCode::OK, Body::from_stream(stream)).into_response();
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static("application/octet-stream"));
    if let Ok(value) = HeaderValue::from_str(&format!("attachment; filename=\"{}\"", manifest.filename)) {
        response
            .headers_mut()
            .insert(header::CONTENT_DISPOSITION, value);
    }
    if let Ok(value) = HeaderValue::from_str(&manifest.size.to_string()) {
        response.headers_mut().insert(header::CONTENT_LENGTH, value);
    }
    response
}

fn chunk_stream(
    chunks_dir: std::path::PathBuf,
    chunk_count: u64,
) -> impl Stream<Item = Result<Bytes, std::io::Error>> {
    async_stream::try_stream! {
        for index in 1..=chunk_count {
            let chunk_path = chunks_dir.join(format!("{index:08}.part"));
            let mut file = tokio::fs::File::open(chunk_path).await?;
            let mut buffer = vec![0u8; 64 * 1024];

            loop {
                let bytes_read = file.read(&mut buffer).await?;
                if bytes_read == 0 {
                    break;
                }
                yield Bytes::copy_from_slice(&buffer[..bytes_read]);
            }
        }
    }
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
