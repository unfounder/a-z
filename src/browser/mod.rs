pub mod assets;
pub mod cache;
pub mod pipeline;
pub mod session;
pub mod spa;
pub mod video;

use crate::dom::document::Document;
use crate::dom::parser::HtmlParser;
use crate::error::BrowserResult;
use crate::http::client::HttpClient;
use crate::http::cookies::{cookie_path, AZCookieStore};
use crate::js::runtime::{spawn_page_runtime, PageRuntime};
use crate::network::NetworkController;
use assets::{extract_assets, PageAssets};
use cache::NavCache;
use pipeline::Pipeline;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Debug, Clone, PartialEq)]
pub enum DocumentReadyState {
    Loading,
    Complete,
}

pub struct PageState {
    pub url: String,
    pub document: Document,
    pub assets: PageAssets,
    pub frame_id: String,
    pub loader_id: String,
    pub timestamp: f64,
    /// Lifecycle state — Loading while scripts run, Complete once DOM is ready.
    pub ready_state: DocumentReadyState,
    /// Persistent per-page JS runtime with live DOM bindings.
    /// None if script execution failed or timed out.
    pub page_runtime: Option<PageRuntime>,
}

pub struct Browser {
    pub http: Arc<HttpClient>,
    parser: Arc<HtmlParser>,
    pub page_state: Arc<Mutex<Option<PageState>>>,
    pub network: Arc<NetworkController>,
    cache: NavCache,
    cookie_store: Arc<AZCookieStore>,
}

impl Browser {
    pub fn new() -> Self {
        let cookie_store = Arc::new(AZCookieStore::new());
        cookie_store.load(&cookie_path());
        let network = Arc::new(NetworkController::new());

        Browser {
            http: Arc::new(HttpClient::new(Arc::clone(&cookie_store), Arc::clone(&network))),
            parser: Arc::new(HtmlParser::new()),
            page_state: Arc::new(Mutex::new(None)),
            network,
            cache: NavCache::new(),
            cookie_store,
        }
    }

    pub async fn navigate(&self, url: &str) -> BrowserResult<String> {
        self.cache.evict_expired();

        let pipeline = Pipeline::new(Arc::clone(&self.http), Arc::clone(&self.parser));
        let (html, doc_opt) = tokio::time::timeout(
            std::time::Duration::from_secs(12),
            pipeline.execute_with_state(url),
        )
        .await
        .map_err(|_| crate::error::BrowserError::Timeout(12_000))??;

        if let Some(doc) = doc_opt {
            // ── Mark Loading ──────────────────────────────────────────────────
            // Signal to concurrent CDP callers that a new page is in flight.
            {
                let mut state = self.page_state.lock().unwrap();
                if let Some(ref mut s) = *state {
                    s.ready_state = DocumentReadyState::Loading;
                }
            }

            // ── Collect + fetch scripts ───────────────────────────────────────
            let script_requests = collect_script_requests(&doc, url);
            let scripts = materialize_scripts(Arc::clone(&self.http), script_requests).await;
            log::info!("JS: spawning persistent runtime for {} ({} script(s))", url, scripts.len());

            // ── Spawn persistent JS runtime ───────────────────────────────────
            // spawn_page_runtime runs scripts, keeps the context alive, and returns
            // the post-script DOM snapshot plus a handle to the live JS thread.
            // All future Runtime.evaluate calls are routed through that handle so
            // they execute inside the same context where the DOM bindings live.
            let (doc, page_runtime) = match tokio::time::timeout(
                std::time::Duration::from_secs(8),
                spawn_page_runtime(scripts, doc, url.to_string(), Arc::clone(&self.network)),
            )
            .await
            {
                Ok(Ok((mutated_doc, runtime))) => {
                    log::info!("JS: persistent runtime ready for {}", url);
                    (mutated_doc, Some(runtime))
                }
                Ok(Err(e)) => {
                    log::warn!("JS: runtime failed for {}: {}", url, e);
                    let fallback = HtmlParser::new()
                        .parse(&html, url)
                        .unwrap_or_else(|_| Document::new());
                    (fallback, None)
                }
                Err(_) => {
                    log::warn!("JS: runtime timed out for {}", url);
                    let fallback = HtmlParser::new()
                        .parse(&html, url)
                        .unwrap_or_else(|_| Document::new());
                    (fallback, None)
                }
            };

            // ── Store final state as Complete ─────────────────────────────────
            let page_assets = extract_assets(&doc, url);
            let mut state = self.page_state.lock().unwrap();
            *state = Some(PageState {
                url: url.to_string(),
                document: doc,
                assets: page_assets,
                frame_id: "main-frame".to_string(),
                loader_id: uuid::Uuid::new_v4().to_string(),
                timestamp: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs_f64(),
                ready_state: DocumentReadyState::Complete,
                page_runtime,
            });
        }

        self.cache.put(url.to_string(), html.clone());
        self.cookie_store.save(&cookie_path());

        Ok(html)
    }

