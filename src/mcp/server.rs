use super::protocol::{
    JsonRpcRequest, JsonRpcResponse, PROTOCOL_VERSION, SERVER_NAME, SERVER_VERSION,
};
use super::tools;
use crate::browser::Browser;
use anyhow::Result;
use serde_json::json;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

pub struct McpServer {
    browser: Arc<Browser>,
}

impl McpServer {
    pub fn new(browser: Arc<Browser>) -> Self {
        McpServer { browser }
    }

    pub async fn listen(self: Arc<Self>, addr: &str) -> Result<()> {
        let listener = TcpListener::bind(addr).await?;
        log::info!("MCP server listening on http://{}", addr);
        loop {
            let (stream, peer) = listener.accept().await?;
            log::debug!("MCP connection from {}", peer);
            let me = Arc::clone(&self);
            tokio::spawn(async move {
                if let Err(e) = me.handle(stream).await {
                    log::debug!("MCP closed: {}", e);
                }
            });
        }
    }

    async fn handle(&self, mut stream: TcpStream) -> Result<()> {
        // Read headers first (until \r\n\r\n)
        let mut raw_buf: Vec<u8> = Vec::with_capacity(4096);
        let mut tmp = [0u8; 1024];
        let header_end = loop {
            let n = stream.read(&mut tmp).await?;
            if n == 0 {
                break raw_buf.len();
            }
            raw_buf.extend_from_slice(&tmp[..n]);
            if let Some(pos) = find_header_end(&raw_buf) {
                break pos;
            }
        };

        let header_str = std::str::from_utf8(&raw_buf[..header_end]).unwrap_or("");
        let head = header_str;

        // Parse Content-Length to read body
        let content_length: usize = head
            .lines()
            .find(|l| l.to_ascii_lowercase().starts_with("content-length"))
            .and_then(|l| l.split(':').nth(1))
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(0);

        // Body starts after the blank line (header_end + 4 for \r\n\r\n)
        let body_start = header_end + 4;
        let already_read = if raw_buf.len() > body_start {
            &raw_buf[body_start..]
        } else {
            &[]
        };
        let mut body_buf = already_read.to_vec();

        // Read remaining body bytes if needed
        while body_buf.len() < content_length {
            let n = stream.read(&mut tmp).await?;
            if n == 0 {
                break;
            }
            body_buf.extend_from_slice(&tmp[..n]);
        }

        let body = std::str::from_utf8(&body_buf).unwrap_or("");

        let first_line = head.lines().next().unwrap_or("");
        let method_path: Vec<&str> = first_line.split_whitespace().collect();
        let path = method_path.get(1).copied().unwrap_or("/");
        let http_method = method_path.first().copied().unwrap_or("GET");

        // CORS preflight
        if http_method == "OPTIONS" {
            let resp = "HTTP/1.1 204 No Content\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Methods: POST, GET, OPTIONS\r\nAccess-Control-Allow-Headers: Content-Type\r\nConnection: close\r\n\r\n";
            stream.write_all(resp.as_bytes()).await?;
            return Ok(());
        }

        let (status, body_out) = match (http_method, path) {
            ("GET", "/") | ("GET", "/mcp") => {
                // Discovery endpoint — returns server info
                let info = json!({
                    "name":    SERVER_NAME,
                    "version": SERVER_VERSION,
                    "protocol": PROTOCOL_VERSION,
                    "transport": "http",
                    "endpoints": {
                        "rpc":  "POST /rpc",
                        "tools":"GET /tools"
                    }
                });
                ("200 OK", info.to_string())
            }
            ("GET", "/tools") => {
                // Convenience: list tools as plain JSON
                ("200 OK", tools::tool_list().to_string())
            }
            ("POST", "/rpc") | ("POST", "/") | ("POST", "/mcp") => {
                // JSON-RPC 2.0 endpoint
                let resp = self.handle_rpc(body).await;
                ("200 OK", resp)
            }
            _ => {
                let e = json!({"error": "not found", "paths": ["/", "/rpc", "/tools"]});
                ("404 Not Found", e.to_string())
            }
        };

        let http_resp = format!(
            "HTTP/1.1 {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nAccess-Control-Allow-Origin: *\r\nConnection: close\r\n\r\n{}",
            status,
            body_out.len(),
            body_out
        );
        // Ignore broken-pipe / connection-reset — client may have timed out while
        // we were running a slow pipeline (Cloudflare challenges, slow servers, etc.)
        let _ = stream.write_all(http_resp.as_bytes()).await;
        Ok(())
    }

    async fn handle_rpc(&self, body: &str) -> String {
        // Try to parse as array (batch) or single request
        if body.trim_start().starts_with('[') {
            // Batch
            let reqs: Vec<JsonRpcRequest> = match serde_json::from_str(body) {
                Ok(v) => v,
                Err(e) => {
                    let r = JsonRpcResponse::err(None, -32700, format!("Parse error: {}", e));
                    return serde_json::to_string(&[r]).unwrap_or_default();
                }
            };
            let mut responses = Vec::new();
            for req in reqs {
                let r = self.dispatch(req).await;
                responses.push(r);
            }
            serde_json::to_string(&responses).unwrap_or_default()
        } else {
            let req: JsonRpcRequest = match serde_json::from_str(body) {
                Ok(r) => r,
                Err(e) => {
                    let r = JsonRpcResponse::err(None, -32700, format!("Parse error: {}", e));
                    return serde_json::to_string(&r).unwrap_or_default();
                }
            };
            let resp = self.dispatch(req).await;
            serde_json::to_string(&resp).unwrap_or_default()
        }
    }

