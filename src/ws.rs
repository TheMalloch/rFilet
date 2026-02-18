use axum::extract::ws::{Message, WebSocket};
use futures_util::{SinkExt, StreamExt};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
use tracing::{info, warn};

use crate::state::*;

const RECONNECT_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(serde::Deserialize)]
struct SenderInit {
    filename: String,
    size: u64,
    #[serde(default)]
    mime_type: String,
}

#[derive(serde::Serialize)]
struct SenderResponse {
    r#type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    offset: Option<u64>,
}

pub async fn handle_sender(socket: WebSocket, state: AppState) {
    let (mut ws_tx, mut ws_rx) = socket.split();

    // Step 1: Wait for metadata from sender
    let metadata = loop {
        match ws_rx.next().await {
            Some(Ok(Message::Text(text))) => {
                match serde_json::from_str::<SenderInit>(&text) {
                    Ok(init) => {
                        break FileMetadata {
                            filename: init.filename,
                            size: init.size,
                            mime_type: if init.mime_type.is_empty() {
                                "application/octet-stream".to_string()
                            } else {
                                init.mime_type
                            },
                        };
                    }
                    Err(e) => {
                        let _ = ws_tx
                            .send(Message::Text(
                                serde_json::to_string(&SenderResponse {
                                    r#type: "error".into(),
                                    id: None,
                                    error: Some(format!("Invalid metadata: {e}")),
                                    offset: None,
                                })
                                .unwrap()
                                .into(),
                            ))
                            .await;
                        return;
                    }
                }
            }
            Some(Ok(Message::Close(_))) | None => return,
            _ => continue,
        }
    };

    // Step 2: Create transfer entry with oneshot for recipient signaling
    let (recipient_tx, recipient_rx) = oneshot::channel::<RecipientLink>();
    let id = nanoid::nanoid!(12);

    state.transfers.insert(
        id.clone(),
        TransferState::WaitingForRecipient {
            metadata: metadata.clone(),
            recipient_tx,
        },
    );

    // Send the transfer ID back to sender
    let _ = ws_tx
        .send(Message::Text(
            serde_json::to_string(&SenderResponse {
                r#type: "ready".into(),
                id: Some(id.clone()),
                error: None,
                offset: None,
            })
            .unwrap()
            .into(),
        ))
        .await;

    info!(transfer_id = %id, filename = %metadata.filename, size = metadata.size, "Transfer created, waiting for recipient");

    // Step 3: Wait for recipient to connect (or sender to disconnect).
    // Loop so we can ignore keepalive messages and send periodic pings to
    // keep the connection alive when the sender tab is backgrounded on mobile.
    let mut recipient_link = {
        tokio::pin!(recipient_rx);
        let mut ping_timer = tokio::time::interval(Duration::from_secs(15));
        ping_timer.tick().await; // skip the initial immediate tick
        loop {
            tokio::select! {
                result = &mut recipient_rx => {
                    match result {
                        Ok(link) => break link,
                        Err(_) => {
                            warn!(transfer_id = %id, "Recipient channel dropped");
                            state.transfers.remove(&id);
                            return;
                        }
                    }
                }
                msg = ws_rx.next() => {
                    match msg {
                        Some(Ok(Message::Close(_))) | None => {
                            info!(transfer_id = %id, "Sender disconnected while waiting");
                            state.transfers.remove(&id);
                            return;
                        }
                        _ => {} // keepalive pong or other — ignore
                    }
                }
                _ = ping_timer.tick() => {
                    let _ = ws_tx.send(Message::Ping(bytes::Bytes::new())).await;
                }
            }
        }
    };

    // Step 4: Recipient is connected, notify sender to start sending
    let _ = ws_tx
        .send(Message::Text(
            serde_json::to_string(&SenderResponse {
                r#type: "start".into(),
                id: None,
                error: None,
                offset: None,
            })
            .unwrap()
            .into(),
        ))
        .await;

    info!(transfer_id = %id, "Transfer started");

    // Step 5: Relay loop with reconnection support
    loop {
        let data_tx = recipient_link.data_tx;
        let mut cancel_rx = recipient_link.cancel_rx;

        let relay_result = relay_data(&mut ws_rx, &mut ws_tx, &data_tx, &mut cancel_rx, &id).await;

        match relay_result {
            RelayResult::Done | RelayResult::SenderDisconnected => break,
            RelayResult::RecipientDisconnected => {
                // Recipient dropped — try to let them reconnect
                let (new_tx, new_rx) = oneshot::channel::<RecipientLink>();
                state.transfers.insert(
                    id.clone(),
                    TransferState::Reconnecting {
                        metadata: metadata.clone(),
                        recipient_tx: new_tx,
                    },
                );

                let _ = ws_tx
                    .send(Message::Text(
                        serde_json::to_string(&SenderResponse {
                            r#type: "paused".into(),
                            id: None,
                            error: None,
                            offset: None,
                        })
                        .unwrap()
                        .into(),
                    ))
                    .await;

                info!(transfer_id = %id, "Recipient disconnected, waiting for reconnect");

                // Wait for reconnect, timeout, or sender disconnect.
                // Loop to ignore keepalive messages and send pings to the sender.
                let new_link = {
                    tokio::pin!(new_rx);
                    let sleep = tokio::time::sleep(RECONNECT_TIMEOUT);
                    tokio::pin!(sleep);
                    let mut ping_timer = tokio::time::interval(Duration::from_secs(15));
                    ping_timer.tick().await;
                    loop {
                        tokio::select! {
                            result = &mut new_rx => {
                                match result {
                                    Ok(link) => break Some(link),
                                    Err(_) => break None,
                                }
                            }
                            _ = &mut sleep => {
                                info!(transfer_id = %id, "Reconnect timeout");
                                state.transfers.remove(&id);
                                break None;
                            }
                            msg = ws_rx.next() => {
                                match msg {
                                    Some(Ok(Message::Close(_))) | None => {
                                        info!(transfer_id = %id, "Sender disconnected while waiting for reconnect");
                                        state.transfers.remove(&id);
                                        break None;
                                    }
                                    _ => {} // keepalive pong — ignore
                                }
                            }
                            _ = ping_timer.tick() => {
                                let _ = ws_tx.send(Message::Ping(bytes::Bytes::new())).await;
                            }
                        }
                    }
                };

                match new_link {
                    Some(link) => {
                        // Tell sender to resume from the receiver's offset
                        let _ = ws_tx
                            .send(Message::Text(
                                serde_json::to_string(&SenderResponse {
                                    r#type: "resume".into(),
                                    id: None,
                                    error: None,
                                    offset: Some(link.resume_offset),
                                })
                                .unwrap()
                                .into(),
                            ))
                            .await;

                        info!(transfer_id = %id, offset = link.resume_offset, "Recipient reconnected, resuming");
                        state.transfers.insert(id.clone(), TransferState::Active);
                        recipient_link = link;
                        // Continue outer loop — restart relay
                    }
                    None => {
                        // Give up
                        let _ = ws_tx
                            .send(Message::Text(
                                serde_json::to_string(&SenderResponse {
                                    r#type: "cancelled".into(),
                                    id: None,
                                    error: Some("Recipient disconnected".into()),
                                    offset: None,
                                })
                                .unwrap()
                                .into(),
                            ))
                            .await;
                        break;
                    }
                }
            }
        }
    }

    state.transfers.remove(&id);
}

