use crate::bitfox::js_hooks::{run_js_security_hooks, HookVerdict};
use crate::bitfox::{bitfox_inspect, run_request_hooks, run_response_hooks, BitFoxContext, BitFoxDecision};
use serde::{Deserialize, Serialize};
use crate::browser::spa;
use crate::dom::document::Document;
use crate::dom::parser::HtmlParser;
use crate::dom::serializer::DomSerializer;
use crate::error::{BrowserError, BrowserResult};
use crate::http::client::HttpClient;
use async_trait::async_trait;
use std::sync::Arc;

#[async_trait]
pub trait PipelineStage: Send + Sync {
    fn name(&self) -> &'static str;
    async fn process(&self, ctx: &mut PipelineContext) -> BrowserResult<StageResult>;
}

pub enum StageResult {
    Continue,
    Complete(String),
    Block { reason: String },
}

pub struct PipelineContext {
    pub url: String,
    pub raw_html: Option<String>,
    pub document: Option<Document>,
    pub final_content: Option<String>,
}

impl PipelineContext {
    pub fn new(url: String) -> Self {
        PipelineContext {
            url,
            raw_html: None,
            document: None,
            final_content: None,
        }
    }
}

// ── Stages ────────────────────────────────────────────────────────────────────

pub struct NormalizeUrlStage;
pub struct JsSecurityStage;
pub struct HttpFetchStage {
    pub http: Arc<HttpClient>,
}
pub struct ParseHtmlStage {
    pub parser: Arc<HtmlParser>,
}
pub struct BitFoxStage;
pub struct YoutubeHydrateStage;
pub struct SerializeStage;

#[async_trait]
impl PipelineStage for NormalizeUrlStage {
    fn name(&self) -> &'static str {
        "normalize_url"
    }
    async fn process(&self, ctx: &mut PipelineContext) -> BrowserResult<StageResult> {
        let mut url = ctx.url.trim().to_string();
        let lower = url.to_ascii_lowercase();
        if lower.starts_with("https//") {
            url = format!("https://{}", &url[7..]);
        } else if lower.starts_with("https:/") && !lower.starts_with("https://") {
            url = format!("https://{}", &url[7..]);
        } else if lower.starts_with("http//") {
            url = format!("http://{}", &url[6..]);
        } else if lower.starts_with("http:/") && !lower.starts_with("http://") {
            url = format!("http://{}", &url[6..]);
        }

        let lower = url.to_ascii_lowercase();
        if lower.starts_with("https://https://") {
            url = format!("https://{}", &url["https://https://".len()..]);
        } else if lower.starts_with("http://http://") {
            url = format!("http://{}", &url["http://http://".len()..]);
        }

        if !(url.starts_with("http://") || url.starts_with("https://")) {
            url = format!("https://{}", url);
        }

        ctx.url = url::Url::parse(&url)
            .map_err(|e| BrowserError::UrlParse(e))?
            .to_string();

        // Run registered plugin request hooks — any plugin can rewrite the URL.
        ctx.url = run_request_hooks(&ctx.url);

        Ok(StageResult::Continue)
    }
}

#[async_trait]
impl PipelineStage for HttpFetchStage {
    fn name(&self) -> &'static str {
        "http_fetch"
    }
    async fn process(&self, ctx: &mut PipelineContext) -> BrowserResult<StageResult> {
        let html = self.http.fetch_html(&ctx.url).await?;
        ctx.raw_html = Some(html);
        Ok(StageResult::Continue)
    }
}

#[async_trait]
impl PipelineStage for ParseHtmlStage {
    fn name(&self) -> &'static str {
        "parse_html"
    }
    async fn process(&self, ctx: &mut PipelineContext) -> BrowserResult<StageResult> {
        let html = ctx.raw_html.as_deref().unwrap_or("");
        let doc = self.parser.parse(html, &ctx.url)?;
        ctx.document = Some(doc);
        Ok(StageResult::Continue)
    }
}

#[async_trait]
impl PipelineStage for JsSecurityStage {
    fn name(&self) -> &'static str {
        "js_security"
    }
    async fn process(&self, ctx: &mut PipelineContext) -> BrowserResult<StageResult> {
        // Run user-supplied JS plugin files from <exe>/plugins/*.js.
        // This is a blocking operation (rquickjs is not async); run on a thread pool.
        let url = ctx.url.clone();
        let verdict = tokio::task::spawn_blocking(move || run_js_security_hooks(&url))
            .await
            .map_err(|e| BrowserError::JavaScript(format!("js_security spawn: {}", e)))??;

        match verdict {
            HookVerdict::Allow => Ok(StageResult::Continue),
            HookVerdict::Block { reason } => {
                log::info!("JS security hook blocked {}: {}", ctx.url, reason);
                Ok(StageResult::Block { reason })
            }
            HookVerdict::Rewrite(new_url) => {
                log::info!("JS security hook rewrote {} -> {}", ctx.url, new_url);
                ctx.url = new_url;
                Ok(StageResult::Continue)
            }
        }
    }
}

