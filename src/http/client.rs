use crate::error::{BrowserError, BrowserResult};
use crate::http::cookies::AZCookieStore;
use crate::network::NetworkController;
use reqwest::{header, Client};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;

const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) \
     AppleWebKit/537.36 (KHTML, like Gecko) \
     Chrome/120.0.0.0 Safari/537.36";

pub struct HttpClient {
    pub client: RwLock<Client>,
    pub cookies: Arc<AZCookieStore>,
    pub network: Arc<NetworkController>,
}

impl HttpClient {
    pub fn new(cookies: Arc<AZCookieStore>, network: Arc<NetworkController>) -> Self {
        let default_headers = Self::build_headers();
        let client = Client::builder()
            .use_native_tls()
            .http2_adaptive_window(true)
            .gzip(true)
            .deflate(true)
            .brotli(true)
            .user_agent(USER_AGENT)
            .default_headers(default_headers)
            .cookie_provider(cookies.jar.clone())
            .timeout(Duration::from_secs(10))
            .connect_timeout(Duration::from_secs(6))
            .pool_max_idle_per_host(10)
            .redirect(reqwest::redirect::Policy::limited(10))
            .build()
            .expect("HttpClient initial build");

        Self {
            client: RwLock::new(client),
            cookies,
            network,
        }
    }

    fn build_headers() -> header::HeaderMap {
        let mut default_headers = header::HeaderMap::new();
        default_headers.insert(
            header::ACCEPT,
            "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8"
                .parse()
                .unwrap(),
        );
        default_headers.insert(header::ACCEPT_LANGUAGE, "en-US,en;q=0.9".parse().unwrap());
        default_headers.insert(
            header::ACCEPT_ENCODING,
            "gzip, deflate, br".parse().unwrap(),
        );
        default_headers.insert("Upgrade-Insecure-Requests", "1".parse().unwrap());
        default_headers.insert("Sec-Fetch-Dest", "document".parse().unwrap());
        default_headers.insert("Sec-Fetch-Mode", "navigate".parse().unwrap());
        default_headers.insert("Sec-Fetch-Site", "none".parse().unwrap());
        default_headers.insert("Sec-Fetch-User", "?1".parse().unwrap());
        default_headers.insert("DNT", "1".parse().unwrap());
        default_headers
    }

    pub async fn refresh_network(&self) {
        if let Ok(new_client) = self.network.get_client().await {
            let mut guard = self.client.write().await;
            *guard = new_client;
        }
    }

    pub async fn fetch_html(&self, url: &str) -> BrowserResult<String> {
        let client = self.client.read().await;
        let mut req = client.get(url);

        if let Ok(parsed) = url::Url::parse(url) {
            let host = parsed.host_str().unwrap_or("");

            // GDPR consent for Google/YouTube
            if host.ends_with("youtube.com") || host.ends_with("google.com") {
                req = req.header("Cookie", "SOCS=CAE=; CONSENT=YES+cb");
            }

            // Amazon needs a real browser Accept header or it returns a bot-detection page
            if host.ends_with("amazon.com")
                || host.ends_with("amazon.in")
                || host.ends_with("amazon.co.uk")
                || host.ends_with("amazon.de")
            {
                req = req
                    .header("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,image/apng,*/*;q=0.8")
                    .header("Accept-Language", "en-US,en;q=0.9")
                    .header("Cache-Control", "no-cache")
                    .header("Pragma", "no-cache")
                    .header("Sec-Ch-Ua", r#""Chromium";v="120", "Google Chrome";v="120", "Not-A.Brand";v="99""#)
                    .header("Sec-Ch-Ua-Mobile", "?0")
                    .header("Sec-Ch-Ua-Platform", r#""Windows""#);
            }
        }

        let resp = req
            .send()
            .await
            .map_err(|e| BrowserError::Http(e.to_string()))?;

        resp.text()
            .await
            .map_err(|e| BrowserError::Http(e.to_string()))
    }

    pub async fn fetch_bytes(&self, url: &str) -> BrowserResult<Vec<u8>> {
        let client = self.client.read().await;
        let resp = client
            .get(url)
            .send()
            .await
            .map_err(|e| BrowserError::Http(e.to_string()))?;

        resp.bytes()
            .await
            .map(|b| b.to_vec())
            .map_err(|e| BrowserError::Http(e.to_string()))
    }

    pub async fn post_form(
        &self,
        url: &str,
        fields: Vec<(String, String)>,
    ) -> BrowserResult<String> {
        let client = self.client.read().await;
        let resp = client
            .post(url)
            .form(&fields)
            .send()
            .await
            .map_err(|e| BrowserError::Http(e.to_string()))?;

        resp.text()
            .await
            .map_err(|e| BrowserError::Http(e.to_string()))
    }
}
