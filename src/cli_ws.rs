use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use axum::extract::ws::{Message, WebSocket};
use futures_util::{SinkExt, StreamExt};
use rand::{rngs::OsRng, RngCore};
use tokio::io::AsyncReadExt;
use tracing::{info, warn};

use crate::cli_state::CliState;

const CHUNK_SIZE: usize = 1024 * 1024; // 1MB

pub async fn handle_ws_download(socket: WebSocket, token: String, state: CliState) {
    let (mut ws_tx, _ws_rx) = socket.split();

    let entry = match state.files.get(&token) {
        Some(e) => e,
        None => {
            let _ = ws_tx
                .send(Message::Text(
                    r#"{"type":"error","error":"File not found"}"#.into(),
                ))
                .await;
            return;
        }
    };

    let path = entry.path.clone();
    let filename = entry.filename.clone();
    let size = entry.size;
    let mime_type = entry.mime_type.clone();
    let enc_key = entry.enc_key;
    drop(entry);

    // Send metadata as first message
    let meta_msg = serde_json::json!({
        "type": "metadata",
        "filename": filename,
        "size": size,
        "mime_type": mime_type,
    });
    if ws_tx
        .send(Message::Text(meta_msg.to_string().into()))
        .await
        .is_err()
    {
        return;
    }

    let cipher = Aes256Gcm::new_from_slice(&enc_key).unwrap();

    let mut file = match tokio::fs::File::open(&path).await {
        Ok(f) => f,
        Err(e) => {
            let _ = ws_tx
                .send(Message::Text(
                    serde_json::json!({"type":"error","error":format!("Failed to open file: {e}")})
                        .to_string()
                        .into(),
                ))
                .await;
            return;
        }
    };

    let mut buf = vec![0u8; CHUNK_SIZE];

    loop {
        let n = match file.read(&mut buf).await {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) => {
                warn!("Error reading file: {e}");
                let _ = ws_tx
                    .send(Message::Text(
                        serde_json::json!({"type":"error","error":"Read error"})
                            .to_string()
                            .into(),
                    ))
                    .await;
                return;
            }
        };

        // Generate random 12-byte nonce
        let mut nonce_bytes = [0u8; 12];
        OsRng.fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);

        let ciphertext = match cipher.encrypt(nonce, &buf[..n]) {
            Ok(ct) => ct,
            Err(e) => {
                warn!("Encryption error: {e}");
                return;
            }
        };

        // Prepend nonce to ciphertext (same format as web version)
        let mut payload = Vec::with_capacity(12 + ciphertext.len());
        payload.extend_from_slice(&nonce_bytes);
        payload.extend_from_slice(&ciphertext);

        if ws_tx
            .send(Message::Binary(payload.into()))
            .await
            .is_err()
        {
            warn!("Client disconnected during transfer");
            return;
        }
    }

    let _ = ws_tx
        .send(Message::Text(r#"{"type":"done"}"#.into()))
        .await;

    info!(token = %token, filename = %filename, "CLI transfer complete");
}
