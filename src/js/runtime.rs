use crate::dom::document::Document;
use crate::dom::node::NodeId;
use crate::dom::parser::HtmlParser;
use crate::dom::query::DomQuery;
use crate::dom::serializer::DomSerializer;
use crate::error::{BrowserError, BrowserResult};
use rquickjs::{Context, Function, Object, Runtime, Value};
use std::sync::{Arc, Mutex};

use crate::network::{NetworkController, ConnectivityMode};

// ── Event-loop types ──────────────────────────────────────────────────────────

/// A DOM input event sent from the UI thread to the live JS context.
#[derive(Debug)]
pub enum DomEvent {
    Click    { x: f32, y: f32 },
    KeyDown  { key: String },
    /// Evaluate a JS expression in the live page context and return the result.
    Evaluate { expression: String, result_tx: std::sync::mpsc::SyncSender<String> },
}

/// A widget queued by a JS plugin via `vbos.ui.*`.
#[derive(Debug, Clone)]
pub enum UiWidget {
    Label     { text: String },
    Button    { text: String },
    Separator,
}

/// A live per-tab JS context reachable from the UI.
#[derive(Clone)]
pub struct PageRuntime {
    /// Send commands to the background JS thread.
    pub event_tx:  std::sync::mpsc::SyncSender<DomEvent>,
    /// Widgets queued by `vbos.ui.*` calls; read and cleared each frame.
    pub ui_widgets: Arc<Mutex<Vec<UiWidget>>>,
}

impl PageRuntime {
    /// Evaluate a JS expression in the persistent page context (has real DOM bindings).
    /// Blocks the calling thread until the JS thread returns a result.
    pub fn evaluate_sync(&self, expression: String) -> String {
        let (tx, rx) = std::sync::mpsc::sync_channel::<String>(1);
        if self.event_tx.send(DomEvent::Evaluate { expression, result_tx: tx }).is_err() {
            return "Error: JS context is closed".to_string();
        }
        rx.recv().unwrap_or_else(|_| "Error: no response from JS thread".to_string())
    }
}

