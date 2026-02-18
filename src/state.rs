use bytes::Bytes;
use dashmap::DashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};

pub const CHANNEL_BUFFER: usize = 16;

#[derive(Clone)]
pub struct AppState {
    pub transfers: Arc<DashMap<String, TransferState>>,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            transfers: Arc::new(DashMap::new()),
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FileMetadata {
    pub filename: String,
    pub size: u64,
    #[serde(default)]
    pub mime_type: String,
}

pub enum TransferState {
    WaitingForRecipient {
        metadata: FileMetadata,
        recipient_tx: oneshot::Sender<RecipientLink>,
    },
    Reconnecting {
        metadata: FileMetadata,
        recipient_tx: oneshot::Sender<RecipientLink>,
    },
    Active,
}

pub struct RecipientLink {
    pub data_tx: mpsc::Sender<RelayMessage>,
    pub cancel_rx: mpsc::Receiver<()>,
    pub resume_offset: u64,
}

pub enum RelayMessage {
    Data(Bytes),
    Finished,
    Error(String),
}
