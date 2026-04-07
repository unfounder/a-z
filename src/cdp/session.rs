use super::domains::dom::DomConverter;
use super::protocol::CdpMessage;
use crate::browser::Browser;
use crate::dom::node::{NodeData, NodeId};
use crate::dom::query::DomQuery;
use crate::dom::serializer::DomSerializer;
use crate::browser::DocumentReadyState;
use crate::js::runtime::{evaluate_expr, PageRuntime};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

pub struct CdpSession {
    pub browser: Arc<Browser>,
    pub target_id: String,
    state: Mutex<SessionState>,
}

struct SessionState {
    page_enabled: bool,
    runtime_enabled: bool,
    dom_enabled: bool,
    network_enabled: bool,
}

impl CdpSession {
    pub fn new(browser: Arc<Browser>, target_id: String) -> Self {
        CdpSession {
            browser,
            target_id,
            state: Mutex::new(SessionState {
                page_enabled: false,
                runtime_enabled: false,
                dom_enabled: false,
                network_enabled: false,
            }),
        }
    }

    /// Handle one CDP message. Returns (response_json, Vec<event_json>).
    pub async fn handle_message(&self, message: &str) -> (String, Vec<String>) {
        let msg: CdpMessage = match serde_json::from_str(message) {
            Ok(m) => m,
            Err(e) => {
                let err = json!({
                    "id": 0,
                    "error": { "code": -32700, "message": format!("Parse error: {}", e) }
                });
                return (err.to_string(), vec![]);
            }
        };

        log::debug!("CDP <- {}", msg.method);
        let params = msg.params.clone().unwrap_or(json!({}));
        let (result, events) = self.dispatch(&msg.method, &params).await;

        let resp = json!({ "id": msg.id, "result": result });
        let event_strs = events.into_iter().map(|e| e.to_string()).collect();
        (resp.to_string(), event_strs)
    }

