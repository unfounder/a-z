use crate::browser::Browser;
use crate::dom::node::{NodeData, NodeId};
use crate::dom::query::DomQuery;
use crate::dom::serializer::DomSerializer;
use crate::js::runtime::evaluate_expr;
use serde_json::{json, Value};
use std::sync::Arc;

// ── Tool catalogue ────────────────────────────────────────────────────────────

pub fn tool_list() -> Value {
    json!([
        {
            "name": "navigate",
            "description": "Navigate the Bose engine to a URL. Returns the page title, node count, and whether navigation succeeded.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "url": { "type": "string", "description": "Absolute URL to navigate to (http:// or https://)" }
                },
                "required": ["url"]
            }
        },
        {
            "name": "get_content",
            "description": "Return the full serialized HTML of the current page as processed by the Bose engine.",
            "inputSchema": {
                "type": "object",
                "properties": {}
            }
        },
        {
            "name": "get_text",
            "description": "Return the visible text content of the current page (all text nodes joined, no HTML tags).",
            "inputSchema": {
                "type": "object",
                "properties": {}
            }
        },
        {
            "name": "get_title",
            "description": "Return the <title> of the current page.",
            "inputSchema": {
                "type": "object",
                "properties": {}
            }
        },
        {
            "name": "get_links",
            "description": "Return all hyperlinks on the current page as [{href, text}] objects.",
            "inputSchema": {
                "type": "object",
                "properties": {}
            }
        },
        {
            "name": "query_selector",
            "description": "Find the first element matching a CSS selector. Returns its tag, id, class, text content, and inner HTML.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "selector": { "type": "string", "description": "CSS selector, e.g. 'h1', '#main', '.article'" }
                },
                "required": ["selector"]
            }
        },
        {
            "name": "query_selector_all",
            "description": "Find all elements matching a CSS selector. Returns an array of {tag, id, class, text} objects.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "selector": { "type": "string", "description": "CSS selector" },
                    "limit":    { "type": "integer", "description": "Max results (default 50)", "default": 50 }
                },
                "required": ["selector"]
            }
        },
        {
            "name": "evaluate",
            "description": "Evaluate a JavaScript expression in an isolated QuickJS context. Returns the string result.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "expression": { "type": "string", "description": "JavaScript expression to evaluate" }
                },
                "required": ["expression"]
            }
        },
        {
            "name": "get_node_info",
            "description": "Return detailed information about a DOM node by its nodeId (from query_selector).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "node_id": { "type": "integer", "description": "Arena node ID" }
                },
                "required": ["node_id"]
            }
        },
        {
            "name": "page_status",
            "description": "Return current browser state: URL, title, node count, BitFox engine version.",
            "inputSchema": {
                "type": "object",
                "properties": {}
            }
        },
        {
            "name": "get_assets",
            "description": "Return all external asset URLs found on the current page: stylesheets, scripts, images, fonts.",
            "inputSchema": {
                "type": "object",
                "properties": {}
            }
        },
        {
            "name": "reload",
            "description": "Force-reload the current URL, bypassing the navigation cache.",
            "inputSchema": {
                "type": "object",
                "properties": {}
            }
        }
    ])
}

// ── Tool dispatch ─────────────────────────────────────────────────────────────

