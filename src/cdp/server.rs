use super::session::CdpSession;
use crate::browser::Browser;
use anyhow::Result;
use futures_util::{SinkExt, StreamExt};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::{accept_async, tungstenite::Message};

pub struct CdpServer {
    browser: Arc<Browser>,
    browser_id: String,
    target_id: String,
}

impl CdpServer {
    pub fn new(browser: Arc<Browser>) -> Self {
        CdpServer {
            browser,
            browser_id: uuid::Uuid::new_v4().to_string(),
            target_id: uuid::Uuid::new_v4().to_string(),
        }
    }

    pub async fn listen(self: Arc<Self>, addr: &str) -> Result<()> {
        let listener = TcpListener::bind(addr).await?;
        log::info!("CDP server listening on ws://{}", addr);
        loop {
            let (stream, peer) = listener.accept().await?;
            log::debug!("CDP connection from {}", peer);
            let me = Arc::clone(&self);
            tokio::spawn(async move {
                if let Err(e) = me.handle_tcp(stream).await {
                    log::debug!("CDP connection closed: {}", e);
                }
            });
        }
    }

    async fn handle_tcp(&self, stream: TcpStream) -> Result<()> {
        // Reject connections from anything other than localhost.
        // Wildcard CORS would allow any web page on the machine to control the browser.
        let peer = stream.peer_addr()?;
        let peer_ip = peer.ip();
        let is_loopback = peer_ip.is_loopback();
        if !is_loopback {
            log::warn!("CDP: rejected non-loopback connection from {}", peer);
            return Ok(());
        }

        // Peek at request headers without consuming them
        let mut peek = [0u8; 4096];
        let n = stream.peek(&mut peek).await?;
        let head = std::str::from_utf8(&peek[..n]).unwrap_or("").to_string();

        // Reject if Origin header is present and not localhost (CSRF protection)
        for line in head.lines() {
            let lo = line.to_lowercase();
            if lo.starts_with("origin:") {
                let origin = lo.trim_start_matches("origin:").trim();
                if !origin.starts_with("http://localhost")
                    && !origin.starts_with("http://127.0.0.1")
                    && !origin.starts_with("https://localhost")
                {
                    log::warn!("CDP: rejected request from Origin: {}", origin);
                    return Ok(());
                }
            }
        }

        let is_ws = head.lines().any(|l| {
            let lo = l.to_lowercase();
            lo.trim_start().starts_with("upgrade") && lo.contains("websocket")
        });

        if is_ws {
            // accept_async re-reads the same bytes (peek did not consume them)
            let ws = accept_async(stream).await?;
            self.handle_cdp_ws(ws).await;
        } else {
            self.handle_http(stream, &head).await?;
        }
        Ok(())
    }

    async fn handle_http(&self, mut stream: TcpStream, head: &str) -> Result<()> {
        // Consume the full request
        let mut buf = [0u8; 4096];
        let _ = stream.read(&mut buf).await;

        let path = head
            .lines()
            .next()
            .and_then(|l| l.split_whitespace().nth(1))
            .unwrap_or("/");

        let (status, body) = match path {
            "/json/version" | "/json/version/" => ("200 OK", self.json_version_body()),
            "/json" | "/json/" | "/json/list" | "/json/list/" => ("200 OK", self.json_list_body()),
            "/json/new" => ("200 OK", self.json_target_body()),
            _ => ("404 Not Found", r#"{"error":"not found"}"#.to_string()),
        };

        let resp = format!(
            "HTTP/1.1 {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nAccess-Control-Allow-Origin: *\r\nConnection: close\r\n\r\n{}",
            status,
            body.len(),
            body
        );
        let _ = stream.write_all(resp.as_bytes()).await;
        Ok(())
    }

    async fn handle_cdp_ws(&self, ws: tokio_tungstenite::WebSocketStream<TcpStream>) {
        let session = CdpSession::new(Arc::clone(&self.browser), self.target_id.clone());
        let (mut sender, mut receiver) = ws.split();

        while let Some(msg_result) = receiver.next().await {
            match msg_result {
                Ok(Message::Text(text)) => {
                    let (resp, events) = session.handle_message(&text).await;
                    if sender.send(Message::Text(resp)).await.is_err() {
                        break;
                    }
                    for event in events {
                        if sender.send(Message::Text(event)).await.is_err() {
                            break;
                        }
                    }
                }
                Ok(Message::Ping(d)) => {
                    let _ = sender.send(Message::Pong(d)).await;
                }
                Ok(Message::Close(_)) | Err(_) => break,
                _ => {}
            }
        }
    }

    // ── HTTP body helpers ─────────────────────────────────────────────────────

    fn json_version_body(&self) -> String {
        serde_json::json!({
            "Browser":          "Bose/0.2.0",
            "Protocol-Version": "1.3",
            "User-Agent":       "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Bose/0.2.0 Safari/537.36",
            "V8-Version":       "QuickJS",
            "WebKit-Version":   "537.36",
            "webSocketDebuggerUrl": format!("ws://localhost:9222/devtools/browser/{}", self.browser_id)
        }).to_string()
    }

    fn json_list_body(&self) -> String {
        serde_json::json!([self.build_target_info()]).to_string()
    }

    fn json_target_body(&self) -> String {
        serde_json::json!(self.build_target_info()).to_string()
    }

    fn build_target_info(&self) -> serde_json::Value {
        let url = self
            .browser
            .page_state
            .lock()
            .ok()
            .and_then(|g| g.as_ref().map(|s| s.url.clone()))
            .unwrap_or_else(|| "about:blank".to_string());

        serde_json::json!({
            "description": "",
            "devtoolsFrontendUrl": format!(
                "/devtools/inspector.html?ws=localhost:9222/devtools/page/{}", self.target_id
            ),
            "id":    &self.target_id,
            "title": "Bose Browser",
            "type":  "page",
            "url":   url,
            "webSocketDebuggerUrl": format!(
                "ws://localhost:9222/devtools/page/{}", self.target_id
            )
        })
    }
}
