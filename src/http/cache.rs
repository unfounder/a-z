use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;

#[derive(Clone)]
pub struct CacheEntry {
    pub content: String,
    pub content_type: String,
    pub timestamp: u64,
    pub expires: Option<u64>,
}

impl CacheEntry {
    pub fn new(content: String, content_type: String) -> Self {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        Self {
            content,
            content_type,
            timestamp,
            expires: None,
        }
    }
}

pub struct HttpCache {
    entries: Arc<RwLock<HashMap<String, CacheEntry>>>,
}

impl HttpCache {
    pub fn new() -> Self {
        Self {
            entries: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn get(&self, url: &str) -> Option<CacheEntry> {
        let entries = self.entries.read().await;
        entries.get(url).cloned()
    }

    pub async fn set(&self, url: String, entry: CacheEntry) {
        let mut entries = self.entries.write().await;
        entries.insert(url, entry);
    }

    pub async fn clear(&self) {
        let mut entries = self.entries.write().await;
        entries.clear();
    }
}
