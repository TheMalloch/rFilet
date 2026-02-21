use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Clone)]
pub struct AppState {
    pub manifests_dir: Arc<PathBuf>,
    pub chunks_dir: Arc<PathBuf>,
}

impl AppState {
    pub fn new() -> Self {
        let base = std::env::var("FILET_STORAGE_DIR").unwrap_or_else(|_| "./data".to_string());
        let manifests_dir = PathBuf::from(&base).join("manifests");
        let chunks_dir = PathBuf::from(base).join("chunks");

        let _ = std::fs::create_dir_all(&manifests_dir);
        let _ = std::fs::create_dir_all(&chunks_dir);

        Self {
            manifests_dir: Arc::new(manifests_dir),
            chunks_dir: Arc::new(chunks_dir),
        }
    }

    pub fn manifest_path(&self, id: &str) -> PathBuf {
        self.manifests_dir.join(format!("{id}.txt"))
    }

    pub fn manifest_tmp_path(&self, id: &str) -> PathBuf {
        self.manifests_dir.join(format!("{id}.tmp.txt"))
    }

    pub fn chunk_dir(&self, id: &str) -> PathBuf {
        self.chunks_dir.join(id)
    }

    pub fn chunk_tmp_dir(&self, id: &str) -> PathBuf {
        self.chunks_dir.join(format!("{id}.tmp"))
    }

    pub fn delete_transfer_files(&self, id: &str) {
        let _ = std::fs::remove_file(self.manifest_path(id));
        let _ = std::fs::remove_file(self.manifest_tmp_path(id));
        let _ = std::fs::remove_dir_all(self.chunk_dir(id));
        let _ = std::fs::remove_dir_all(self.chunk_tmp_dir(id));
    }

    pub fn load_manifest(&self, id: &str) -> Option<FileManifest> {
        let content = std::fs::read_to_string(self.manifest_path(id)).ok()?;
        FileManifest::parse(&content).ok()
    }

    pub fn purge_expired(&self) -> usize {
        let now = unix_now();
        let mut deleted = 0usize;

        let entries = match std::fs::read_dir(&*self.manifests_dir) {
            Ok(entries) => entries,
            Err(_) => return 0,
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|v| v.to_str()) != Some("txt") {
                continue;
            }
            if path
                .file_name()
                .and_then(|v| v.to_str())
                .map(|name| name.ends_with(".tmp.txt"))
                .unwrap_or(false)
            {
                continue;
            }

            let content = match std::fs::read_to_string(&path) {
                Ok(content) => content,
                Err(_) => continue,
            };

            let manifest = match FileManifest::parse(&content) {
                Ok(manifest) => manifest,
                Err(_) => continue,
            };

            if manifest.expires_at_unix <= now {
                self.delete_transfer_files(&manifest.id);
                deleted += 1;
            }
        }

        deleted
    }
}

pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::from_secs(0))
        .as_secs()
}

#[derive(Clone)]
pub struct FileManifest {
    pub id: String,
    pub filename: String,
    pub size: u64,
    pub created_at_unix: u64,
    pub expires_at_unix: u64,
    pub chunk_size: u64,
    pub chunk_count: u64,
    pub received_size: u64,
    pub complete: bool,
}

impl FileManifest {
    pub fn to_text(&self) -> String {
        [
            format!("id={}", self.id),
            format!("filename={}", self.filename),
            format!("size={}", self.size),
            format!("created_at_unix={}", self.created_at_unix),
            format!("expires_at_unix={}", self.expires_at_unix),
            format!("chunk_size={}", self.chunk_size),
            format!("chunk_count={}", self.chunk_count),
            format!("received_size={}", self.received_size),
            format!("complete={}", self.complete),
        ]
        .join("\n")
            + "\n"
    }

    pub fn parse(content: &str) -> Result<Self, &'static str> {
        let values: HashMap<String, String> = content
            .lines()
            .filter_map(|line| line.split_once('=').map(|(k, v)| (k.to_string(), v.to_string())))
            .collect();

        let get = |key: &str| values.get(key).cloned().ok_or("manifest missing key");

        Ok(Self {
            id: get("id")?,
            filename: get("filename")?,
            size: get("size")?.parse().map_err(|_| "invalid size")?,
            created_at_unix: get("created_at_unix")?
                .parse()
                .map_err(|_| "invalid created_at_unix")?,
            expires_at_unix: get("expires_at_unix")?
                .parse()
                .map_err(|_| "invalid expires_at_unix")?,
            chunk_size: get("chunk_size")?
                .parse()
                .map_err(|_| "invalid chunk_size")?,
            chunk_count: get("chunk_count")?
                .parse()
                .map_err(|_| "invalid chunk_count")?,
            received_size: get("received_size")?
                .parse()
                .map_err(|_| "invalid received_size")?,
            complete: get("complete")?.parse().map_err(|_| "invalid complete")?,
        })
    }
}
