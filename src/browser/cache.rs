use dashmap::DashMap;
/// Phase 4: Navigation cache — deduplicates identical URL requests within TTL.
/// Each entry stores the serialized HTML output. The live Document stays in
/// PageState; this cache only stores the rendered string to avoid re-running
/// the full pipeline for repeated loads of the same URL (e.g. page refresh
/// within a short window, MCP polling, CDP reload).
use std::sync::Arc;
use std::time::{Duration, Instant};

const DEFAULT_TTL: Duration = Duration::from_secs(5);

struct CacheEntry {
    html: String,
    created: Instant,
}

pub struct NavCache {
    map: Arc<DashMap<String, CacheEntry>>,
    ttl: Duration,
}

impl NavCache {
    pub fn new() -> Self {
        NavCache {
            map: Arc::new(DashMap::new()),
            ttl: DEFAULT_TTL,
        }
    }

    /// Return cached HTML if still fresh, otherwise None.
    pub fn get(&self, url: &str) -> Option<String> {
        let entry = self.map.get(url)?;
        if entry.created.elapsed() < self.ttl {
            Some(entry.html.clone())
        } else {
            drop(entry);
            self.map.remove(url);
            None
        }
    }

    /// Store a rendered HTML string for the given URL.
    pub fn put(&self, url: String, html: String) {
        self.map.insert(
            url,
            CacheEntry {
                html,
                created: Instant::now(),
            },
        );
    }

    /// Invalidate a specific URL (e.g. after a POST or explicit reload).
    pub fn invalidate(&self, url: &str) {
        self.map.remove(url);
    }

    /// Drop all entries older than TTL.
    pub fn evict_expired(&self) {
        self.map.retain(|_, v| v.created.elapsed() < self.ttl);
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }
}

impl Default for NavCache {
    fn default() -> Self {
        Self::new()
    }
}