    pub async fn reload(&self, url: &str) -> BrowserResult<String> {
        self.cache.invalidate(url);
        self.navigate(url).await
    }

    pub fn current_assets(&self) -> Option<PageAssets> {
        self.page_state
            .lock()
            .ok()?
            .as_ref()
            .map(|s| s.assets.clone())
    }

    pub fn clear_cookies(&self) {
        self.cookie_store.clear();
        let _ = std::fs::remove_file(cookie_path());
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

const MAX_EXTERNAL_SCRIPTS: usize = 64;
const MAX_SINGLE_SCRIPT_BYTES: usize = 12 * 1024 * 1024;
const MAX_TOTAL_SCRIPT_BYTES: usize = 32 * 1024 * 1024;
const EXTERNAL_SCRIPT_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, PartialEq, Eq)]
enum ScriptRequest {
    Inline(String),
    External(String),
}

/// Collect script execution requests in document order.
/// Includes inline scripts and external `src` scripts (except ES modules).
fn collect_script_requests(doc: &Document, base_url: &str) -> Vec<ScriptRequest> {
    use crate::dom::node::NodeData;
    let mut out = Vec::new();
    let mut seen_external = HashSet::new();

    for node in &doc.nodes {
        if let NodeData::Element {
            tag_name, attrs, ..
        } = &node.data
        {
            if tag_name != "script" {
                continue;
            }

            let is_module = attrs
                .iter()
                .any(|a| a.name == "type" && a.value.to_ascii_lowercase().contains("module"));
            if is_module {
                continue;
            }

            if let Some(src) = attrs
                .iter()
                .find(|a| a.name == "src")
                .map(|a| a.value.as_str())
            {
                if let Some(resolved) = resolve_script_url(src, base_url) {
                    if seen_external.insert(resolved.clone()) {
                        out.push(ScriptRequest::External(resolved));
                    }
                }
                continue;
            }

            let text: String = node
                .children
                .iter()
                .filter_map(|&cid| doc.nodes.get(cid))
                .filter_map(|n| {
                    if let NodeData::Text { content } = &n.data {
                        Some(content.clone())
                    } else {
                        None
                    }
                })
                .collect();
            let trimmed = text.trim().to_string();
            if !trimmed.is_empty() {
                out.push(ScriptRequest::Inline(trimmed));
            }
        }
    }
    out
}

fn resolve_script_url(src: &str, base_url: &str) -> Option<String> {
    let src = src.trim();
    if src.is_empty() {
        return None;
    }

    let lower = src.to_ascii_lowercase();
    if lower.starts_with("data:") || lower.starts_with("javascript:") {
        return None;
    }

    if src.starts_with("//") {
        let scheme = url::Url::parse(base_url)
            .ok()
            .map(|u| u.scheme().to_string())
            .unwrap_or_else(|| "https".to_string());
        return Some(format!("{}:{}", scheme, src));
    }

    if let Ok(url) = url::Url::parse(src) {
        return match url.scheme() {
            "http" | "https" | "file" => Some(url.to_string()),
            _ => None,
        };
    }

    let base = url::Url::parse(base_url).ok()?;
    base.join(src).ok().map(|u| u.to_string())
}