#[async_trait]
impl PipelineStage for BitFoxStage {
    fn name(&self) -> &'static str {
        "bitfox_inspect"
    }
    async fn process(&self, ctx: &mut PipelineContext) -> BrowserResult<StageResult> {
        let bfctx = BitFoxContext {
            request_url: ctx.url.clone(),
            content_type: "text/html".to_string(),
            origin_signature: None,
            payload_hash: String::new(),
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64,
            session_id: uuid::Uuid::new_v4().to_string(),
        };
        match bitfox_inspect(bfctx) {
            BitFoxDecision::Block => {
                return Ok(StageResult::Block {
                    reason: "BitFox blocked this URL".to_string(),
                });
            }
            _ => {}
        }
        Ok(StageResult::Continue)
    }
}

#[async_trait]
impl PipelineStage for YoutubeHydrateStage {
    fn name(&self) -> &'static str {
        "youtube_hydrate"
    }
    async fn process(&self, ctx: &mut PipelineContext) -> BrowserResult<StageResult> {
        let host = url::Url::parse(&ctx.url)
            .ok()
            .and_then(|u| u.host_str().map(|h| h.to_string()))
            .unwrap_or_default();

        let is_youtube = host.ends_with("youtube.com") || host.ends_with("youtu.be");

        if let Some(doc) = ctx.document.as_mut() {
            if is_youtube {
                let raw = ctx.raw_html.as_deref().unwrap_or("");
                if spa::rewrite_youtube_watch_fallback(doc, raw, &ctx.url) {
                    spa::strip_external_resources(doc);
                    return Ok(StageResult::Continue);
                }
                if spa::is_spa_shell(doc) {
                    if !spa::hydrate_from_scripts(doc, raw) {
                        spa::inject_youtube_search_ui(doc, &ctx.url);
                    }
                    spa::strip_external_resources(doc);
                }
            }
        }
        Ok(StageResult::Continue)
    }
}

#[async_trait]
impl PipelineStage for SerializeStage {
    fn name(&self) -> &'static str {
        "serialize"
    }
    async fn process(&self, ctx: &mut PipelineContext) -> BrowserResult<StageResult> {
        let html = match &mut ctx.document {
            Some(doc) => {
                inject_engine_badge(doc, &ctx.url);
                DomSerializer::serialize(doc)?
            }
            None => ctx.raw_html.clone().unwrap_or_default(),
        };
        // Run registered plugin response hooks — any plugin can rewrite the HTML.
        let html = run_response_hooks(&ctx.url, &html);
        ctx.final_content = Some(html.clone());
        Ok(StageResult::Complete(html))
    }
}

fn inject_engine_badge(doc: &mut crate::dom::document::Document, url: &str) {
    use crate::dom::node::{Attribute, NodeData};

    let node_count = doc.nodes.len();
    let hostname = url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.to_string()))
        .unwrap_or_else(|| url.to_string());

    // Find or create body
    let body_id = match doc.find_element("body") {
        Some(id) => id,
        None => return,
    };

    // Badge HTML injected as a style + div
    let badge_style = r#"
        position:fixed;bottom:8px;right:8px;z-index:2147483647;
        background:#000080;color:#fff;font-family:'MS Sans Serif',Arial,sans-serif;
        font-size:11px;padding:4px 10px;border:2px solid #fff;
        box-shadow:2px 2px 0 #000;pointer-events:none;
    "#
    .replace('\n', "")
    .replace("        ", "");

    let badge_text = format!("Bose \u{2713} | {} nodes | {}", node_count, hostname);

    // <style> node
    let style_id = doc.create_node(NodeData::Element {
        tag_name: "style".to_string(),
        namespace: "http://www.w3.org/1999/xhtml".to_string(),
        attrs: vec![],
    });
    let style_text_id = doc.create_node(NodeData::Text {
        content: "#az-engine-badge{".to_string() + &badge_style + "}",
    });
    doc.append_child(style_id, style_text_id);

    // <div id="az-engine-badge">
    let div_id = doc.create_node(NodeData::Element {
        tag_name: "div".to_string(),
        namespace: "http://www.w3.org/1999/xhtml".to_string(),
        attrs: vec![
            Attribute {
                name: "id".to_string(),
                value: "az-engine-badge".to_string(),
            },
            Attribute {
                name: "style".to_string(),
                value: badge_style,
            },
        ],
    });
    let text_id = doc.create_node(NodeData::Text {
        content: badge_text,
    });
    doc.append_child(div_id, text_id);

    doc.append_child(body_id, style_id);
    doc.append_child(body_id, div_id);
}

// ── Pipeline ──────────────────────────────────────────────────────────────────

/// Serializable pipeline configuration — read from `<exe>/pipeline.ron`.
///
/// Example `pipeline.ron`:
/// ```ron
/// (stages: ["NormalizeUrl", "JsSecurity", "BitFox", "HttpFetch", "ParseHtml", "YoutubeHydrate", "Serialize"])
/// ```
#[derive(Debug, Serialize, Deserialize)]
pub struct PipelineConfig {
    pub stages: Vec<String>,
}

