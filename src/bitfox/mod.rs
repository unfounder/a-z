/// BitFox — Bose Inspection & Trust Fox
/// Inspects every URL before the pipeline and decides Allow / Block / Flag / Verify.

pub mod js_hooks;

use std::sync::{Arc, Mutex, OnceLock};

// ── Plugin trait ──────────────────────────────────────────────────────────────

/// Safe in-process plugin interface.
///
/// Implement this trait and register your plugin with [`register_plugin`].
/// All methods are called synchronously inside the request pipeline.
pub trait BitFoxPlugin: Send + Sync {
    /// Called once when the plugin is registered.
    fn on_load(&self) {}

    /// Called before every navigation.
    /// Return `Some(new_url)` to rewrite the URL, `None` to leave it unchanged.
    fn intercept_request(&self, url: &str) -> Option<String> {
        let _ = url;
        None
    }

    /// Called after a response is received.
    /// Return `Some(new_html)` to rewrite the body, `None` to leave it unchanged.
    fn intercept_response(&self, url: &str, html: &str) -> Option<String> {
        let _ = (url, html);
        None
    }
}

// ── Global registry ───────────────────────────────────────────────────────────

static REGISTRY: OnceLock<Mutex<Vec<Arc<dyn BitFoxPlugin>>>> = OnceLock::new();

fn registry() -> &'static Mutex<Vec<Arc<dyn BitFoxPlugin>>> {
    REGISTRY.get_or_init(|| Mutex::new(Vec::new()))
}

/// Register a plugin.  `on_load` is called immediately.
pub fn register_plugin(plugin: Arc<dyn BitFoxPlugin>) {
    plugin.on_load();
    registry().lock().unwrap().push(plugin);
}

/// Run all plugins' `intercept_request` hooks in registration order.
/// Each plugin sees the URL as (potentially) rewritten by the previous one.
pub fn run_request_hooks(url: &str) -> String {
    let plugins = registry().lock().unwrap();
    let mut current = url.to_string();
    for p in plugins.iter() {
        if let Some(rewritten) = p.intercept_request(&current) {
            log::debug!("BitFox plugin rewrote {} -> {}", current, rewritten);
            current = rewritten;
        }
    }
    current
}

/// Run all plugins' `intercept_response` hooks in registration order.
pub fn run_response_hooks(url: &str, html: &str) -> String {
    let plugins = registry().lock().unwrap();
    let mut current = html.to_string();
    for p in plugins.iter() {
        if let Some(rewritten) = p.intercept_response(url, &current) {
            current = rewritten;
        }
    }
    current
}

#[derive(Debug, Clone, PartialEq)]
pub enum BitFoxDecision {
    Allow,
    Block,
    Flag,
    Verify,
}

pub struct BitFoxContext {
    pub request_url: String,
    pub content_type: String,
    pub origin_signature: Option<String>,
    pub payload_hash: String,
    pub timestamp: i64,
    pub session_id: String,
}

/// Known ad / tracker / malware domains (matched as hostname suffix).
const BLOCK_DOMAINS: &[&str] = &[
    // Ad networks
    "doubleclick.net",
    "googlesyndication.com",
    "googleadservices.com",
    "adnxs.com",
    "amazon-adsystem.com",
    "adsystem.com",
    "moatads.com",
    "taboola.com",
    "outbrain.com",
    "advertising.com",
    "adsrvr.org",
    "casalemedia.com",
    "pubmatic.com",
    "rubiconproject.com",
    "openx.net",
    "servedby-buysellads.com",
    "buysellads.com",
    "carbonads.net",
    // Trackers
    "scorecardresearch.com",
    "quantserve.com",
    "chartbeat.com",
    "newrelic.com",
    "nr-data.net",
    "hotjar.com",
    "mouseflow.com",
    "crazyegg.com",
    "fullstory.com",
    "loggly.com",
    "mixpanel.com",
    "segment.com",
    "segment.io",
    "amplitude.com",
    "optimizely.com",
    "omtrdc.net",
    "demdex.net",
    "bluekai.com",
    "addthis.com",
    "addthisedge.com",
    "sharethis.com",
    "sentry.io",
    "bugsnag.com",
];

/// Ports that are suspicious enough to flag.
const SUSPICIOUS_PORTS: &[u16] = &[4444, 4445, 1337, 31337, 6667, 6668, 6669, 8888, 9001, 9030];

pub fn bitfox_inspect(ctx: BitFoxContext) -> BitFoxDecision {
    let parsed = match url::Url::parse(&ctx.request_url) {
        Ok(u) => u,
        Err(_) => return BitFoxDecision::Block,
    };

    let host = parsed.host_str().unwrap_or("").to_ascii_lowercase();
    let scheme = parsed.scheme();
    let port = parsed.port();

    // 1. Block IP addresses used for SSRF / local network scanning
    if is_ip_address(&host) {
        if is_private_ip(&host) {
            log::warn!("BitFox: blocked private-IP request {}", ctx.request_url);
            return BitFoxDecision::Block;
        }
        return BitFoxDecision::Flag; // public IP — unusual but not blocked
    }

    // 2. Block localhost variants
    if host == "localhost"
        || host == "127.0.0.1"
        || host.starts_with("127.")
        || host == "[::1]"
        || host.ends_with(".local")
    {
        log::warn!("BitFox: blocked localhost {}", ctx.request_url);
        return BitFoxDecision::Block;
    }

    // 3. Block known ad / tracker / malware domains
    for &blocked in BLOCK_DOMAINS {
        if host == blocked || host.ends_with(&format!(".{}", blocked)) {
            log::info!("BitFox: blocked {} (rule: {})", host, blocked);
            return BitFoxDecision::Block;
        }
    }

    // 4. Flag suspicious ports
    if let Some(p) = port {
        if SUSPICIOUS_PORTS.contains(&p) {
            log::warn!("BitFox: flagged port {} in {}", p, ctx.request_url);
            return BitFoxDecision::Flag;
        }
    }

    // 5. Flag non-HTTP(S) schemes reaching the engine
    if scheme != "http" && scheme != "https" {
        return BitFoxDecision::Flag;
    }

    // 6. Flag very long hostnames (DGA indicator)
    if host.len() > 80 {
        return BitFoxDecision::Flag;
    }

    BitFoxDecision::Allow
}

fn is_ip_address(host: &str) -> bool {
    // IPv4: four dot-separated octets all parseable as u8
    let parts: Vec<&str> = host.split('.').collect();
    if parts.len() == 4 && parts.iter().all(|o| o.parse::<u8>().is_ok()) {
        return true;
    }
    // IPv6 bracket form
    host.starts_with('[') && host.ends_with(']')
}

fn is_private_ip(host: &str) -> bool {
    let parts: Vec<u8> = host.split('.').filter_map(|o| o.parse().ok()).collect();
    if parts.len() != 4 {
        return false;
    }
    matches!(
        (parts[0], parts[1]),
        (10, _) | (172, 16..=31) | (192, 168) | (127, _) | (169, 254)
    )
}
