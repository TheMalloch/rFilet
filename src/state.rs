use bytes::Bytes;
use dashmap::DashMap;
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
    pub blobs: Arc<DashMap<String, StoredBlob>>,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            blobs: Arc::new(DashMap::new()),
        }
    }
}

#[derive(Clone)]
pub struct StoredBlob {
    pub filename: String,
    pub size: u64,
    pub bytes: Bytes,
}