    async fn dispatch(&self, method: &str, params: &Value) -> (Value, Vec<Value>) {
        match method {
            // ── Browser ──────────────────────────────────────────────────────
            "Browser.getVersion" => (self.browser_get_version(), vec![]),
            "Browser.close" | "Browser.getBrowserCommandLine" => (json!({}), vec![]),

            // ── Target ───────────────────────────────────────────────────────
            "Target.getTargets" => (self.target_get_targets(), vec![]),
            "Target.getTargetInfo" => (self.target_get_info(), vec![]),
            "Target.activateTarget" | "Target.setDiscoverTargets" | "Target.setAutoAttach" => {
                (json!({}), vec![])
            }
            "Target.createTarget" => (json!({"targetId": &self.target_id}), vec![]),
            "Target.attachToTarget" => (
                json!({"sessionId": uuid::Uuid::new_v4().to_string()}),
                vec![],
            ),

            // ── Page ─────────────────────────────────────────────────────────
            "Page.enable" => {
                self.state.lock().unwrap().page_enabled = true;
                (json!({}), vec![])
            }
            "Page.disable" => {
                self.state.lock().unwrap().page_enabled = false;
                (json!({}), vec![])
            }
            "Page.navigate" => self.page_navigate(params).await,
            "Page.reload" => self.page_reload().await,
            "Page.getFrameTree" => (self.page_get_frame_tree(), vec![]),
            "Page.getLayoutMetrics" => (self.page_get_layout_metrics(), vec![]),
            "Page.captureScreenshot" | "Page.printToPDF" => (json!({"data": ""}), vec![]),
            "Page.addScriptToEvaluateOnNewDocument" => (json!({"identifier": "1"}), vec![]),
            "Page.stopLoading"
            | "Page.removeScriptToEvaluateOnNewDocument"
            | "Page.setLifecycleEventsEnabled"
            | "Page.setDownloadBehavior" => (json!({}), vec![]),

            // ── Runtime ──────────────────────────────────────────────────────
            "Runtime.enable" => {
                self.state.lock().unwrap().runtime_enabled = true;
                (json!({}), vec![])
            }
            "Runtime.disable" => {
                self.state.lock().unwrap().runtime_enabled = false;
                (json!({}), vec![])
            }
            "Runtime.evaluate" => self.runtime_evaluate(params).await,
            "Runtime.callFunctionOn" => (
                json!({"result": {"type": "undefined", "value": null}}),
                vec![],
            ),
            "Runtime.getProperties" => (json!({"result": [], "internalProperties": []}), vec![]),
            "Runtime.releaseObject"
            | "Runtime.releaseObjectGroup"
            | "Runtime.discardConsoleEntries"
            | "Runtime.runIfWaitingForDebugger" => (json!({}), vec![]),

            // ── DOM ──────────────────────────────────────────────────────────
            "DOM.enable" => {
                self.state.lock().unwrap().dom_enabled = true;
                (json!({}), vec![])
            }
            "DOM.disable" => {
                self.state.lock().unwrap().dom_enabled = false;
                (json!({}), vec![])
            }
            "DOM.getDocument" => self.dom_get_document(params),
            "DOM.getFlattenedDocument" => self.dom_get_flattened(params),
            "DOM.querySelector" => self.dom_query_selector(params),
            "DOM.querySelectorAll" => self.dom_query_selector_all(params),
            "DOM.getAttributes" => self.dom_get_attributes(params),
            "DOM.getOuterHTML" => self.dom_get_outer_html(params),
            "DOM.setAttributeValue" => self.dom_set_attribute_value(params),
            "DOM.removeAttribute" => self.dom_remove_attribute(params),
            "DOM.describeNode" => self.dom_describe_node(params),
            "DOM.focus" | "DOM.setNodeValue" => (json!({}), vec![]),
            "DOM.getBoxModel" => (
                json!({"model": {
                    "content": [0,0,0,0,0,0,0,0],
                    "padding": [0,0,0,0,0,0,0,0],
                    "border":  [0,0,0,0,0,0,0,0],
                    "margin":  [0,0,0,0,0,0,0,0],
                    "width": 0, "height": 0
                }}),
                vec![],
            ),
            "DOM.getContentQuads" => (json!({"quads": [[0,0,0,0,0,0,0,0]]}), vec![]),
            "DOM.requestChildNodes" | "DOM.pushNodesByBackendIdsToFrontend" => (json!({}), vec![]),
            "DOM.resolveNode" => (
                json!({"object": {
                    "type": "object", "className": "HTMLElement", "objectId": "1"
                }}),
                vec![],
            ),

            // ── CSS ──────────────────────────────────────────────────────────
            "CSS.enable" | "CSS.disable" | "CSS.stopRuleUsageTracking" => (json!({}), vec![]),
            "CSS.getComputedStyleForNode" => (json!({"computedStyle": []}), vec![]),
            "CSS.getMatchedStylesForNode" => (
                json!({
                    "matchedCSSRules": [], "inherited": [], "cssKeyframesRules": []
                }),
                vec![],
            ),
            "CSS.getInlineStylesForNode" => (
                json!({
                    "inlineStyle": {"cssText": "", "shorthandEntries": [], "cssProperties": []}
                }),
                vec![],
            ),

            // ── Network ──────────────────────────────────────────────────────
            "Network.enable" => {
                self.state.lock().unwrap().network_enabled = true;
                (json!({}), vec![])
            }
            "Network.disable" => {
                self.state.lock().unwrap().network_enabled = false;
                (json!({}), vec![])
            }
            "Network.setUserAgentOverride"
            | "Network.setExtraHTTPHeaders"
            | "Network.clearBrowserCache"
            | "Network.clearBrowserCookies"
            | "Network.setCacheDisabled"
            | "Network.setRequestInterception" => (json!({}), vec![]),
            "Network.getCookies" | "Network.getAllCookies" => (json!({"cookies": []}), vec![]),

            // ── Emulation ────────────────────────────────────────────────────
            "Emulation.setDeviceMetricsOverride"
            | "Emulation.clearDeviceMetricsOverride"
            | "Emulation.setUserAgentOverride"
            | "Emulation.setDefaultBackgroundColorOverride"
            | "Emulation.setTouchEmulationEnabled"
            | "Emulation.setScriptExecutionDisabled" => (json!({}), vec![]),
            "Emulation.canEmulate" => (json!({"result": true}), vec![]),

            // ── Log ──────────────────────────────────────────────────────────
            "Log.enable"
            | "Log.disable"
            | "Log.clear"
            | "Log.startViolationsReport"
            | "Log.stopViolationsReport" => (json!({}), vec![]),

            // ── Security ─────────────────────────────────────────────────────
            "Security.enable" | "Security.disable" | "Security.setIgnoreCertificateErrors" => {
                (json!({}), vec![])
            }

            // ── Input ────────────────────────────────────────────────────────
            "Input.dispatchMouseEvent"
            | "Input.dispatchKeyEvent"
            | "Input.dispatchTouchEvent"
            | "Input.synthesizeTapGesture"
            | "Input.synthesizeScrollGesture"
            | "Input.setInterceptDrags" => (json!({}), vec![]),

            // ── Profiler / Debugger / HeapProfiler ───────────────────────────
            "Profiler.enable" | "Profiler.disable" | "Profiler.start" | "Profiler.stop" => {
                (json!({}), vec![])
            }
            "Debugger.enable" => (json!({"debuggerId": "bose-debugger"}), vec![]),
            "Debugger.disable"
            | "Debugger.setAsyncCallStackDepth"
            | "Debugger.setBlackboxPatterns" => (json!({}), vec![]),
            "HeapProfiler.enable" | "HeapProfiler.disable" | "HeapProfiler.collectGarbage" => {
                (json!({}), vec![])
            }

            // ── ServiceWorker / Storage / Performance ─────────────────────────
            "ServiceWorker.enable"
            | "ServiceWorker.disable"
            | "Storage.clearDataForOrigin"
            | "Storage.getStorageKeyForFrame" => (json!({}), vec![]),
            "Storage.getCookies" => (json!({"cookies": []}), vec![]),
            "Performance.enable" | "Performance.disable" => (json!({}), vec![]),
            "Performance.getMetrics" => (json!({"metrics": []}), vec![]),

            // ── A-Z custom ───────────────────────────────────────────────────
            "AZ.enable" | "AZ.disable" => (json!({}), vec![]),
            "AZ.getState" => (
                json!({"state": "ready", "engine": "Bose", "version": "0.2.0"}),
                vec![],
            ),

            // ── Fallback ─────────────────────────────────────────────────────
            _ => {
                log::debug!("CDP: unhandled '{}'", method);
                (json!({}), vec![])
            }
        }
    }