async fn materialize_scripts(http: Arc<HttpClient>, requests: Vec<ScriptRequest>) -> Vec<String> {
    let mut out = Vec::new();
    let mut external_seen = 0usize;
    let mut total_bytes = 0usize;

    for request in requests {
        match request {
            ScriptRequest::Inline(code) => {
                if !code.is_empty() {
                    out.push(code);
                }
            }
            ScriptRequest::External(url) => {
                if external_seen >= MAX_EXTERNAL_SCRIPTS {
                    log::debug!("JS: external script cap reached; skipping {}", url);
                    continue;
                }
                external_seen += 1;

                let bytes =
                    match tokio::time::timeout(EXTERNAL_SCRIPT_TIMEOUT, http.fetch_bytes(&url))
                        .await
                    {
                        Ok(Ok(bytes)) => bytes,
                        Ok(Err(err)) => {
                            log::debug!("JS: failed to fetch external script {}: {}", url, err);
                            continue;
                        }
                        Err(_) => {
                            log::debug!("JS: timed out fetching external script {}", url);
                            continue;
                        }
                    };

                if bytes.is_empty() {
                    continue;
                }
                if bytes.len() > MAX_SINGLE_SCRIPT_BYTES {
                    log::debug!(
                        "JS: skipping oversized external script {} ({} bytes)",
                        url,
                        bytes.len()
                    );
                    continue;
                }
                if total_bytes + bytes.len() > MAX_TOTAL_SCRIPT_BYTES {
                    log::debug!(
                        "JS: total external script budget exceeded; stopping at {}",
                        url
                    );
                    break;
                }

                total_bytes += bytes.len();
                let script = String::from_utf8_lossy(&bytes).into_owned();
                if !script.trim().is_empty() {
                    out.push(script);
                }
            }
        }
    }

    if external_seen > 0 {
        log::debug!(
            "JS: prepared {} scripts (external fetched={}, bytes={})",
            out.len(),
            external_seen.min(MAX_EXTERNAL_SCRIPTS),
            total_bytes
        );
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dom::parser::HtmlParser;

    #[test]
    fn collects_external_and_inline_scripts() {
        let html = r#"
            <html><head>
                <script src="/a.js"></script>
                <script>window.inlineA = 1;</script>
                <script type="module" src="/esm.js"></script>
                <script src="//cdn.example.com/lib.js"></script>
                <script>window.inlineB = 2;</script>
            </head></html>
        "#;
        let doc = HtmlParser::new()
            .parse(html, "https://example.com/page")
            .expect("parse");

        let got = collect_script_requests(&doc, "https://example.com/page");
        let want = vec![
            ScriptRequest::External("https://example.com/a.js".to_string()),
            ScriptRequest::Inline("window.inlineA = 1;".to_string()),
            ScriptRequest::External("https://cdn.example.com/lib.js".to_string()),
            ScriptRequest::Inline("window.inlineB = 2;".to_string()),
        ];
        assert_eq!(got, want);
    }

    #[test]
    fn resolves_script_src_against_base() {
        assert_eq!(
            resolve_script_url("../app.js", "https://example.com/a/b/index.html"),
            Some("https://example.com/a/app.js".to_string())
        );
        assert_eq!(
            resolve_script_url("//cdn.site/x.js", "https://example.com"),
            Some("https://cdn.site/x.js".to_string())
        );
        assert_eq!(
            resolve_script_url("data:text/javascript,1", "https://example.com"),
            None
        );
    }
}