/// Execute page scripts with real DOM access.
pub async fn execute_with_dom(
    scripts: Vec<String>,
    doc: Document,
    url: String,
    network: Arc<NetworkController>,
) -> BrowserResult<Document> {
    let dom = Arc::new(Mutex::new(doc));
    let dom_thread = Arc::clone(&dom);
    let net_thread = Arc::clone(&network);

    tokio::task::spawn_blocking(move || -> BrowserResult<()> {
        let rt = Runtime::new().map_err(|e| BrowserError::JavaScript(e.to_string()))?;
        let ctx = Context::full(&rt).map_err(|e| BrowserError::JavaScript(e.to_string()))?;

        // Gas limit: interrupt any script that runs longer than 8 seconds.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
        rt.set_interrupt_handler(Some(Box::new(move || {
            std::time::Instant::now() > deadline
        })));

        ctx.with(|ctx| {
            // 1. Install __bose_dom__ bindings (DOM + real HTTP fetch)
            install_bose_dom(&ctx, Arc::clone(&dom_thread))
                .map_err(|e| BrowserError::JavaScript(e.to_string()))?;

            // 1b. Install vbos bindings (Network control)
            install_vbos(&ctx, Arc::clone(&net_thread))
                .map_err(|e| BrowserError::JavaScript(e.to_string()))?;

            // 2. Web API stubs (console, setTimeout, fetch polyfill using __bose_dom__.httpGet, etc.)
            let _ = ctx.eval::<Value, _>(WEB_API_STUBS);

            // 3. DOM shim — wraps __bose_dom__ into standard document/Element API
            let _ = ctx.eval::<Value, _>(DOM_SHIM_JS);

            // 4. Populate location from real URL (host, pathname, search, etc.)
            let safe_url = url.replace('\'', "\\'");
            let loc = format!(r#"
(function(){{
  try{{
    var __u=new URL('{url}');
    globalThis.location.href=__u.href;
    globalThis.location.protocol=__u.protocol;
    globalThis.location.host=__u.host;
    globalThis.location.hostname=__u.hostname;
    globalThis.location.port=__u.port||'';
    globalThis.location.pathname=__u.pathname;
    globalThis.location.search=__u.search;
    globalThis.location.hash=__u.hash;
    globalThis.location.origin=__u.origin;
    globalThis.document.domain=__u.hostname;
    globalThis.origin=__u.origin;
  }}catch(e){{}}
}})();
"#, url = safe_url);
            let _ = ctx.eval::<Value, _>(loc.as_str());

            // 5. Execute page scripts — best effort, ignore individual errors
            for script in &scripts {
                let _ = ctx.eval::<Value, _>(script.as_str());
            }

            // 5b. Drain deferred setTimeout(fn, 0) callbacks — multiple passes (max 3).
            //     Captures common deferred-init patterns without running recursive loops.
            for _ in 0..3 {
                let _ = ctx.eval::<Value, _>(
                    r#"
    (function(){
      var cbs=globalThis.__bose_deferred__||[];
      globalThis.__bose_deferred__=[];
      for(var i=0;i<cbs.length&&i<100;i++){try{cbs[i]();}catch(e){}}
    })();
    "#,
                );
            }

            Ok::<(), BrowserError>(())
        })?;

        // 6. Pump microtask queue — capped at 2000 iterations.
        //    Prevents infinite Promise loops from stalling the pipeline.
        for _ in 0..2000 {
            match rt.execute_pending_job() {
                Ok(true) => {}
                Ok(false) | Err(_) => break,
            }
        }

        Ok(())
    })
    .await
    .map_err(|e| BrowserError::JavaScript(format!("spawn_blocking: {}", e)))??;

    Arc::try_unwrap(dom)
        .map_err(|_| BrowserError::JavaScript("DOM Arc still shared after JS".to_string()))
        .map(|m| m.into_inner().unwrap())
}

/// Spawn a persistent JS context for a loaded page.
///
/// Runs `scripts` once, then keeps the context alive in a background thread
/// so that subsequent UI events (clicks, keypresses) can be dispatched into it.
/// Returns the post-script Document AND a `PageRuntime` the UI can use to
/// send events and read plugin-queued widgets.
pub async fn spawn_page_runtime(
    scripts:  Vec<String>,
    doc:      Document,
    url:      String,
    network:  Arc<NetworkController>,
) -> BrowserResult<(Document, PageRuntime)> {
    let dom        = Arc::new(Mutex::new(doc));
    let dom_thread = Arc::clone(&dom);
    let net_thread = Arc::clone(&network);
    let ui_widgets: Arc<Mutex<Vec<UiWidget>>> = Arc::new(Mutex::new(Vec::new()));
    let ui_thread  = Arc::clone(&ui_widgets);

    let (event_tx, event_rx) = std::sync::mpsc::sync_channel::<DomEvent>(64);
    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<BrowserResult<()>>();

    std::thread::spawn(move || {
        let rt = match Runtime::new() {
            Ok(r)  => r,
            Err(e) => { ready_tx.send(Err(BrowserError::JavaScript(e.to_string()))).ok(); return; }
        };
        let ctx = match Context::full(&rt) {
            Ok(c)  => c,
            Err(e) => { ready_tx.send(Err(BrowserError::JavaScript(e.to_string()))).ok(); return; }
        };

        // Gas limit: 8s per script during initial load
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
        rt.set_interrupt_handler(Some(Box::new(move || std::time::Instant::now() > deadline)));

        // Setup stubs + DOM bindings
        let result = ctx.with(|ctx| {
            install_bose_dom(&ctx, Arc::clone(&dom_thread))
                .map_err(|e| BrowserError::JavaScript(e.to_string()))?;
            install_vbos_with_ui(&ctx, Arc::clone(&net_thread), Arc::clone(&ui_thread))
                .map_err(|e| BrowserError::JavaScript(e.to_string()))?;
            let _ = ctx.eval::<Value, _>(WEB_API_STUBS);
            let _ = ctx.eval::<Value, _>(DOM_SHIM_JS);

            let safe_url = url.replace('\'', "\\'");
            let loc = format!(r#"(function(){{try{{var __u=new URL('{url}');globalThis.location.href=__u.href;globalThis.location.protocol=__u.protocol;globalThis.location.host=__u.host;globalThis.location.hostname=__u.hostname;globalThis.location.port=__u.port||'';globalThis.location.pathname=__u.pathname;globalThis.location.search=__u.search;globalThis.location.hash=__u.hash;globalThis.location.origin=__u.origin;globalThis.document.domain=__u.hostname;globalThis.origin=__u.origin;}}catch(e){{}}}})()"#, url = safe_url);
            let _ = ctx.eval::<Value, _>(loc.as_str());

            for script in &scripts {
                let _ = ctx.eval::<Value, _>(script.as_str());
            }
            // Drain deferred callbacks
            for _ in 0..3 {
                let _ = ctx.eval::<Value, _>(r#"(function(){var cbs=globalThis.__bose_deferred__||[];globalThis.__bose_deferred__=[];for(var i=0;i<cbs.length&&i<100;i++){try{cbs[i]();}catch(e){}}})();"#);
            }
            Ok::<(), BrowserError>(())
        });

        ready_tx.send(result).ok();

        // Event loop — stays alive until BrowserTab is dropped (sender closes)
        for event in &event_rx {
            // Reset gas limit for each event handler
            let ev_deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            rt.set_interrupt_handler(Some(Box::new(move || std::time::Instant::now() > ev_deadline)));

            // Wrap the entire ctx.with + microtask pump in catch_unwind.
            // A Rust panic inside a __bose_dom__ FFI callback (or in execute_pending_job)
            // would otherwise unwind through C frames and kill the event-loop thread.
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                ctx.with(|ctx| {
                    match &event {
                        DomEvent::Click { x, y } => {
                            let js = format!(
                                "(function(){{try{{var el=document.elementFromPoint({x},{y});if(el){{var e=new MouseEvent('click',{{bubbles:true,cancelable:true,clientX:{x},clientY:{y}}});el.dispatchEvent(e);}}}}catch(e){{}}}})()",
                                x = x, y = y
                            );
                            let _ = ctx.eval::<Value, _>(js.as_str());
                        }
                        DomEvent::KeyDown { key } => {
                            let safe_key = key.replace('\'', "\\'");
                            let js = format!(
                                "try{{document.dispatchEvent(new KeyboardEvent('keydown',{{key:'{}',bubbles:true}}));}}catch(e){{}}",
                                safe_key
                            );
                            let _ = ctx.eval::<Value, _>(js.as_str());
                        }
                        DomEvent::Evaluate { expression, result_tx } => {
                            // Run in the persistent context where DOM_SHIM_JS and real
                            // DOM bindings are already installed — document.title etc. work.
                            // The inner JS try/catch turns TypeError/ReferenceError into a
                            // clean error string rather than a Rust Err propagation.
                            let code = format!(
                                "(function(){{try{{var __r=(function(){{return({expr});}}());if(__r===null)return 'null';if(__r===undefined)return 'undefined';if(typeof __r==='string')return __r;return String(__r);}}catch(__e){{return 'Error: '+String(__e);}}}})()",
                                expr = expression
                            );
                            let result = match ctx.eval::<Value, _>(code.as_str()) {
                                Ok(v) => {
                                    if let Some(js_s) = v.as_string() {
                                        js_s.to_string().unwrap_or_else(|_| "[error]".to_string())
                                    } else {
                                        "undefined".to_string()
                                    }
                                }
                                Err(e) => format!("Error: {}", e),
                            };
                            let _ = result_tx.send(result);
                        }
                    }
                    // Pump microtasks — each job is individually guarded so a panicking
                    // job doesn't propagate past the outer catch_unwind.
                    for _ in 0..200 {
                        let job = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            rt.execute_pending_job()
                        }));
                        match job {
                            Ok(Ok(true))  => {}
                            Ok(Ok(false)) | Ok(Err(_)) | Err(_) => break,
                        }
                    }
                });
            }));
        }
    });

    // Wait for initial script execution to complete
    ready_rx.recv()
        .map_err(|_| BrowserError::JavaScript("JS event-loop thread died on startup".to_string()))??;

    // Read the post-script document snapshot
    let post_doc = dom.lock().unwrap().clone();
    Ok((post_doc, PageRuntime { event_tx, ui_widgets }))
}