    async fn dispatch(&self, req: JsonRpcRequest) -> JsonRpcResponse {
        let id = req.id.clone();
        let params = req.params.clone().unwrap_or(json!({}));
        log::debug!("MCP <- {}", req.method);

        match req.method.as_str() {
            // ── MCP lifecycle ─────────────────────────────────────────────────
            "initialize" => JsonRpcResponse::ok(
                id,
                json!({
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": {
                        "tools": { "listChanged": false },
                        "resources": { "subscribe": false, "listChanged": false },
                        "prompts": { "listChanged": false }
                    },
                    "serverInfo": {
                        "name":    SERVER_NAME,
                        "version": SERVER_VERSION
                    }
                }),
            ),
            "initialized" | "notifications/initialized" => {
                // Notification — no response id needed, but return empty ok
                JsonRpcResponse::ok(id, json!({}))
            }
            "ping" => JsonRpcResponse::ok(id, json!({})),

            // ── Tools ─────────────────────────────────────────────────────────
            "tools/list" => JsonRpcResponse::ok(id, json!({ "tools": tools::tool_list() })),
            "tools/call" => {
                let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
                let args = params.get("arguments").cloned().unwrap_or(json!({}));

                if name.is_empty() {
                    return JsonRpcResponse::err(id, -32602, "Missing tool name");
                }

                let result = tools::call_tool(&self.browser, name, &args).await;
                JsonRpcResponse::ok(id, result)
            }

            // ── Resources (minimal — page HTML as a resource) ─────────────────
            "resources/list" => {
                let url = self
                    .browser
                    .page_state
                    .lock()
                    .ok()
                    .and_then(|g| g.as_ref().map(|s| s.url.clone()))
                    .unwrap_or_else(|| "about:blank".to_string());

                JsonRpcResponse::ok(
                    id,
                    json!({
                        "resources": [
                            {
                                "uri":      format!("bose://page/html"),
                                "name":     "Current page HTML",
                                "mimeType": "text/html",
                                "description": format!("Serialized Bose DOM for {}", url)
                            },
                            {
                                "uri":      "bose://page/text",
                                "name":     "Current page text",
                                "mimeType": "text/plain",
                                "description": "Visible text of current page"
                            },
                            {
                                "uri":      "bose://page/links",
                                "name":     "Current page links",
                                "mimeType": "application/json",
                                "description": "All hyperlinks on current page"
                            }
                        ]
                    }),
                )
            }
            "resources/read" => {
                let uri = params.get("uri").and_then(|v| v.as_str()).unwrap_or("");
                let content = match uri {
                    "bose://page/html" => {
                        let r = tools::call_tool(&self.browser, "get_content", &json!({})).await;
                        r["content"][0]["text"].as_str().unwrap_or("").to_string()
                    }
                    "bose://page/text" => {
                        let r = tools::call_tool(&self.browser, "get_text", &json!({})).await;
                        r["content"][0]["text"].as_str().unwrap_or("").to_string()
                    }
                    "bose://page/links" => {
                        let r = tools::call_tool(&self.browser, "get_links", &json!({})).await;
                        r["content"][0]["text"].as_str().unwrap_or("").to_string()
                    }
                    _ => {
                        return JsonRpcResponse::err(
                            id,
                            -32602,
                            format!("Unknown resource: {}", uri),
                        )
                    }
                };
                JsonRpcResponse::ok(
                    id,
                    json!({
                        "contents": [{ "uri": uri, "mimeType": "text/plain", "text": content }]
                    }),
                )
            }

            // ── Prompts (minimal) ─────────────────────────────────────────────
            "prompts/list" => JsonRpcResponse::ok(
                id,
                json!({
                    "prompts": [
                        {
                            "name": "summarize_page",
                            "description": "Summarize the current page content",
                            "arguments": []
                        },
                        {
                            "name": "extract_data",
                            "description": "Extract structured data from the current page",
                            "arguments": [
                                {
                                    "name": "fields",
                                    "description": "Comma-separated field names to extract",
                                    "required": false
                                }
                            ]
                        }
                    ]
                }),
            ),
            "prompts/get" => {
                let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
                let url = self
                    .browser
                    .page_state
                    .lock()
                    .ok()
                    .and_then(|g| g.as_ref().map(|s| s.url.clone()))
                    .unwrap_or_else(|| "about:blank".to_string());

                let messages = match name {
                    "summarize_page" => json!([{
                        "role": "user",
                        "content": {
                            "type": "text",
                            "text": format!(
                                "Please summarize the content of this page: {}\n\nUse the get_text tool to read its content first.",
                                url
                            )
                        }
                    }]),
                    "extract_data" => {
                        let fields = params
                            .get("arguments")
                            .and_then(|a| a.get("fields"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("title, description, links");
                        json!([{
                            "role": "user",
                            "content": {
                                "type": "text",
                                "text": format!(
                                    "Extract the following fields from this page: {}\nURL: {}\n\nUse query_selector and get_text tools to find the data.",
                                    fields, url
                                )
                            }
                        }])
                    }
                    _ => {
                        return JsonRpcResponse::err(
                            id,
                            -32602,
                            format!("Unknown prompt: {}", name),
                        )
                    }
                };

                JsonRpcResponse::ok(id, json!({ "messages": messages }))
            }

            // ── Fallback ──────────────────────────────────────────────────────
            _ => {
                log::debug!("MCP: unhandled '{}'", req.method);
                JsonRpcResponse::err(id, -32601, format!("Method not found: {}", req.method))
            }
        }
    }
}

// ── Helper: find end of HTTP headers (\r\n\r\n) ───────────────────────────────
fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}