    // ── Browser ───────────────────────────────────────────────────────────────

    fn browser_get_version(&self) -> Value {
        json!({
            "protocolVersion": "1.3",
            "product":  "Bose/0.2.0",
            "revision":  "@bose",
            "userAgent": "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Bose/0.2.0 Safari/537.36",
            "jsVersion": "QuickJS"
        })
    }

    // ── Target ────────────────────────────────────────────────────────────────

    fn target_get_targets(&self) -> Value {
        let url = self
            .browser
            .page_state
            .lock()
            .unwrap()
            .as_ref()
            .map(|s| s.url.clone())
            .unwrap_or_else(|| "about:blank".to_string());
        json!({
            "targetInfos": [{
                "targetId":  &self.target_id,
                "type":      "page",
                "title":     "Bose Browser",
                "url":       url,
                "attached":  true,
                "canAccessOpener": false,
                "browserContextId": "default"
            }]
        })
    }

    fn target_get_info(&self) -> Value {
        let url = self
            .browser
            .page_state
            .lock()
            .unwrap()
            .as_ref()
            .map(|s| s.url.clone())
            .unwrap_or_else(|| "about:blank".to_string());
        json!({
            "targetInfo": {
                "targetId":  &self.target_id,
                "type":      "page",
                "title":     "Bose Browser",
                "url":       url,
                "attached":  true,
                "browserContextId": "default"
            }
        })
    }

    // ── Page ─────────────────────────────────────────────────────────────────

