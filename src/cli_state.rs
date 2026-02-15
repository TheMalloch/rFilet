use dashmap::DashMap;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Clone)]
pub struct CliState {
    pub files: Arc<DashMap<String, SharedFile>>,
}

impl CliState {
    pub fn new() -> Self {
        Self {
            files: Arc::new(DashMap::new()),
        }
    }
}

pub struct SharedFile {
    pub path: PathBuf,
    pub filename: String,
    pub size: u64,
    pub mime_type: String,
    pub enc_key: [u8; 32],
}
