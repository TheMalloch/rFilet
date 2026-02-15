use axum::{
    extract::{Path, State, WebSocketUpgrade},
    http::{header, StatusCode},
    response::{Html, IntoResponse, Json},
};

use crate::state::{AppState, TransferState};
use crate::static_assets::{RECEIVER_HTML, SENDER_HTML};
use crate::ws;

pub async fn sender_page() -> Html<&'static str> {
    Html(SENDER_HTML)
}

pub async fn receiver_page(Path(id): Path<String>, State(state): State<AppState>) -> impl IntoResponse {
    // Check transfer exists
    if !state.transfers.contains_key(&id) {
        return (
            StatusCode::NOT_FOUND,
            [(header::CONTENT_TYPE, "text/html")],
            "<!DOCTYPE html><html><body style='background:#0a0a0a;color:#e0e0e0;font-family:monospace;display:flex;align-items:center;justify-content:center;height:100vh'><p>transfer not found or expired</p></body></html>".to_string(),
        );
    }

    let html = RECEIVER_HTML.replace("{{TRANSFER_ID}}", &id);
    (StatusCode::OK, [(header::CONTENT_TYPE, "text/html")], html)
}

pub async fn transfer_info(
    Path(id): Path<String>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    match state.transfers.get(&id) {
        Some(entry) => match entry.value() {
            TransferState::WaitingForRecipient { metadata, .. } => {
                (StatusCode::OK, Json(serde_json::json!({
                    "filename": metadata.filename,
                    "size": metadata.size,
                    "mime_type": metadata.mime_type,
                }))).into_response()
            }
            _ => StatusCode::GONE.into_response(),
        },
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

pub async fn ws_send(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| ws::handle_sender(socket, state))
}

pub async fn ws_recv(
    ws: WebSocketUpgrade,
    Path(id): Path<String>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| ws::handle_receiver(socket, id, state))
}