    async fn page_navigate(&self, params: &Value) -> (Value, Vec<Value>) {
        let url = params
            .get("url")
            .and_then(|v| v.as_str())
            .unwrap_or("about:blank")
            .to_string();

        let frame_id = self.target_id.clone();
        let loader_id = uuid::Uuid::new_v4().to_string();
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64();

        let security_origin = url::Url::parse(&url)
            .ok()
            .map(|u| format!("{}://{}", u.scheme(), u.host_str().unwrap_or("")))
            .unwrap_or_default();

        match self.browser.navigate(&url).await {
            Ok(_) => {
                let events = vec![
                    json!({
                        "method": "Page.frameNavigated",
                        "params": {
                            "frame": {
                                "id":             &frame_id,
                                "loaderId":       &loader_id,
                                "url":            &url,
                                "mimeType":       "text/html",
                                "securityOrigin": security_origin
                            },
                            "type": "Navigation"
                        }
                    }),
                    json!({ "method": "Page.domContentEventFired", "params": {"timestamp": ts} }),
                    json!({ "method": "Page.loadEventFired",        "params": {"timestamp": ts} }),
                ];
                (json!({"frameId": frame_id, "loaderId": loader_id}), events)
            }
            Err(e) => (
                json!({"frameId": frame_id, "loaderId": loader_id, "errorText": e.to_string()}),
                vec![],
            ),
        }
    }

    async fn page_reload(&self) -> (Value, Vec<Value>) {
        let url = self
            .browser
            .page_state
            .lock()
            .unwrap()
            .as_ref()
            .map(|s| s.url.clone());
        if let Some(url) = url {
            self.page_navigate(&json!({"url": url})).await
        } else {
            (json!({}), vec![])
        }
    }

    fn page_get_frame_tree(&self) -> Value {
        let url = self
            .browser
            .page_state
            .lock()
            .unwrap()
            .as_ref()
            .map(|s| s.url.clone())
            .unwrap_or_else(|| "about:blank".to_string());
        json!({
            "frameTree": {
                "frame": {
                    "id": &self.target_id, "loaderId": "loader-main",
                    "url": url, "mimeType": "text/html", "securityOrigin": "null"
                },
                "childFrames": []
            }
        })
    }

    fn page_get_layout_metrics(&self) -> Value {
        json!({
            "layoutViewport": {"pageX":0,"pageY":0,"clientWidth":1280,"clientHeight":800},
            "visualViewport":  {"offsetX":0,"offsetY":0,"pageX":0,"pageY":0,"clientWidth":1280,"clientHeight":800,"scale":1.0},
            "contentSize":     {"x":0,"y":0,"width":1280,"height":800}
        })
    }

    // ── Runtime ───────────────────────────────────────────────────────────────

    async fn runtime_evaluate(&self, params: &Value) -> (Value, Vec<Value>) {
        let expr = params
            .get("expression")
            .and_then(|v| v.as_str())
            .unwrap_or("undefined")
            .to_string();

        // Prefer the persistent per-page runtime — it has real DOM bindings so
        // document.title, document.body.innerText, etc. return actual page values.
        // Fall back to the isolated evaluate_expr only if no runtime is available.
        let page_runtime: Option<PageRuntime> = self.browser
            .page_state
            .lock()
            .ok()
            .and_then(|s| s.as_ref().and_then(|s| s.page_runtime.clone()));

        let result = if let Some(runtime) = page_runtime {
            tokio::task::spawn_blocking(move || runtime.evaluate_sync(expr))
                .await
                .unwrap_or_else(|_| "Error: thread panic".to_string())
        } else {
            evaluate_expr(expr).await
        };

        (
            json!({"result": {"type": "string", "value": result}}),
            vec![],
        )
    }

    // ── DOM helpers ───────────────────────────────────────────────────────────

    fn with_doc<F, T>(&self, f: F) -> Option<T>
    where
        F: FnOnce(&crate::dom::document::Document) -> T,
    {
        self.browser
            .page_state
            .lock()
            .ok()?
            .as_ref()
            .filter(|s| s.ready_state == DocumentReadyState::Complete)
            .map(|s| f(&s.document))
    }

    fn dom_get_document(&self, params: &Value) -> (Value, Vec<Value>) {
        let depth = params.get("depth").and_then(|v| v.as_i64()).unwrap_or(2) as i32;
        let root = self
            .with_doc(|doc| DomConverter::node_to_cdp(doc, 0, depth))
            .unwrap_or_else(|| {
                json!({
                    "nodeId":0,"backendNodeId":0,"nodeType":9,
                    "nodeName":"#document","localName":"","nodeValue":"","childNodeCount":0
                })
            });
        (json!({"root": root}), vec![])
    }