impl Default for PipelineConfig {
    fn default() -> Self {
        PipelineConfig {
            stages: vec![
                "NormalizeUrl".into(),
                "JsSecurity".into(),
                "BitFox".into(),
                "HttpFetch".into(),
                "ParseHtml".into(),
                "YoutubeHydrate".into(),
                "Serialize".into(),
            ],
        }
    }
}

fn load_pipeline_config() -> PipelineConfig {
    let ron_path = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("pipeline.ron")))
        .unwrap_or_else(|| std::path::PathBuf::from("pipeline.ron"));

    if ron_path.exists() {
        match std::fs::read_to_string(&ron_path) {
            Ok(text) => match ron::from_str::<PipelineConfig>(&text) {
                Ok(cfg) => {
                    log::info!("Pipeline: loaded config from {:?} ({} stages)", ron_path, cfg.stages.len());
                    return cfg;
                }
                Err(e) => log::warn!("Pipeline: invalid pipeline.ron: {}; using defaults", e),
            },
            Err(e) => log::debug!("Pipeline: could not read pipeline.ron: {}", e),
        }
    }
    PipelineConfig::default()
}

pub struct Pipeline {
    stages: Vec<Box<dyn PipelineStage>>,
}

impl Pipeline {
    pub fn new(http: Arc<HttpClient>, parser: Arc<HtmlParser>) -> Self {
        let cfg = load_pipeline_config();
        let stages = Self::build_stages(cfg, http, parser);
        Pipeline { stages }
    }

    fn build_stages(
        cfg:    PipelineConfig,
        http:   Arc<HttpClient>,
        parser: Arc<HtmlParser>,
    ) -> Vec<Box<dyn PipelineStage>> {
        // http/parser are consumed by the first HttpFetch/ParseHtml that needs them
        let mut http_slot   = Some(http);
        let mut parser_slot = Some(parser);
        let mut out: Vec<Box<dyn PipelineStage>> = Vec::new();

        for name in &cfg.stages {
            match name.as_str() {
                "NormalizeUrl" => out.push(Box::new(NormalizeUrlStage)),
                "JsSecurity"   => out.push(Box::new(JsSecurityStage)),
                "BitFox"       => out.push(Box::new(BitFoxStage)),
                "HttpFetch"    => {
                    if let Some(h) = http_slot.take() {
                        out.push(Box::new(HttpFetchStage { http: h }));
                    } else {
                        log::warn!("Pipeline: HttpFetch listed twice in pipeline.ron; ignoring duplicate");
                    }
                }
                "ParseHtml"    => {
                    if let Some(p) = parser_slot.take() {
                        out.push(Box::new(ParseHtmlStage { parser: p }));
                    } else {
                        log::warn!("Pipeline: ParseHtml listed twice in pipeline.ron; ignoring duplicate");
                    }
                }
                "YoutubeHydrate" => out.push(Box::new(YoutubeHydrateStage)),
                "Serialize"      => out.push(Box::new(SerializeStage)),
                unknown          => log::warn!("Pipeline: unknown stage {:?} in pipeline.ron; skipped", unknown),
            }
        }

        // Ensure mandatory stages are present even if missing from config
        if !out.iter().any(|s| s.name() == "http_fetch") {
            if let Some(h) = http_slot.take() {
                log::warn!("Pipeline: HttpFetch missing from pipeline.ron; appending");
                out.push(Box::new(HttpFetchStage { http: h }));
            }
        }
        if !out.iter().any(|s| s.name() == "parse_html") {
            if let Some(p) = parser_slot.take() {
                log::warn!("Pipeline: ParseHtml missing from pipeline.ron; appending");
                out.push(Box::new(ParseHtmlStage { parser: p }));
            }
        }
        if !out.iter().any(|s| s.name() == "serialize") {
            log::warn!("Pipeline: Serialize missing from pipeline.ron; appending");
            out.push(Box::new(SerializeStage));
        }

        out
    }

    /// Execute pipeline and return (html, document) so callers can keep the DOM.
    pub async fn execute_with_state(&self, url: &str) -> BrowserResult<(String, Option<Document>)> {
        let mut ctx = PipelineContext::new(url.to_string());
        for stage in &self.stages {
            log::debug!("Pipeline stage: {}", stage.name());
            match stage.process(&mut ctx).await? {
                StageResult::Complete(html) => return Ok((html, ctx.document.take())),
                StageResult::Block { reason } => {
                    return Err(BrowserError::Navigation(reason));
                }
                StageResult::Continue => {}
            }
        }
        let html = ctx
            .final_content
            .ok_or_else(|| BrowserError::Navigation("Pipeline: no output".to_string()))?;
        Ok((html, ctx.document.take()))
    }

    pub async fn execute(&self, url: &str) -> BrowserResult<String> {
        self.execute_with_state(url).await.map(|(html, _)| html)
    }
}
