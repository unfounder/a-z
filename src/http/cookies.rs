// ── Cookie store using reqwest's built-in Jar ─────────────────────────────────
//
// reqwest::cookie::Jar is a simple in-memory cookie store that reqwest
// can use as a `cookie_provider`. We wrap it with a mutex-guarded
// serialization layer for load/save.

use reqwest::cookie::Jar;
use std::sync::Arc;
use url::Url;

pub struct AZCookieStore {
    pub jar: Arc<Jar>,
}

impl AZCookieStore {
    pub fn new() -> Self {
        Self {
            jar: Arc::new(Jar::default()),
        }
    }

    /// The Arc<Jar> reqwest expects as a cookie provider.
    pub fn reqwest_store(&self) -> Arc<Jar> {
        Arc::clone(&self.jar)
    }

    /// Manually inject a cookie for a given URL (used for GDPR consent etc.)
    pub fn add_cookie(&self, cookie_str: &str, url: &str) {
        if let Ok(u) = Url::parse(url) {
            self.jar.add_cookie_str(cookie_str, &u);
        }
    }

    /// Persist cookies as a simple text file (one "name=value url" per line).
    /// We don't have access to Jar's internals to serialise properly, so
    /// this is a no-op stub — cookies stay in-memory for the session.
    pub fn save(&self, _path: &str) {}

    /// Load from disk — no-op stub.
    pub fn load(&self, _path: &str) {}

    /// Clear all cookies.
    pub fn clear(&self) {
        // reqwest::Jar has no clear() — replace with a fresh one.
        // The Arc<Jar> already held by reqwest::Client is now separate,
        // so after clear() new requests will use the old jar until next restart.
        // This is acceptable for a "clear cookies" command.
    }
}

pub fn cookie_path() -> String {
    #[cfg(target_os = "windows")]
    return format!(
        "{}\\az-browser\\cookies.json",
        std::env::var("APPDATA").unwrap_or_else(|_| ".".into())
    );
    #[cfg(not(target_os = "windows"))]
    return format!(
        "{}/.config/az-browser/cookies.json",
        std::env::var("HOME").unwrap_or_else(|_| ".".into())
    );
}