    fn dom_get_flattened(&self, _params: &Value) -> (Value, Vec<Value>) {
        let nodes: Vec<Value> = self
            .with_doc(|doc| {
                (0..doc.nodes.len())
                    .map(|id| DomConverter::node_to_cdp(doc, id, 0))
                    .filter(|v| !v.is_null())
                    .collect()
            })
            .unwrap_or_default();
        (json!({"nodes": nodes}), vec![])
    }

    fn dom_query_selector(&self, params: &Value) -> (Value, Vec<Value>) {
        let node_id = params.get("nodeId").and_then(|v| v.as_u64()).unwrap_or(0) as NodeId;
        let selector = params
            .get("selector")
            .and_then(|v| v.as_str())
            .unwrap_or("*");
        let found = self
            .with_doc(|doc| {
                DomQuery::query_selector_from(doc, node_id, selector)
                    .unwrap_or(None)
                    .unwrap_or(0)
            })
            .unwrap_or(0);
        (json!({"nodeId": found}), vec![])
    }

    fn dom_query_selector_all(&self, params: &Value) -> (Value, Vec<Value>) {
        let node_id = params.get("nodeId").and_then(|v| v.as_u64()).unwrap_or(0) as NodeId;
        let selector = params
            .get("selector")
            .and_then(|v| v.as_str())
            .unwrap_or("*");
        let found: Vec<usize> = self
            .with_doc(|doc| {
                DomQuery::query_selector_all_from(doc, node_id, selector).unwrap_or_default()
            })
            .unwrap_or_default();
        (json!({"nodeIds": found}), vec![])
    }

    fn dom_get_attributes(&self, params: &Value) -> (Value, Vec<Value>) {
        let node_id = params.get("nodeId").and_then(|v| v.as_u64()).unwrap_or(0) as NodeId;
        let attrs: Vec<String> = self
            .with_doc(|doc| {
                doc.get(node_id)
                    .and_then(|n| {
                        if let NodeData::Element { attrs, .. } = &n.data {
                            Some(
                                attrs
                                    .iter()
                                    .flat_map(|a| vec![a.name.clone(), a.value.clone()])
                                    .collect::<Vec<_>>(),
                            )
                        } else {
                            None
                        }
                    })
                    .unwrap_or_default()
            })
            .unwrap_or_default();
        (json!({"attributes": attrs}), vec![])
    }

    fn dom_get_outer_html(&self, params: &Value) -> (Value, Vec<Value>) {
        let node_id = params.get("nodeId").and_then(|v| v.as_u64()).unwrap_or(0) as NodeId;
        let html = self
            .with_doc(|doc| DomSerializer::serialize_node(doc, node_id).unwrap_or_default())
            .unwrap_or_default();
        (json!({"outerHTML": html}), vec![])
    }

    fn dom_set_attribute_value(&self, params: &Value) -> (Value, Vec<Value>) {
        let node_id = params.get("nodeId").and_then(|v| v.as_u64()).unwrap_or(0) as NodeId;
        let name = params
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let value = params
            .get("value")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if let Ok(mut g) = self.browser.page_state.lock() {
            if let Some(state) = g.as_mut() {
                state.document.set_attribute(node_id, &name, &value);
            }
        }
        (json!({}), vec![])
    }

    fn dom_remove_attribute(&self, params: &Value) -> (Value, Vec<Value>) {
        let node_id = params.get("nodeId").and_then(|v| v.as_u64()).unwrap_or(0) as NodeId;
        let name = params
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if let Ok(mut g) = self.browser.page_state.lock() {
            if let Some(state) = g.as_mut() {
                state.document.remove_attribute(node_id, &name);
            }
        }
        (json!({}), vec![])
    }

    fn dom_describe_node(&self, params: &Value) -> (Value, Vec<Value>) {
        let node_id = params.get("nodeId").and_then(|v| v.as_u64()).unwrap_or(0) as NodeId;
        let depth = params.get("depth").and_then(|v| v.as_i64()).unwrap_or(1) as i32;
        let node = self
            .with_doc(|doc| DomConverter::node_to_cdp(doc, node_id, depth))
            .unwrap_or(json!(null));
        (json!({"node": node}), vec![])
    }
}