/// Execute scripts with no DOM (fallback, used when no Document is available)
pub async fn execute_page_scripts(scripts: Vec<String>, url: String) -> BrowserResult<()> {
    tokio::task::spawn_blocking(move || -> BrowserResult<()> {
        let rt = Runtime::new().map_err(|e| BrowserError::JavaScript(e.to_string()))?;
        let ctx = Context::full(&rt).map_err(|e| BrowserError::JavaScript(e.to_string()))?;

        ctx.with(|ctx| {
            let _ = ctx.eval::<Value, _>(WEB_API_STUBS);
            let loc = format!(
                "try{{globalThis.location.href='{}'}}catch(e){{}};",
                url.replace('\'', "\\'")
            );
            let _ = ctx.eval::<Value, _>(loc.as_str());
            for script in &scripts {
                let _ = ctx.eval::<Value, _>(script.as_str());
            }
        });

        Ok(())
    })
    .await
    .map_err(|e| BrowserError::JavaScript(format!("spawn_blocking: {}", e)))?
}

// ── DOM Bindings ──────────────────────────────────────────────────────────────

fn install_bose_dom(ctx: &rquickjs::Ctx<'_>, dom: Arc<Mutex<Document>>) -> rquickjs::Result<()> {
    let bose = Object::new(ctx.clone())?;

    macro_rules! bind {
        ($name:literal, $closure:expr) => {
            bose.set($name, Function::new(ctx.clone(), $closure)?)?;
        };
    }

    // ── Document structure ────────────────────────────────────────────────

    {
        let d = Arc::clone(&dom);
        bind!("getBody", move || -> i32 {
            d.lock()
                .unwrap()
                .find_element("body")
                .map(|id| id as i32)
                .unwrap_or(-1)
        });
    }

    {
        let d = Arc::clone(&dom);
        bind!("getHead", move || -> i32 {
            d.lock()
                .unwrap()
                .find_element("head")
                .map(|id| id as i32)
                .unwrap_or(-1)
        });
    }

    {
        let d = Arc::clone(&dom);
        bind!("getDocumentElement", move || -> i32 {
            d.lock()
                .unwrap()
                .find_element("html")
                .map(|id| id as i32)
                .unwrap_or(-1)
        });
    }

    {
        let d = Arc::clone(&dom);
        bind!("getTitle", move || -> String {
            d.lock().unwrap().get_title()
        });
    }

    {
        let d = Arc::clone(&dom);
        bind!("setTitle", move |title: String| {
            d.lock().unwrap().set_title(&title);
        });
    }

    // ── Query ─────────────────────────────────────────────────────────────

    {
        let d = Arc::clone(&dom);
        bind!("getElementById", move |id: String| -> i32 {
            d.lock()
                .unwrap()
                .get_element_by_id(&id)
                .map(|n| n as i32)
                .unwrap_or(-1)
        });
    }

    {
        let d = Arc::clone(&dom);
        bind!("querySelector", move |parent_id: u32, sel: String| -> i32 {
            let doc = d.lock().unwrap();
            DomQuery::query_selector_from(&doc, parent_id as NodeId, &sel)
                .unwrap_or(None)
                .map(|id| id as i32)
                .unwrap_or(-1)
        });
    }

    {
        let d = Arc::clone(&dom);
        bind!("querySelectorAll", move |parent_id: u32,
                                        sel: String|
              -> Vec<u32> {
            let doc = d.lock().unwrap();
            DomQuery::query_selector_all_from(&doc, parent_id as NodeId, &sel)
                .unwrap_or_default()
                .into_iter()
                .map(|id| id as u32)
                .collect()
        });
    }

    {
        let d = Arc::clone(&dom);
        bind!("getElementsByTag", move |parent_id: u32,
                                        tag: String|
              -> Vec<u32> {
            let doc = d.lock().unwrap();
            if parent_id == 0 {
                doc.get_elements_by_tag(&tag)
            } else {
                let mut result = Vec::new();
                let mut stack: Vec<NodeId> = doc
                    .get(parent_id as NodeId)
                    .map(|n| n.children.clone())
                    .unwrap_or_default();
                while let Some(id) = stack.pop() {
                    if let Some(node) = doc.get(id) {
                        if node.tag_name() == Some(tag.as_str()) {
                            result.push(id);
                        }
                        stack.extend(node.children.iter().copied());
                    }
                }
                result
            }
            .into_iter()
            .map(|id| id as u32)
            .collect()
        });
    }

    // ── Attributes ────────────────────────────────────────────────────────

    {
        let d = Arc::clone(&dom);
        bind!("getAttribute", move |node_id: u32,
                                    name: String|
              -> String {
            d.lock()
                .unwrap()
                .get_attribute(node_id as NodeId, &name)
                .unwrap_or("")
                .to_string()
        });
    }

    {
        let d = Arc::clone(&dom);
        bind!(
            "setAttribute",
            move |node_id: u32, name: String, value: String| {
                d.lock()
                    .unwrap()
                    .set_attribute(node_id as NodeId, &name, &value);
            }
        );
    }

    {
        let d = Arc::clone(&dom);
        bind!("removeAttribute", move |node_id: u32, name: String| {
            d.lock().unwrap().remove_attribute(node_id as NodeId, &name);
        });
    }

    // ── Content ───────────────────────────────────────────────────────────

    {
        let d = Arc::clone(&dom);
        bind!("getTextContent", move |node_id: u32| -> String {
            d.lock().unwrap().get_text_content(node_id as NodeId)
        });
    }

    {
        let d = Arc::clone(&dom);
        bind!("setTextContent", move |node_id: u32, text: String| {
            d.lock().unwrap().set_text_content(node_id as NodeId, &text);
        });
    }

    {
        let d = Arc::clone(&dom);
        bind!("getInnerHTML", move |node_id: u32| -> String {
            let doc = d.lock().unwrap();
            DomSerializer::serialize_children(&doc, node_id as NodeId).unwrap_or_default()
        });
    }

    {
        let d = Arc::clone(&dom);
        bind!("setInnerHTML", move |node_id: u32, html: String| {
            let frag_html = format!("<html><body>{}</body></html>", html);
            let parser = HtmlParser::new();
            if let Ok(frag_doc) = parser.parse(&frag_html, "") {
                let src_children: Vec<NodeId> = frag_doc
                    .find_element("body")
                    .and_then(|b| frag_doc.get(b))
                    .map(|n| n.children.clone())
                    .unwrap_or_default();
                let mut doc = d.lock().unwrap();
                doc.replace_inner_html(node_id as NodeId, &frag_doc, &src_children);
            }
        });
    }

    {
        let d = Arc::clone(&dom);
        bind!("getTagName", move |node_id: u32| -> String {
            d.lock()
                .unwrap()
                .get_tag_name(node_id as NodeId)
                .to_string()
        });
    }

    // ── Tree navigation ───────────────────────────────────────────────────

    {
        let d = Arc::clone(&dom);
        bind!("getParent", move |node_id: u32| -> i32 {
            d.lock()
                .unwrap()
                .get(node_id as NodeId)
                .and_then(|n| n.parent)
                .map(|p| p as i32)
                .unwrap_or(-1)
        });
    }

    {
        let d = Arc::clone(&dom);
        bind!("getChildren", move |node_id: u32| -> Vec<u32> {
            d.lock()
                .unwrap()
                .get(node_id as NodeId)
                .map(|n| n.children.iter().map(|&c| c as u32).collect())
                .unwrap_or_default()
        });
    }

    {
        let d = Arc::clone(&dom);
        bind!("getNextSibling", move |node_id: u32| -> i32 {
            d.lock()
                .unwrap()
                .get_next_sibling(node_id as NodeId)
                .map(|id| id as i32)
                .unwrap_or(-1)
        });
    }

    {
        let d = Arc::clone(&dom);
        bind!("getPrevSibling", move |node_id: u32| -> i32 {
            d.lock()
                .unwrap()
                .get_prev_sibling(node_id as NodeId)
                .map(|id| id as i32)
                .unwrap_or(-1)
        });
    }

    // ── Mutation ──────────────────────────────────────────────────────────

    {
        let d = Arc::clone(&dom);
        bind!("createElement", move |tag: String| -> u32 {
            d.lock().unwrap().create_element_node(&tag) as u32
        });
    }

    {
        let d = Arc::clone(&dom);
        bind!("createTextNode", move |text: String| -> u32 {
            d.lock().unwrap().create_text_node(&text) as u32
        });
    }

    {
        let d = Arc::clone(&dom);
        bind!("appendChild", move |parent_id: u32, child_id: u32| {
            d.lock()
                .unwrap()
                .append_child(parent_id as NodeId, child_id as NodeId);
        });
    }

    {
        let d = Arc::clone(&dom);
        bind!("removeNode", move |node_id: u32| {
            d.lock().unwrap().remove_from_parent(node_id as NodeId);
        });
    }

    // ── Events (record only — callbacks run post-execution) ───────────────
    bind!(
        "addEventListener",
        move |_node_id: u32, _event_type: String| {
            // Recorded for future EventTarget phase
        }
    );

    // ── Real HTTP (used by fetch/XHR polyfills) ───────────────────────────
    {
        let client = reqwest::blocking::Client::builder()
            .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
            .timeout(std::time::Duration::from_secs(8))
            .cookie_store(true)
            .gzip(true)
            .build()
            .unwrap_or_else(|_| reqwest::blocking::Client::new());
        let client = Arc::new(client);

        let c = Arc::clone(&client);
        bose.set(
            "httpGet",
            Function::new(ctx.clone(), move |url: String| -> String {
                c.get(&url)
                    .header("Accept", "application/json, text/html, */*;q=0.8")
                    .header("Cookie", "SOCS=CAE=; CONSENT=YES+cb")
                    .send()
                    .and_then(|r| r.text())
                    .unwrap_or_default()
            })?,
        )?;

        let c = Arc::clone(&client);
        bose.set(
            "httpPost",
            Function::new(
                ctx.clone(),
                move |url: String, body: String, ct: String| -> String {
                    let ct = if ct.is_empty() {
                        "application/json".to_string()
                    } else {
                        ct
                    };
                    c.post(&url)
                        .header("Content-Type", ct)
                        .header("Accept", "application/json, */*")
                        .body(body)
                        .send()
                        .and_then(|r| r.text())
                        .unwrap_or_default()
                },
            )?,
        )?;
    }

    ctx.globals().set("__bose_dom__", bose)?;
    Ok(())
}

