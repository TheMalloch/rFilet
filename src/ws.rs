use axum::extract::ws::{Message, WebSocket};
use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use tracing::warn;

use crate::state::{AppState, StoredBlob};

#[derive(serde::Deserialize)]
struct SenderInit {
    r#type: String,
    filename: String,
    size: u64,
}

#[derive(serde::Serialize)]
struct WsOk {
    r#type: &'static str,
    id: String,
}

#[derive(serde::Serialize)]
struct WsError {
    r#type: &'static str,
    message: String,
}

pub async fn handle_sender(socket: WebSocket, state: AppState) {
    let (mut ws_tx, mut ws_rx) = socket.split();

    let init = match ws_rx.next().await {
        Some(Ok(Message::Text(text))) => match serde_json::from_str::<SenderInit>(&text) {
            Ok(parsed) if parsed.r#type == "send" => parsed,
            Ok(_) => {
                let _ = send_error(&mut ws_tx, "first message must be type=send").await;
                return;
            }
            Err(_) => {
                let _ = send_error(&mut ws_tx, "invalid send metadata").await;
                return;
            }
        },
        _ => {
            let _ = send_error(&mut ws_tx, "missing send metadata").await;
            return;
        }
    };

    let id = nanoid::nanoid!(16);

    let ready = serde_json::to_string(&WsOk {
        r#type: "ready",
        id: id.clone(),
    })
    .unwrap_or_else(|_| "{\"type\":\"error\",\"message\":\"internal error\"}".to_string());

    if ws_tx.send(Message::Text(ready.into())).await.is_err() {
        return;
    }

    let expected_size = init.size as usize;
    let mut payload = Vec::with_capacity(expected_size.min(8 * 1024 * 1024));

    while payload.len() < expected_size {
        match ws_rx.next().await {
            Some(Ok(Message::Binary(chunk))) => {
                if payload.len().saturating_add(chunk.len()) > expected_size {
                    let _ = send_error(&mut ws_tx, "payload exceeds declared size").await;
                    return;
                }
                payload.extend_from_slice(&chunk);
            }
            Some(Ok(Message::Close(_))) | None => break,
            Some(Ok(_)) => continue,
            Some(Err(err)) => {
                warn!(error = %err, "websocket receive error");
                let _ = send_error(&mut ws_tx, "websocket receive error").await;
                return;
            }
        }
    }

    if payload.len() != expected_size {
        let _ = send_error(&mut ws_tx, "payload size mismatch").await;
        return;
    }

    state.blobs.insert(
        id,
        StoredBlob {
            filename: init.filename,
            size: init.size,
            bytes: Bytes::from(payload),
        },
    );
}

async fn send_error(
    ws_tx: &mut futures_util::stream::SplitSink<WebSocket, Message>,
    message: &str,
) -> Result<(), axum::Error> {
    let text = serde_json::to_string(&WsError {
        r#type: "error",
        message: message.to_string(),
    })
    .unwrap_or_else(|_| "{\"type\":\"error\",\"message\":\"internal error\"}".to_string());
    ws_tx.send(Message::Text(text.into())).await
}
