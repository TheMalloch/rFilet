use axum::extract::ws::{Message, WebSocket};
use futures_util::{SinkExt, StreamExt};
use std::path::Path;
use tokio::fs;
use tracing::warn;

use crate::state::{AppState, FileManifest, unix_now};

#[derive(serde::Deserialize)]
struct SenderInit {
    r#type: String,
    filename: String,
    size: u64,
    #[serde(default)]
    expires_in_seconds: Option<u64>,
    #[serde(default)]
    expires_at_unix: Option<u64>,
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

    let now = unix_now();
    let expires_at_unix = match compute_expiration(&init, now) {
        Ok(value) => value,
        Err(message) => {
            let _ = send_error(&mut ws_tx, message).await;
            return;
        }
    };

    let id = nanoid::nanoid!(16);
    let chunk_tmp_dir = state.chunk_tmp_dir(&id);
    let manifest_tmp_path = state.manifest_tmp_path(&id);

    if fs::create_dir_all(&chunk_tmp_dir).await.is_err() {
        let _ = send_error(&mut ws_tx, "failed to initialize chunk storage").await;
        return;
    }

    let mut manifest = FileManifest {
        id: id.clone(),
        filename: init.filename,
        size: init.size,
        created_at_unix: now,
        expires_at_unix,
        chunk_size: 0,
        chunk_count: 0,
        received_size: 0,
        complete: false,
    };

    if write_manifest_file(&manifest_tmp_path, &manifest).await.is_err() {
        state.delete_transfer_files(&id);
        let _ = send_error(&mut ws_tx, "failed to write manifest").await;
        return;
    }

    let ready = serde_json::to_string(&WsOk {
        r#type: "ready",
        id: id.clone(),
    })
    .unwrap_or_else(|_| "{\"type\":\"error\",\"message\":\"internal error\"}".to_string());

    if ws_tx.send(Message::Text(ready.into())).await.is_err() {
        state.delete_transfer_files(&id);
        return;
    }

    let mut part_index: u64 = 0;

    while manifest.received_size < manifest.size {
        match ws_rx.next().await {
            Some(Ok(Message::Binary(chunk))) => {
                if chunk.is_empty() {
                    continue;
                }

                let chunk_len = chunk.len() as u64;
                if manifest.received_size.saturating_add(chunk_len) > manifest.size {
                    state.delete_transfer_files(&id);
                    let _ = send_error(&mut ws_tx, "payload exceeds declared size").await;
                    return;
                }

                part_index += 1;
                let part_path = chunk_tmp_dir.join(format!("{part_index:08}.part"));
                if fs::write(&part_path, &chunk).await.is_err() {
                    state.delete_transfer_files(&id);
                    let _ = send_error(&mut ws_tx, "failed to store chunk").await;
                    return;
                }

                manifest.received_size += chunk_len;
                manifest.chunk_count = part_index;
                if manifest.chunk_size == 0 {
                    manifest.chunk_size = chunk_len;
                }

                if write_manifest_file(&manifest_tmp_path, &manifest).await.is_err() {
                    state.delete_transfer_files(&id);
                    let _ = send_error(&mut ws_tx, "failed to update manifest").await;
                    return;
                }
            }
            Some(Ok(Message::Close(_))) | None => {
                state.delete_transfer_files(&id);
                return;
            }
            Some(Ok(_)) => continue,
            Some(Err(err)) => {
                warn!(error = %err, "websocket receive error");
                state.delete_transfer_files(&id);
                let _ = send_error(&mut ws_tx, "websocket receive error").await;
                return;
            }
        }
    }

    if manifest.received_size != manifest.size {
        state.delete_transfer_files(&id);
        let _ = send_error(&mut ws_tx, "payload size mismatch").await;
        return;
    }

    manifest.complete = true;
    if write_manifest_file(&manifest_tmp_path, &manifest).await.is_err() {
        state.delete_transfer_files(&id);
        let _ = send_error(&mut ws_tx, "failed to finalize manifest").await;
        return;
    }

    let chunk_final_dir = state.chunk_dir(&id);
    let manifest_final_path = state.manifest_path(&id);

    if fs::rename(&chunk_tmp_dir, &chunk_final_dir).await.is_err() {
        state.delete_transfer_files(&id);
        let _ = send_error(&mut ws_tx, "failed to finalize chunks").await;
        return;
    }

    if fs::rename(&manifest_tmp_path, &manifest_final_path).await.is_err() {
        state.delete_transfer_files(&id);
        let _ = send_error(&mut ws_tx, "failed to finalize manifest").await;
    }
}

async fn write_manifest_file(path: &Path, manifest: &FileManifest) -> Result<(), std::io::Error> {
    fs::write(path, manifest.to_text()).await
}

fn compute_expiration(init: &SenderInit, now: u64) -> Result<u64, &'static str> {
    const MAX_TTL_SECONDS: u64 = 60 * 60 * 24 * 30;
    const DEFAULT_TTL_SECONDS: u64 = 60 * 60 * 24;

    if let Some(expires_at_unix) = init.expires_at_unix {
        if expires_at_unix <= now {
            return Err("expires_at_unix must be in the future");
        }
        if expires_at_unix.saturating_sub(now) > MAX_TTL_SECONDS {
            return Err("expires_at_unix cannot exceed 30 days from now");
        }
        return Ok(expires_at_unix);
    }

    let ttl = init.expires_in_seconds.unwrap_or(DEFAULT_TTL_SECONDS);
    if ttl == 0 {
        return Err("expires_in_seconds must be greater than 0");
    }
    if ttl > MAX_TTL_SECONDS {
        return Err("expires_in_seconds cannot exceed 30 days");
    }
    Ok(now.saturating_add(ttl))
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