fn install_vbos(ctx: &rquickjs::Ctx<'_>, network: Arc<NetworkController>) -> rquickjs::Result<()> {
    install_vbos_with_ui(ctx, network, Arc::new(Mutex::new(Vec::new())))
}

fn install_vbos_with_ui(
    ctx:        &rquickjs::Ctx<'_>,
    network:    Arc<NetworkController>,
    ui_widgets: Arc<Mutex<Vec<UiWidget>>>,
) -> rquickjs::Result<()> {
    let vbos = Object::new(ctx.clone())?;

    // ── Network control ───────────────────────────────────────────────────
    {
        let n = Arc::clone(&network);
        vbos.set("setNetworkMode", Function::new(ctx.clone(), move |mode: String| {
            let m = match mode.to_lowercase().as_str() {
                "tor"    => ConnectivityMode::Tor,
                "direct" => ConnectivityMode::Direct,
                _        => return,
            };
            n.set_mode(m);
        })?)?;
    }

    vbos.set("onBeforeRequest", Function::new(ctx.clone(), |_callback: Value| {})?)?;

    // ── vbos.ui — plugin widget injection ─────────────────────────────────
    // Plugins can call vbos.ui.label("Threat: High") to draw persistent UI
    // in the browser chrome below the rendered page.
    let ui_obj = Object::new(ctx.clone())?;

    {
        let w = Arc::clone(&ui_widgets);
        ui_obj.set("label", Function::new(ctx.clone(), move |text: String| {
            w.lock().unwrap().push(UiWidget::Label { text });
        })?)?;
    }
    {
        let w = Arc::clone(&ui_widgets);
        ui_obj.set("button", Function::new(ctx.clone(), move |text: String| {
            w.lock().unwrap().push(UiWidget::Button { text });
        })?)?;
    }
    {
        let w = Arc::clone(&ui_widgets);
        ui_obj.set("separator", Function::new(ctx.clone(), move || {
            w.lock().unwrap().push(UiWidget::Separator);
        })?)?;
    }
    {
        let w = Arc::clone(&ui_widgets);
        ui_obj.set("clear", Function::new(ctx.clone(), move || {
            w.lock().unwrap().clear();
        })?)?;
    }

    vbos.set("ui", ui_obj)?;
    ctx.globals().set("vbos", vbos)?;
    Ok(())
}