enum RelayResult {
    Done,
    SenderDisconnected,
    RecipientDisconnected,
}

async fn relay_data(
    ws_rx: &mut futures_util::stream::SplitStream<WebSocket>,
    _ws_tx: &mut futures_util::stream::SplitSink<WebSocket, Message>,
    data_tx: &mpsc::Sender<RelayMessage>,
    cancel_rx: &mut mpsc::Receiver<()>,
    id: &str,
) -> RelayResult {
    loop {
        tokio::select! {
            msg = ws_rx.next() => {
                match msg {
                    Some(Ok(Message::Binary(data))) => {
                        if data_tx.send(RelayMessage::Data(data)).await.is_err() {
                            warn!(transfer_id = %id, "Recipient channel closed during relay");
                            return RelayResult::RecipientDisconnected;
                        }
                    }
                    Some(Ok(Message::Text(text))) => {
                        if let Ok(val) = serde_json::from_str::<serde_json::Value>(&*text) {
                            if val.get("type").and_then(|t| t.as_str()) == Some("done") {
                                let _ = data_tx.send(RelayMessage::Finished).await;
                                info!(transfer_id = %id, "Transfer complete");
                                return RelayResult::Done;
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => {
                        let _ = data_tx.send(RelayMessage::Error("Sender disconnected".into())).await;
                        warn!(transfer_id = %id, "Sender disconnected during transfer");
                        return RelayResult::SenderDisconnected;
                    }
                    _ => continue,
                }
            }
            _ = cancel_rx.recv() => {
                info!(transfer_id = %id, "Recipient disconnected during transfer");
                return RelayResult::RecipientDisconnected;
            }
        }
    }
}

pub async fn handle_receiver(socket: WebSocket, id: String, state: AppState, resume_offset: u64) {
    let (mut ws_tx, mut ws_rx) = socket.split();

    // Atomically remove the transfer from the map
    let entry = state.transfers.remove(&id);
    let (metadata, recipient_tx) = match entry {
        Some((
            _,
            TransferState::WaitingForRecipient {
                metadata,
                recipient_tx,
            },
        )) => (metadata, recipient_tx),
        Some((
            _,
            TransferState::Reconnecting {
                metadata,
                recipient_tx,
            },
        )) => (metadata, recipient_tx),
        _ => {
            let _ = ws_tx
                .send(Message::Text(
                    r#"{"type":"error","error":"Transfer not found or already claimed"}"#
                        .to_string()
                        .into(),
                ))
                .await;
            return;
        }
    };

    // Create the relay channel
    let (data_tx, mut data_rx) = mpsc::channel::<RelayMessage>(CHANNEL_BUFFER);
    let (cancel_tx, cancel_rx) = mpsc::channel::<()>(1);

    // Send metadata to recipient
    let _ = ws_tx
        .send(Message::Text(
            serde_json::to_string(&serde_json::json!({
                "type": "metadata",
                "filename": metadata.filename,
                "size": metadata.size,
                "mime_type": metadata.mime_type,
            }))
            .unwrap()
            .into(),
        ))
        .await;

    // Signal the sender that recipient is ready
    let link = RecipientLink {
        data_tx,
        cancel_rx,
        resume_offset,
    };

    if recipient_tx.send(link).is_err() {
        let _ = ws_tx
            .send(Message::Text(
                r#"{"type":"error","error":"Sender disconnected"}"#
                    .to_string()
                    .into(),
            ))
            .await;
        return;
    }

    // Mark as active
    state.transfers.insert(id.clone(), TransferState::Active);

    info!(transfer_id = %id, resume_offset, "Recipient connected, relaying data");

    // Relay data from mpsc channel to recipient WS
    loop {
        tokio::select! {
            msg = data_rx.recv() => {
                match msg {
                    Some(RelayMessage::Data(data)) => {
                        if ws_tx.send(Message::Binary(data)).await.is_err() {
                            warn!(transfer_id = %id, "Failed to send to recipient");
                            let _ = cancel_tx.send(()).await;
                            break;
                        }
                    }
                    Some(RelayMessage::Finished) => {
                        let _ = ws_tx.send(Message::Text(
                            r#"{"type":"done"}"#.to_string().into()
                        )).await;
                        info!(transfer_id = %id, "Transfer delivered to recipient");
                        break;
                    }
                    Some(RelayMessage::Error(e)) => {
                        let _ = ws_tx.send(Message::Text(
                            serde_json::to_string(&serde_json::json!({
                                "type": "error",
                                "error": e,
                            })).unwrap().into()
                        )).await;
                        break;
                    }
                    None => {
                        let _ = ws_tx.send(Message::Text(
                            r#"{"type":"error","error":"Sender disconnected"}"#.to_string().into()
                        )).await;
                        break;
                    }
                }
            }
            msg = ws_rx.next() => {
                match msg {
                    Some(Ok(Message::Close(_))) | None => {
                        info!(transfer_id = %id, "Recipient disconnected");
                        let _ = cancel_tx.send(()).await;
                        break;
                    }
                    _ => continue,
                }
            }
        }
    }

    // Don't mark as Done here — the sender handler decides
    // (it may transition to Reconnecting instead)
}