pub async fn call_tool(browser: &Arc<Browser>, name: &str, args: &Value) -> Value {
    match name {
        "navigate" => tool_navigate(browser, args).await,
        "get_content" => tool_get_content(browser),
        "get_text" => tool_get_text(browser),
        "get_title" => tool_get_title(browser),
        "get_links" => tool_get_links(browser),
        "query_selector" => tool_query_selector(browser, args),
        "query_selector_all" => tool_query_selector_all(browser, args),
        "evaluate" => tool_evaluate(args).await,
        "get_node_info" => tool_get_node_info(browser, args),
        "page_status" => tool_page_status(browser),
        "get_assets" => tool_get_assets(browser),
        "reload" => tool_reload(browser).await,
        _ => tool_error(format!("Unknown tool: {}", name)),
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn text_result(text: impl Into<String>) -> Value {
    json!({ "content": [{ "type": "text", "text": text.into() }] })
}

fn tool_error(msg: impl Into<String>) -> Value {
    json!({ "isError": true, "content": [{ "type": "text", "text": msg.into() }] })
}

fn node_summary(doc: &crate::dom::document::Document, id: NodeId) -> Value {
    let node = match doc.get(id) {
        Some(n) => n,
        None => return json!(null),
    };
    let tag = node.tag_name().unwrap_or("").to_string();
    let id_attr = node.attr("id").unwrap_or("").to_string();
    let class_attr = node.attr("class").unwrap_or("").to_string();
    let text = doc.get_text_content(id);
    let inner = DomSerializer::serialize_children(doc, id).unwrap_or_default();
    json!({
        "nodeId":  id,
        "tag":     tag,
        "id":      id_attr,
        "class":   class_attr,
        "text":    text.chars().take(500).collect::<String>(),
        "innerHTML": inner.chars().take(2000).collect::<String>()
    })
}

// ── Tool implementations ──────────────────────────────────────────────────────

async fn tool_navigate(browser: &Arc<Browser>, args: &Value) -> Value {
    let url = match args.get("url").and_then(|v| v.as_str()) {
        Some(u) => u.to_string(),
        None => return tool_error("Missing required argument: url"),
    };

    match browser.navigate(&url).await {
        Ok(_) => {
            let guard = browser.page_state.lock().unwrap();
            if let Some(state) = guard.as_ref() {
                let title = state.document.get_title();
                let node_count = state.document.nodes.len();
                text_result(format!(
                    "Navigated to: {}\nTitle: {}\nNodes in DOM: {}\nEngine: Bose",
                    state.url, title, node_count
                ))
            } else {
                text_result(format!("Navigated to: {}", url))
            }
        }
        Err(e) => tool_error(format!("Navigation failed: {}", e)),
    }
}

fn tool_get_content(browser: &Arc<Browser>) -> Value {
    let guard = browser.page_state.lock().unwrap();
    match guard.as_ref() {
        Some(state) => match DomSerializer::serialize(&state.document) {
            Ok(html) => text_result(html),
            Err(e) => tool_error(format!("Serialize error: {}", e)),
        },
        None => tool_error("No page loaded. Use navigate first."),
    }
}

fn tool_get_text(browser: &Arc<Browser>) -> Value {
    let guard = browser.page_state.lock().unwrap();
    match guard.as_ref() {
        Some(state) => {
            let text = collect_visible_text(&state.document, 0);
            text_result(text)
        }
        None => tool_error("No page loaded. Use navigate first."),
    }
}

/// Recursively collect text nodes, skipping script/style.
fn collect_visible_text(doc: &crate::dom::document::Document, node_id: NodeId) -> String {
    collect_text_depth(doc, node_id, 0)
}

fn collect_text_depth(
    doc: &crate::dom::document::Document,
    node_id: NodeId,
    depth: usize,
) -> String {
    if depth > 512 {
        return String::new();
    }
    let node = match doc.get(node_id) {
        Some(n) => n,
        None => return String::new(),
    };
    match &node.data {
        NodeData::Text { content } => {
            let t = content.trim().to_string();
            if t.is_empty() {
                String::new()
            } else {
                t + " "
            }
        }
        NodeData::Element { tag_name, .. } if tag_name == "script" || tag_name == "style" => {
            String::new()
        }
        _ => {
            let children = node.children.clone();
            children
                .iter()
                .map(|&c| collect_text_depth(doc, c, depth + 1))
                .collect::<String>()
        }
    }
}

fn tool_get_title(browser: &Arc<Browser>) -> Value {
    let guard = browser.page_state.lock().unwrap();
    match guard.as_ref() {
        Some(state) => text_result(state.document.get_title()),
        None => tool_error("No page loaded."),
    }
}

fn tool_get_links(browser: &Arc<Browser>) -> Value {
    let guard = browser.page_state.lock().unwrap();
    match guard.as_ref() {
        Some(state) => {
            let doc = &state.document;
            let anchors = doc.get_elements_by_tag("a");
            let links: Vec<Value> = anchors
                .iter()
                .map(|&id| {
                    let href = doc
                        .get(id)
                        .and_then(|n| n.attr("href"))
                        .unwrap_or("")
                        .to_string();
                    let text = doc
                        .get_text_content(id)
                        .trim()
                        .chars()
                        .take(200)
                        .collect::<String>();
                    json!({ "href": href, "text": text })
                })
                .collect();
            json!({
                "content": [{
                    "type": "text",
                    "text": serde_json::to_string_pretty(&links).unwrap_or_default()
                }]
            })
        }
        None => tool_error("No page loaded."),
    }
}

fn tool_query_selector(browser: &Arc<Browser>, args: &Value) -> Value {
    let selector = match args.get("selector").and_then(|v| v.as_str()) {
        Some(s) => s.to_string(),
        None => return tool_error("Missing required argument: selector"),
    };
    let guard = browser.page_state.lock().unwrap();
    match guard.as_ref() {
        Some(state) => match DomQuery::query_selector_from(&state.document, 0, &selector) {
            Ok(Some(id)) => {
                let summary = node_summary(&state.document, id);
                json!({
                    "content": [{
                        "type": "text",
                        "text": serde_json::to_string_pretty(&summary).unwrap_or_default()
                    }]
                })
            }
            Ok(None) => tool_error(format!("No element matched '{}'", selector)),
            Err(e) => tool_error(format!("Selector error: {}", e)),
        },
        None => tool_error("No page loaded."),
    }
}

fn tool_query_selector_all(browser: &Arc<Browser>, args: &Value) -> Value {
    let selector = match args.get("selector").and_then(|v| v.as_str()) {
        Some(s) => s.to_string(),
        None => return tool_error("Missing required argument: selector"),
    };
    let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(50) as usize;
    let guard = browser.page_state.lock().unwrap();
    match guard.as_ref() {
        Some(state) => match DomQuery::query_selector_all_from(&state.document, 0, &selector) {
            Ok(ids) => {
                let results: Vec<Value> = ids
                    .iter()
                    .take(limit)
                    .map(|&id| node_summary(&state.document, id))
                    .collect();
                json!({
                    "content": [{
                        "type": "text",
                        "text": format!("Found {} element(s):\n{}", results.len(),
                            serde_json::to_string_pretty(&results).unwrap_or_default())
                    }]
                })
            }
            Err(e) => tool_error(format!("Selector error: {}", e)),
        },
        None => tool_error("No page loaded."),
    }
}

async fn tool_evaluate(args: &Value) -> Value {
    let expr = match args.get("expression").and_then(|v| v.as_str()) {
        Some(e) => e.to_string(),
        None => return tool_error("Missing required argument: expression"),
    };
    let result = evaluate_expr(expr).await;
    text_result(result)
}

fn tool_get_node_info(browser: &Arc<Browser>, args: &Value) -> Value {
    let node_id = match args.get("node_id").and_then(|v| v.as_u64()) {
        Some(n) => n as NodeId,
        None => return tool_error("Missing required argument: node_id"),
    };
    let guard = browser.page_state.lock().unwrap();
    match guard.as_ref() {
        Some(state) => {
            if state.document.get(node_id).is_none() {
                return tool_error(format!("nodeId {} not found", node_id));
            }
            let summary = node_summary(&state.document, node_id);
            json!({
                "content": [{
                    "type": "text",
                    "text": serde_json::to_string_pretty(&summary).unwrap_or_default()
                }]
            })
        }
        None => tool_error("No page loaded."),
    }
}

fn tool_page_status(browser: &Arc<Browser>) -> Value {
    let guard = browser.page_state.lock().unwrap();
    let (url, title, nodes, ts, assets) = match guard.as_ref() {
        Some(state) => (
            state.url.clone(),
            state.document.get_title(),
            state.document.nodes.len(),
            state.timestamp,
            state.assets.total(),
        ),
        None => ("about:blank".to_string(), String::new(), 0, 0.0, 0),
    };
    text_result(format!(
        "URL:        {}\nTitle:      {}\nDOM nodes:  {}\nAssets:     {}\nTimestamp:  {:.3}\nEngine:     Bose/0.4.0\nAllocator:  mimalloc\nJS engine:  QuickJS\nCDP:        ws://127.0.0.1:9222\nMCP:        http://127.0.0.1:9223",
        url, title, nodes, assets, ts
    ))
}

fn tool_get_assets(browser: &Arc<Browser>) -> Value {
    match browser.current_assets() {
        Some(a) => {
            let out = json!({
                "stylesheets": a.stylesheets,
                "scripts":     a.scripts,
                "images":      a.images,
                "fonts":       a.fonts,
                "prefetch":    a.prefetch,
                "total":       a.total()
            });
            json!({
                "content": [{
                    "type": "text",
                    "text": serde_json::to_string_pretty(&out).unwrap_or_default()
                }]
            })
        }
        None => tool_error("No page loaded."),
    }
}

async fn tool_reload(browser: &Arc<Browser>) -> Value {
    let url = browser
        .page_state
        .lock()
        .unwrap()
        .as_ref()
        .map(|s| s.url.clone());
    match url {
        Some(u) => match browser.reload(&u).await {
            Ok(_) => text_result(format!("Reloaded: {}", u)),
            Err(e) => tool_error(format!("Reload failed: {}", e)),
        },
        None => tool_error("No page loaded. Use navigate first."),
    }
}