// ── Stubs and shim ────────────────────────────────────────────────────────────

const WEB_API_STUBS: &str = r#"
(function(){
var noop=function(){},nooparr=function(){return[]},noopnull=function(){return null};
globalThis.console={log:noop,warn:noop,error:noop,info:noop,debug:noop,trace:noop,group:noop,groupEnd:noop,time:noop,timeEnd:noop,assert:noop,dir:noop};
globalThis.self=globalThis;globalThis.globalThis=globalThis;
globalThis.top=globalThis;globalThis.parent=globalThis;globalThis.opener=null;globalThis.frames=[];
globalThis.location={href:'',protocol:'https:',host:'',hostname:'',pathname:'/',search:'',hash:'',assign:noop,replace:noop,reload:noop};
globalThis.navigator={userAgent:'Mozilla/5.0 (Bose Engine)',language:'en-US',languages:['en-US'],cookieEnabled:false,onLine:true};
globalThis.history={pushState:noop,replaceState:noop,back:noop,forward:noop,go:noop,length:1};
globalThis.screen={width:1920,height:1080,availWidth:1920,availHeight:1080};
// setTimeout: queue delay=0 callbacks for a single deferred-init pass after scripts run.
// Higher-delay and setInterval callbacks are silently dropped — recursive loops
// (setTimeout(check,100)) would stack-overflow if called synchronously.
globalThis.__bose_deferred__=[];
globalThis.setTimeout=function(fn,delay){
  if(typeof fn==='function'&&(delay===undefined||delay===0||delay===null)){
    globalThis.__bose_deferred__.push(fn);
  }
  return 0;
};
globalThis.clearTimeout=noop;globalThis.setInterval=function(){return 0};globalThis.clearInterval=noop;
globalThis.requestAnimationFrame=function(fn){
  if(typeof fn==='function'){ globalThis.__bose_deferred__.push(fn); }
  return 0;
};
globalThis.cancelAnimationFrame=noop;
globalThis.queueMicrotask=noop;
globalThis.localStorage={getItem:noopnull,setItem:noop,removeItem:noop,clear:noop,length:0};
globalThis.sessionStorage={getItem:noopnull,setItem:noop,removeItem:noop,clear:noop,length:0};
// Real fetch using __bose_dom__.httpGet/httpPost
globalThis.fetch=function(url,opts){
  try{
    var method=(opts&&opts.method)?opts.method.toUpperCase():'GET';
    var body=(opts&&opts.body)?String(opts.body):'';
    var ct=(opts&&opts.headers&&opts.headers['Content-Type'])?opts.headers['Content-Type']:'application/json';
    var text=method==='POST'?__bose_dom__.httpPost(String(url),body,ct):__bose_dom__.httpGet(String(url));
    var resp={ok:true,status:200,statusText:'OK',url:url,redirected:false,
      headers:{get:function(h){return h.toLowerCase()==='content-type'?'text/html':null;},has:function(){return false;}},
      text:function(){return Promise.resolve(text);},
      json:function(){try{return Promise.resolve(JSON.parse(text));}catch(e){return Promise.reject(e);}},
      arrayBuffer:function(){return Promise.resolve(new ArrayBuffer(0));},
      blob:function(){return Promise.resolve({});},
      clone:function(){return resp;}};
    return Promise.resolve(resp);
  }catch(e){return Promise.reject(e);}
};
// Real XMLHttpRequest using __bose_dom__.httpGet/httpPost
globalThis.XMLHttpRequest=function(){
  var self=this;
  self.readyState=0;self.status=0;self.statusText='';self.responseText='';self.response='';
  self.responseType='';self.onload=null;self.onerror=null;self.onreadystatechange=null;
  self._method='GET';self._url='';self._headers={};
  self.open=function(m,u){self._method=m.toUpperCase();self._url=u;self.readyState=1;};
  self.setRequestHeader=function(k,v){self._headers[k]=v;};
  self.send=function(body){
    try{
      var text=self._method==='POST'?__bose_dom__.httpPost(self._url,body?String(body):'',self._headers['Content-Type']||'application/json'):__bose_dom__.httpGet(self._url);
      self.readyState=4;self.status=200;self.statusText='OK';
      self.responseText=text;self.response=text;
    }catch(e){self.readyState=4;self.status=0;self.responseText='';}
    if(typeof self.onreadystatechange==='function'){try{self.onreadystatechange();}catch(e){}}
    if(self.status>=200&&self.status<300){if(typeof self.onload==='function'){try{self.onload({target:self});}catch(e){}}}
    else{if(typeof self.onerror==='function'){try{self.onerror();}catch(e){}}}
  };
  self.abort=noop;self.addEventListener=function(t,h){if(t==='load')self.onload=h;if(t==='error')self.onerror=h;};
  self.getResponseHeader=function(){return null;};self.getAllResponseHeaders=function(){return'';};
};
globalThis.CustomEvent=function(t,o){return{type:t,detail:o?o.detail:null,bubbles:!!(o&&o.bubbles),preventDefault:noop,stopPropagation:noop}};
globalThis.Event=function(t,o){return{type:t,bubbles:!!(o&&o.bubbles),preventDefault:noop,stopPropagation:noop,stopImmediatePropagation:noop}};
globalThis.MouseEvent=function(t,o){return{type:t,bubbles:!!(o&&o.bubbles),clientX:(o&&o.clientX)||0,clientY:(o&&o.clientY)||0,pageX:(o&&o.clientX)||0,pageY:(o&&o.clientY)||0,button:0,buttons:1,preventDefault:noop,stopPropagation:noop,stopImmediatePropagation:noop}};
globalThis.KeyboardEvent=function(t,o){return{type:t,bubbles:!!(o&&o.bubbles),key:(o&&o.key)||'',code:(o&&o.code)||'',keyCode:0,charCode:0,preventDefault:noop,stopPropagation:noop,stopImmediatePropagation:noop}};
globalThis.PointerEvent=function(t,o){return Object.assign(new MouseEvent(t,o),{pointerId:1,pointerType:'mouse',isPrimary:true})};
globalThis.TouchEvent=function(t,o){return{type:t,bubbles:!!(o&&o.bubbles),touches:[],changedTouches:[],targetTouches:[],preventDefault:noop,stopPropagation:noop}};
globalThis.FocusEvent=function(t,o){return{type:t,bubbles:!!(o&&o.bubbles),relatedTarget:null,preventDefault:noop,stopPropagation:noop}};
globalThis.MutationObserver=function(){return{observe:noop,disconnect:noop,takeRecords:nooparr}};
globalThis.IntersectionObserver=function(){return{observe:noop,disconnect:noop}};
globalThis.ResizeObserver=function(){return{observe:noop,disconnect:noop}};
globalThis.PerformanceObserver=function(f){return{observe:noop,disconnect:noop}};
globalThis.performance={now:function(){return 0},mark:noop,measure:noop,getEntriesByName:nooparr,getEntriesByType:nooparr};
globalThis.crypto={getRandomValues:function(a){return a},subtle:{digest:function(){return Promise.reject()}}};
globalThis.atob=function(s){try{var b=s.replace(/[^A-Za-z0-9+/=]/g,'');var chars='ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/';var out='';for(var i=0;i<b.length;i+=4){var c0=chars.indexOf(b[i]),c1=chars.indexOf(b[i+1]),c2=chars.indexOf(b[i+2]),c3=chars.indexOf(b[i+3]);out+=String.fromCharCode((c0<<2)|(c1>>4));if(c2!==64&&c2>=0)out+=String.fromCharCode(((c1&0xf)<<4)|(c2>>2));if(c3!==64&&c3>=0)out+=String.fromCharCode(((c2&3)<<6)|c3);}return out;}catch(e){return s;}};
globalThis.btoa=function(s){try{var chars='ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/';var out='';for(var i=0;i<s.length;i+=3){var b0=s.charCodeAt(i),b1=i+1<s.length?s.charCodeAt(i+1):0,b2=i+2<s.length?s.charCodeAt(i+2):0;out+=chars[b0>>2]+chars[((b0&3)<<4)|(b1>>4)]+(i+1<s.length?chars[((b1&0xf)<<2)|(b2>>6)]:'=')+(i+2<s.length?chars[b2&0x3f]:'=');}return out;}catch(e){return s;}};
globalThis.URL=function(url,base){
  var full=url;
  if(base&&!url.match(/^https?:\/\//)){
    try{var b=base.replace(/[?#].*/,'');full=url.startsWith('/')?b.replace(/(https?:\/\/[^/]+).*/,'$1')+url:b.replace(/\/[^/]*$/,'/')+url;}catch(e){}
  }
  this.href=full;this._url=full;
  try{var m=full.match(/^(https?:)\/\/([^/?#]+)(\/[^?#]*)?(\?[^#]*)?(#.*)?$/)||[];
    this.protocol=m[1]||'https:';this.host=m[2]||'';this.hostname=(m[2]||'').replace(/:\d+$/,'');
    this.port=((m[2]||'').match(/:([\d]+)$/)||[])[1]||'';
    this.pathname=m[3]||'/';this.search=m[4]||'';this.hash=m[5]||'';
    this.origin=this.protocol+'//'+this.host;
  }catch(e){}
  var sp=this.search?this.search.slice(1):'';
  var params={};
  if(sp)sp.split('&').forEach(function(p){var kv=p.split('=');params[decodeURIComponent(kv[0])]=decodeURIComponent(kv[1]||'');});
  this.searchParams={get:function(k){return params[k]||null;},set:function(k,v){params[k]=v;},has:function(k){return k in params;},toString:function(){return Object.keys(params).map(function(k){return encodeURIComponent(k)+'='+encodeURIComponent(params[k]);}).join('&');}};
  this.toString=function(){return full;};
};
globalThis.URLSearchParams=function(init){
  var params={};
  if(typeof init==='string'){var s=init.startsWith('?')?init.slice(1):init;if(s)s.split('&').forEach(function(p){var kv=p.split('=');params[decodeURIComponent(kv[0])]=decodeURIComponent(kv[1]||'');});}
  else if(typeof init==='object'&&init){Object.keys(init).forEach(function(k){params[k]=init[k];});}
  return{get:function(k){return params[k]||null;},set:function(k,v){params[k]=String(v);},has:function(k){return k in params;},append:function(k,v){params[k]=v;},delete:function(k){delete params[k];},toString:function(){return Object.keys(params).map(function(k){return encodeURIComponent(k)+'='+encodeURIComponent(params[k]);}).join('&');},entries:function(){return Object.keys(params).map(function(k){return[k,params[k]];});}};
};
globalThis.structuredClone=function(v){try{return JSON.parse(JSON.stringify(v));}catch(e){return v;}};
globalThis.AbortController=function(){var sig={aborted:false,addEventListener:noop,removeEventListener:noop};this.signal=sig;this.abort=function(){sig.aborted=true;};};
globalThis.AbortSignal={timeout:function(){return{aborted:false,addEventListener:noop};}};
globalThis.TextEncoder=function(){this.encode=function(s){return new Uint8Array(s.split('').map(function(c){return c.charCodeAt(0);}));};};
globalThis.TextDecoder=function(){this.decode=function(a){return Array.from(a).map(function(b){return String.fromCharCode(b);}).join('');};};
globalThis.Worker=function(){return{postMessage:noop,terminate:noop,addEventListener:noop}};
globalThis.WebSocket=function(){return{close:noop,send:noop,addEventListener:noop}};
globalThis.EventSource=function(){return{close:noop,addEventListener:noop}};
globalThis.Image=function(){return{src:'',onload:null,onerror:null}};
globalThis.Audio=function(){return{play:function(){return Promise.resolve()},pause:noop,load:noop}};
globalThis.getComputedStyle=function(){return{getPropertyValue:function(){return''}}};
globalThis.matchMedia=function(){return{matches:false,addListener:noop,removeListener:noop,addEventListener:noop}};
globalThis.scrollTo=noop;globalThis.scrollBy=noop;globalThis.scroll=noop;
globalThis.alert=noop;globalThis.confirm=function(){return false};globalThis.prompt=function(){return null};
globalThis.open=noop;globalThis.close=noop;globalThis.focus=noop;globalThis.blur=noop;
})();
"#;

const DOM_SHIM_JS: &str = r#"
(function(){
'use strict';
var bd=globalThis.__bose_dom__;
if(!bd)return;

function BoseElement(id){this._id=id;}
BoseElement.prototype={
constructor:BoseElement,
get tagName(){return(bd.getTagName(this._id)||'').toUpperCase();},
get nodeName(){return this.tagName;},
get nodeType(){return 1;},
get nodeValue(){return null;},
get id(){return bd.getAttribute(this._id,'id')||'';},
set id(v){bd.setAttribute(this._id,'id',String(v));},
get className(){return bd.getAttribute(this._id,'class')||'';},
set className(v){bd.setAttribute(this._id,'class',String(v));},
get innerHTML(){return bd.getInnerHTML(this._id)||'';},
set innerHTML(v){bd.setInnerHTML(this._id,String(v));},
get outerHTML(){return'<'+this.tagName.toLowerCase()+'>'+this.innerHTML+'</'+this.tagName.toLowerCase()+'>';},
get textContent(){return bd.getTextContent(this._id)||'';},
set textContent(v){bd.setTextContent(this._id,String(v));},
get innerText(){return this.textContent;},
set innerText(v){this.textContent=v;},
get parentElement(){var p=bd.getParent(this._id);return p<0?null:new BoseElement(p);},
get parentNode(){return this.parentElement;},
get children(){return(bd.getChildren(this._id)||[]).map(function(id){return new BoseElement(id);});},
get childNodes(){return this.children;},
get firstChild(){var c=bd.getChildren(this._id);return c&&c.length?new BoseElement(c[0]):null;},
get lastChild(){var c=bd.getChildren(this._id);return c&&c.length?new BoseElement(c[c.length-1]):null;},
get nextSibling(){var s=bd.getNextSibling(this._id);return s<0?null:new BoseElement(s);},
get previousSibling(){var s=bd.getPrevSibling(this._id);return s<0?null:new BoseElement(s);},
get style(){var self=this;return{setProperty:function(n,v){var cur=bd.getAttribute(self._id,'style')||'';bd.setAttribute(self._id,'style',cur+n+':'+v+';');},getPropertyValue:function(){return'';},removeProperty:function(){}};},
getAttribute:function(n){var v=bd.getAttribute(this._id,n);return(v===''||v===null||v===undefined)?null:v;},
setAttribute:function(n,v){bd.setAttribute(this._id,n,String(v));},
removeAttribute:function(n){bd.removeAttribute(this._id,n);},
hasAttribute:function(n){return bd.getAttribute(this._id,n)!='';},
querySelector:function(s){var id=bd.querySelector(this._id,s);return id<0?null:new BoseElement(id);},
querySelectorAll:function(s){return(bd.querySelectorAll(this._id,s)||[]).map(function(id){return new BoseElement(id);});},
getElementsByTagName:function(t){return(bd.getElementsByTag(this._id,t)||[]).map(function(id){return new BoseElement(id);});},
getElementsByClassName:function(){return[];},
appendChild:function(c){bd.appendChild(this._id,c._id);return c;},
insertBefore:function(n,_r){bd.appendChild(this._id,n._id);return n;},
removeChild:function(c){bd.removeNode(c._id);return c;},
remove:function(){bd.removeNode(this._id);},
replaceChild:function(n,o){bd.removeNode(o._id);bd.appendChild(this._id,n._id);return o;},
cloneNode:function(){return new BoseElement(this._id);},
contains:function(o){return o&&o._id===this._id;},
matches:function(s){var ids=bd.querySelectorAll(0,s)||[];return ids.indexOf(this._id)>=0;},
closest:function(s){var cur=this;while(cur){if(cur.matches&&cur.matches(s))return cur;cur=cur.parentElement;}return null;},
addEventListener:function(t,h,_o){bd.addEventListener(this._id,t);if(t==='DOMContentLoaded'&&typeof h==='function'){try{h({type:t});}catch(e){}}},
removeEventListener:function(){},
dispatchEvent:function(){return true;},
getBoundingClientRect:function(){return{top:0,left:0,width:0,height:0,right:0,bottom:0,x:0,y:0};},
focus:function(){},blur:function(){},click:function(){},scrollIntoView:function(){},
attachShadow:function(){return this;},
get shadowRoot(){return this;}
};

var boseDoc={
nodeType:9,readyState:'complete',compatMode:'CSS1Compat',
get title(){return bd.getTitle();},
set title(v){bd.setTitle(String(v));},
get body(){var id=bd.getBody();return id<0?null:new BoseElement(id);},
get head(){var id=bd.getHead();return id<0?null:new BoseElement(id);},
get documentElement(){var id=bd.getDocumentElement();return id<0?null:new BoseElement(id);},
cookie:'',domain:'',referrer:'',
getElementById:function(id){var nid=bd.getElementById(id);return nid<0?null:new BoseElement(nid);},
querySelector:function(s){var id=bd.querySelector(0,s);return id<0?null:new BoseElement(id);},
querySelectorAll:function(s){return(bd.querySelectorAll(0,s)||[]).map(function(id){return new BoseElement(id);});},
getElementsByTagName:function(t){return(bd.getElementsByTag(0,t)||[]).map(function(id){return new BoseElement(id);});},
getElementsByClassName:function(){return[];},
getElementsByName:function(){return[];},
createElement:function(t){return new BoseElement(bd.createElement(t.toLowerCase()));},
createTextNode:function(t){return new BoseElement(bd.createTextNode(t));},
createDocumentFragment:function(){return new BoseElement(bd.createElement('div'));},
createEvent:function(){return{initEvent:function(){},preventDefault:function(){},stopPropagation:function(){}};},
createComment:function(){return new BoseElement(bd.createTextNode(''));},
addEventListener:function(t,h,_o){if((t==='DOMContentLoaded'||t==='readystatechange')&&typeof h==='function'){try{h({type:t,target:boseDoc});}catch(e){}}},
removeEventListener:function(){},
dispatchEvent:function(){return true;},
write:function(){},writeln:function(){},open:function(){},close:function(){},
get defaultView(){return globalThis;}
};

globalThis.document=boseDoc;
globalThis.Document=function(){};
globalThis.HTMLDocument=function(){};
globalThis.Element=BoseElement;
globalThis.HTMLElement=BoseElement;
globalThis.Node={ELEMENT_NODE:1,TEXT_NODE:3,COMMENT_NODE:8,DOCUMENT_NODE:9,DOCUMENT_FRAGMENT_NODE:11};
globalThis.NodeList=Array;
globalThis.HTMLCollection=Array;

var _wAddEv=globalThis.addEventListener||function(){};
globalThis.addEventListener=function(t,h,o){
if(t==='DOMContentLoaded'||t==='load'){try{if(typeof h==='function')h({type:t,target:globalThis});}catch(e){}}
else{_wAddEv.call?_wAddEv.call(globalThis,t,h,o):_wAddEv(t,h,o);}
};
globalThis.window=globalThis;
})();
"#;

/// Evaluate a JS expression in an isolated QuickJS context and return the result as a String.
/// Used by CDP Runtime.evaluate.
pub async fn evaluate_expr(expr: String) -> String {
    tokio::task::spawn_blocking(move || -> String {
        let rt = match Runtime::new() {
            Ok(r) => r,
            Err(e) => return format!("Error: {}", e),
        };
        let ctx = match Context::full(&rt) {
            Ok(c) => c,
            Err(e) => return format!("Error: {}", e),
        };
        let result = Arc::new(Mutex::new(String::from("undefined")));
        let rc = Arc::clone(&result);
        ctx.with(|ctx| {
            // Install minimal stubs so common globals don't throw
            let _ = ctx.eval::<Value, _>(WEB_API_STUBS);
            // Wrap expression: coerce result to String via JS
            let code = format!(
                "(function(){{var __r=(function(){{return({});}}());if(__r===null)return 'null';if(__r===undefined)return 'undefined';if(typeof __r==='string')return __r;return String(__r);}})()",
                expr
            );
            match ctx.eval::<Value, _>(code.as_str()) {
                Ok(v) => {
                    // v is a JS string from our wrapper — extract as owned String
                    let s = if let Some(js_s) = v.as_string() {
                        js_s.to_string().unwrap_or_else(|_| "[error]".to_string())
                    } else {
                        "undefined".to_string()
                    };
                    *rc.lock().unwrap() = s;
                }
                Err(e) => {
                    *rc.lock().unwrap() = format!("Error: {}", e);
                }
            }
        });
        let out = result.lock().unwrap().clone();
        out
    })
    .await
    .unwrap_or_else(|_| "Error: thread panic".to_string())
}

// ── Compat stubs ─────────────────────────────────────────────────────────────

pub struct JsRuntime;
impl JsRuntime {
    pub fn new() -> Self {
        JsRuntime
    }
    pub fn execute(&self, _script: &str) -> BrowserResult<String> {
        Ok(String::new())
    }
}

pub struct JsContext;
impl JsContext {
    pub fn new() -> BrowserResult<Self> {
        Ok(JsContext)
    }
    pub async fn execute_script(&self, _script: &str) -> BrowserResult<String> {
        Ok(String::new())
    }
}
