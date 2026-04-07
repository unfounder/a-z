/// SPA data extractor — reads embedded JSON blobs from known SPA frameworks
/// and injects readable HTML content into the document body.
///
/// Supported:
///   - YouTube  (ytInitialData / InnerTube API fallback)
///   - Next.js  (__NEXT_DATA__)
///   - Remix    (__remixContext)
///   - Nuxt     /__nuxt_data__ / window.__NUXT__
use crate::dom::document::Document;
use crate::dom::node::{NodeData, NodeId};

/// Inject a YouTube search interface when the homepage has no video content.
/// YouTube doesn't serve video data to unauthenticated requests on the homepage,
/// but search result pages (/results?search_query=...) DO return full video data.
pub fn inject_youtube_search_ui(doc: &mut Document, url: &str) {
    // Only inject for YouTube URLs
    let is_youtube = url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.ends_with("youtube.com")))
        .unwrap_or(false);
    if !is_youtube {
        return;
    }

    let body_id = match doc.find_element("body") {
        Some(id) => id,
        None => return,
    };

    let html = r#"<div id="bose-spa-content" style="font-family:Arial,sans-serif;background:#0f0f0f;color:#fff;min-height:100vh;display:flex;flex-direction:column;align-items:center;padding-top:80px">
  <div style="display:flex;align-items:center;gap:12px;margin-bottom:40px">
    <span style="color:#ff0000;font-size:36px;font-weight:bold">▶ YouTube</span>
    <span style="color:#aaa;font-size:13px">— Bose engine</span>
  </div>
  <form action="https://www.youtube.com/results" method="get" style="display:flex;gap:8px;width:100%;max-width:600px">
    <input name="search_query" type="text" placeholder="Search YouTube..."
      style="flex:1;padding:12px 16px;font-size:16px;border:1px solid #555;border-radius:24px;background:#121212;color:#fff;outline:none">
    <button type="submit"
      style="padding:12px 20px;background:#ff0000;color:#fff;border:none;border-radius:24px;font-size:15px;font-weight:600;cursor:pointer">
      Search
    </button>
  </form>
  <p style="color:#555;font-size:12px;margin-top:20px">
    YouTube homepage requires sign-in for recommendations. Use search to find videos.
  </p>
</div>"#;

    inject_html_into_body(doc, body_id, html);
}

/// Strip all external scripts and stylesheets from the document.
/// Called after SPA hydration so that external JS can't re-hydrate and overwrite our content.
pub fn strip_external_resources(doc: &mut Document) {
    // Collect all <script src> and <link rel=stylesheet> node IDs
    let to_remove: Vec<NodeId> = doc
        .nodes
        .iter()
        .enumerate()
        .filter_map(|(id, node)| {
            match &node.data {
                NodeData::Element {
                    tag_name, attrs, ..
                } => {
                    if tag_name == "script" {
                        // Remove <script src="..."> — external scripts
                        if attrs.iter().any(|a| a.name == "src" && !a.value.is_empty()) {
                            return Some(id);
                        }
                        // Also remove inline scripts (they may navigate/redirect)
                        return Some(id);
                    }
                    if tag_name == "link" {
                        let rel = attrs
                            .iter()
                            .find(|a| a.name == "rel")
                            .map(|a| a.value.as_str())
                            .unwrap_or("");
                        // Remove stylesheets and preloads (they can trigger redirects or override our layout)
                        if rel.contains("stylesheet") || rel == "preload" || rel == "prefetch" {
                            return Some(id);
                        }
                    }
                    None
                }
                _ => None,
            }
        })
        .collect();

    for id in to_remove {
        doc.remove_from_parent(id);
    }
}

/// Returns true if the body is an empty SPA shell (little visible text, custom elements present).
pub fn is_spa_shell(doc: &Document) -> bool {
    let body_id = match doc.find_element("body") {
        Some(id) => id,
        None => return false,
    };
    let text = doc.get_text_content(body_id);
    let visible: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    // Body has very little text but lots of nodes — classic hydration shell
    visible.len() < 400 && doc.nodes.len() > 50
}

/// Extract SPA data from inline scripts and inject rendered HTML into body.
/// Returns true if anything was injected.
pub fn hydrate_from_scripts(doc: &mut Document, raw_html: &str) -> bool {
    // Try each framework in priority order
    try_youtube(doc, raw_html)
        || try_nextjs(doc, raw_html)
        || try_remix(doc, raw_html)
        || try_nuxt(doc, raw_html)
}

// ── YouTube ────────────────────────────────────────────────────────────────

fn try_youtube(doc: &mut Document, raw_html: &str) -> bool {
    try_youtube_inner(doc, raw_html).unwrap_or(false)
}

/// Replace unsupported YouTube watch pages with a stable native fallback view.
/// This keeps the canvas deterministic when MSE/EME playback is unavailable.
pub fn rewrite_youtube_watch_fallback(doc: &mut Document, raw_html: &str, url: &str) -> bool {
    let Ok(parsed) = url::Url::parse(url) else {
        return false;
    };
    let Some(host) = parsed.host_str() else {
        return false;
    };
    if !(host.ends_with("youtube.com") || host.ends_with("youtu.be")) {
        return false;
    }

    let path = parsed.path();
    let video_id = if host.ends_with("youtu.be") {
        parsed
            .path_segments()
            .and_then(|mut seg| seg.next().map(|s| s.to_string()))
            .unwrap_or_default()
    } else if path == "/watch" {
        parsed
            .query_pairs()
            .find(|(k, _)| k == "v")
            .map(|(_, v)| v.to_string())
            .unwrap_or_default()
    } else {
        String::new()
    };

    if video_id.is_empty() {
        return false;
    }

    let body_id = match doc.find_element("body") {
        Some(id) => id,
        None => return false,
    };

    let mut title = "YouTube Video".to_string();
    let mut channel = String::new();
    let mut views = String::new();
    let mut length = String::new();

    if let Some(player_json) = extract_js_var(raw_html, "ytInitialPlayerResponse") {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(player_json) {
            if let Some(t) = v
                .pointer("/videoDetails/title")
                .and_then(|x| x.as_str())
                .filter(|s| !s.trim().is_empty())
            {
                title = t.to_string();
            }
            if let Some(c) = v
                .pointer("/videoDetails/author")
                .and_then(|x| x.as_str())
                .filter(|s| !s.trim().is_empty())
            {
                channel = c.to_string();
            }
            if let Some(vc) = v
                .pointer("/videoDetails/viewCount")
                .and_then(|x| x.as_str())
                .filter(|s| !s.trim().is_empty())
            {
                views = format!("{} views", vc);
            }
            if let Some(sec) = v
                .pointer("/videoDetails/lengthSeconds")
                .and_then(|x| x.as_str())
                .filter(|s| !s.trim().is_empty())
            {
                length = format!("{} sec", sec);
            }
        }
    }

    let thumb = format!("https://i.ytimg.com/vi/{}/hqdefault.jpg", video_id);
    let watch = format!("https://www.youtube.com/watch?v={}", video_id);
    let query = url::form_urlencoded::byte_serialize(title.as_bytes()).collect::<String>();
    let related = format!("https://www.youtube.com/results?search_query={}", query);

    let mut html = String::from(
        r#"<div id="bose-spa-content" style="font-family:Arial,sans-serif;background:#111;color:#fff;min-height:100vh;padding:18px">"#,
    );
    html.push_str(
        r#"<div style="background:#1e1e1e;border:1px solid #333;border-radius:10px;padding:14px;margin-bottom:14px">"#,
    );
    html.push_str(
        r#"<div style="font-size:14px;color:#ffcc80;margin-bottom:8px"><b>Native canvas mode:</b> YouTube playback APIs (MSE/EME/DRM) are not implemented yet.</div>"#,
    );
    html.push_str(
        r#"<div style="font-size:12px;color:#bbb">Page remains stable in-canvas. Use the <b>Videos -> Play externally</b> action for playback.</div>"#,
    );
    html.push_str("</div>");

    html.push_str(r#"<div style="display:grid;grid-template-columns:minmax(320px,640px) 1fr;gap:16px;align-items:start">"#);
    html.push_str(&format!(
        r#"<img src="{thumb}" style="width:100%;max-width:640px;border-radius:8px;border:1px solid #333" loading="lazy">"#,
        thumb = thumb
    ));
    html.push_str("<div>");
    html.push_str(&format!(
        r#"<div style="font-size:22px;font-weight:700;line-height:1.3;margin-bottom:8px">{}</div>"#,
        html_escape(&title)
    ));
    if !channel.is_empty() {
        html.push_str(&format!(
            r#"<div style="font-size:14px;color:#b0bec5;margin-bottom:6px">{}</div>"#,
            html_escape(&channel)
        ));
    }
    if !views.is_empty() || !length.is_empty() {
        html.push_str(&format!(
            r#"<div style="font-size:12px;color:#90a4ae;margin-bottom:12px">{} {}</div>"#,
            html_escape(&views),
            html_escape(&length)
        ));
    }
    html.push_str(&format!(
        r#"<div style="display:flex;gap:10px;flex-wrap:wrap">
            <a href="{watch}" style="padding:9px 14px;background:#ff0033;color:#fff;border-radius:8px;text-decoration:none;font-weight:600">Open watch URL</a>
            <a href="{related}" style="padding:9px 14px;background:#263238;color:#fff;border-radius:8px;text-decoration:none">Related videos</a>
          </div>"#,
        watch = watch,
        related = related
    ));
    html.push_str("</div></div></div>");

    inject_html_into_body(doc, body_id, &html);
    true
}

fn try_youtube_inner(doc: &mut Document, raw_html: &str) -> Option<bool> {
    let json_str = extract_js_var(raw_html, "ytInitialData")?;
    let data: serde_json::Value = serde_json::from_str(json_str).ok()?;

    let body_id = doc.find_element("body")?;

    // Extract video items from the nested structure
    let mut videos: Vec<(String, String, String, String)> = Vec::new();

    if let Some(tabs) = data.pointer("/contents/twoColumnBrowseResultsRenderer/tabs") {
        if let Some(tab) = tabs.get(0) {
            if let Some(items) = tab.pointer("/tabRenderer/content/richGridRenderer/contents") {
                for item in items.as_array().unwrap_or(&vec![]) {
                    if let Some(video) = item.pointer("/richItemRenderer/content/videoRenderer") {
                        if let Some(v) = extract_video(video) {
                            videos.push(v);
                        }
                    }
                    if let Some(shelf) =
                        item.pointer("/richSectionRenderer/content/richShelfRenderer/contents")
                    {
                        for s in shelf.as_array().unwrap_or(&vec![]) {
                            if let Some(video) =
                                s.pointer("/richItemRenderer/content/videoRenderer")
                            {
                                if let Some(v) = extract_video(video) {
                                    videos.push(v);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // Also check search result pages: /contents/twoColumnSearchResultsRenderer
    if videos.is_empty() {
        if let Some(items) = data.pointer(
            "/contents/twoColumnSearchResultsRenderer/primaryContents/sectionListRenderer/contents",
        ) {
            for section in items.as_array().unwrap_or(&vec![]) {
                if let Some(contents) = section.pointer("/itemSectionRenderer/contents") {
                    for item in contents.as_array().unwrap_or(&vec![]) {
                        if let Some(v) = extract_video(item.get("videoRenderer").unwrap_or(item)) {
                            videos.push(v);
                        }
                    }
                }
            }
        }
    }

    // Fallback: deep scan for any videoRenderer
    if videos.is_empty() {
        videos = deep_find_videos(&data, 0);
    }

    if videos.is_empty() {
        return None;
    }

    let html = build_youtube_html(&videos, "YouTube");
    inject_html_into_body(doc, body_id, &html);
    Some(true)
}

/// Build YouTube video grid HTML from a list of (id, title, channel, views).
pub fn build_youtube_html(videos: &[(String, String, String, String)], heading: &str) -> String {
    let mut html = String::from(
        r#"<div id="bose-spa-content" style="font-family:Arial,sans-serif;padding:16px;background:#0f0f0f;color:#fff;min-height:100vh">"#,
    );
    html.push_str(r#"<div style="display:flex;align-items:center;gap:12px;margin-bottom:20px;border-bottom:1px solid #333;padding-bottom:12px">"#);
    html.push_str(&format!(
        r#"<span style="color:#ff0000;font-size:22px;font-weight:bold">▶ {}</span>"#,
        html_escape(heading)
    ));
    html.push_str(r#"<span style="color:#aaa;font-size:12px">— rendered by Bose engine</span>"#);
    html.push_str("</div>");
    html.push_str(r#"<div style="display:grid;grid-template-columns:repeat(auto-fill,minmax(280px,1fr));gap:16px">"#);
    for (id, title, channel, views) in videos {
        let thumb = format!("https://i.ytimg.com/vi/{}/hqdefault.jpg", id);
        let url = format!("https://www.youtube.com/watch?v={}", id);
        html.push_str(&format!(
            r#"<div style="background:#1a1a1a;border-radius:8px;overflow:hidden">
              <a href="{url}" style="text-decoration:none;color:inherit">
                <img src="{thumb}" style="width:100%;aspect-ratio:16/9;object-fit:cover;display:block" loading="lazy">
                <div style="padding:10px">
                  <div style="font-size:14px;font-weight:600;color:#fff;line-height:1.4;margin-bottom:6px">{title}</div>
                  <div style="font-size:12px;color:#aaa">{channel}</div>
                  <div style="font-size:11px;color:#777;margin-top:2px">{views}</div>
                </div>
              </a>
            </div>"#,
            url = url, thumb = thumb,
            title = html_escape(title), channel = html_escape(channel), views = views
        ));
    }
    html.push_str("</div></div>");
    html
}

fn extract_video(v: &serde_json::Value) -> Option<(String, String, String, String)> {
    let id = v.get("videoId")?.as_str()?.to_string();
    let title = v
        .pointer("/title/runs/0/text")
        .or_else(|| v.pointer("/title/simpleText"))
        .and_then(|t| t.as_str())
        .unwrap_or("")
        .to_string();
    let channel = v
        .pointer("/ownerText/runs/0/text")
        .or_else(|| v.pointer("/longBylineText/runs/0/text"))
        .or_else(|| v.pointer("/shortBylineText/runs/0/text"))
        .and_then(|t| t.as_str())
        .unwrap_or("")
        .to_string();
    let views = v
        .pointer("/viewCountText/simpleText")
        .or_else(|| v.pointer("/shortViewCountText/simpleText"))
        .and_then(|t| t.as_str())
        .unwrap_or("")
        .to_string();
    if title.is_empty() {
        return None;
    }
    Some((id, title, channel, views))
}

fn deep_find_videos(
    val: &serde_json::Value,
    depth: usize,
) -> Vec<(String, String, String, String)> {
    if depth > 12 {
        return vec![];
    }
    let mut found = vec![];
    match val {
        serde_json::Value::Object(map) => {
            if map.contains_key("videoId") {
                if let Some(v) = extract_video(val) {
                    found.push(v);
                }
            }
            for (_, v) in map {
                found.extend(deep_find_videos(v, depth + 1));
            }
        }
        serde_json::Value::Array(arr) => {
            for v in arr {
                found.extend(deep_find_videos(v, depth + 1));
            }
        }
        _ => {}
    }
    found
}

// ── Next.js ────────────────────────────────────────────────────────────────

fn try_nextjs(doc: &mut Document, raw_html: &str) -> bool {
    try_nextjs_inner(doc, raw_html).unwrap_or(false)
}

fn try_nextjs_inner(doc: &mut Document, raw_html: &str) -> Option<bool> {
    let json_str = extract_js_var(raw_html, "__NEXT_DATA__")?;
    let data: serde_json::Value = serde_json::from_str(json_str).ok()?;
    let body_id = doc.find_element("body")?;

    let page = data.get("page").and_then(|v| v.as_str()).unwrap_or("/");
    let query = data.get("query");
    let props = data.pointer("/props/pageProps");

    let mut html = String::from(
        r#"<div id="bose-spa-content" style="font-family:Arial,sans-serif;padding:16px">"#,
    );
    html.push_str(&format!(
        r#"<div style="background:#0070f3;color:#fff;padding:8px 12px;border-radius:4px;margin-bottom:16px;font-size:13px">Bose — Next.js page: <b>{}</b></div>"#,
        html_escape(page)
    ));

    // Render page props as readable key/value
    if let Some(props_val) = props {
        html.push_str("<div style=\"font-size:13px;line-height:1.8\">");
        render_json_as_html(props_val, &mut html, 0);
        html.push_str("</div>");
    } else if let Some(q) = query {
        html.push_str(&format!(
            "<pre style=\"font-size:12px\">{}</pre>",
            html_escape(&serde_json::to_string_pretty(q).unwrap_or_default())
        ));
    }

    html.push_str("</div>");
    inject_html_into_body(doc, body_id, &html);
    Some(true)
}

// ── Remix ──────────────────────────────────────────────────────────────────

fn try_remix(doc: &mut Document, raw_html: &str) -> bool {
    try_remix_inner(doc, raw_html).unwrap_or(false)
}

fn try_remix_inner(doc: &mut Document, raw_html: &str) -> Option<bool> {
    let json_str = extract_js_var(raw_html, "__remixContext")?;
    let data: serde_json::Value = serde_json::from_str(json_str).ok()?;
    let body_id = doc.find_element("body")?;

    let mut html = String::from(
        r#"<div id="bose-spa-content" style="font-family:Arial,sans-serif;padding:16px">"#,
    );
    html.push_str(r#"<div style="background:#8b5cf6;color:#fff;padding:8px 12px;border-radius:4px;margin-bottom:16px;font-size:13px">Bose — Remix app</div>"#);
    html.push_str("<div style=\"font-size:13px;line-height:1.8\">");
    render_json_as_html(&data, &mut html, 0);
    html.push_str("</div></div>");
    inject_html_into_body(doc, body_id, &html);
    Some(true)
}

// ── Nuxt ───────────────────────────────────────────────────────────────────

fn try_nuxt(doc: &mut Document, raw_html: &str) -> bool {
    try_nuxt_inner(doc, raw_html).unwrap_or(false)
}

fn try_nuxt_inner(doc: &mut Document, raw_html: &str) -> Option<bool> {
    let json_str = extract_js_var(raw_html, "window.__NUXT__")?;
    let data: serde_json::Value = serde_json::from_str(json_str).ok()?;
    let body_id = doc.find_element("body")?;

    let mut html = String::from(
        r#"<div id="bose-spa-content" style="font-family:Arial,sans-serif;padding:16px">"#,
    );
    html.push_str(r#"<div style="background:#00DC82;color:#000;padding:8px 12px;border-radius:4px;margin-bottom:16px;font-size:13px">Bose — Nuxt app</div>"#);
    html.push_str("<div style=\"font-size:13px;line-height:1.8\">");
    render_json_as_html(&data, &mut html, 0);
    html.push_str("</div></div>");
    inject_html_into_body(doc, body_id, &html);
    Some(true)
}

// ── Helpers ────────────────────────────────────────────────────────────────

/// Extract the JSON value assigned to a JS variable name in raw HTML.
/// Handles:
///   `var NAME = {...}`   `var NAME={`          (spaced / minified)
///   `window.NAME = {`   `window.NAME={`
///   `window["NAME"] = {` `window["NAME"]={`    (bracket double-quote)
///   `window['NAME'] = {` `window['NAME']={`    (bracket single-quote)
///   `NAME = {`           `NAME={`              (bare assignment)
///   `"NAME":{`                                 (JSON key form)
fn extract_js_var<'a>(html: &'a str, name: &str) -> Option<&'a str> {
    let needles: &[String] = &[
        format!("var {} = ", name),
        format!("var {}=", name),
        format!("window.{} = ", name),
        format!("window.{}=", name),
        format!("window[\"{}\"] = ", name),
        format!("window[\"{}\"]= ", name),
        format!("window[\"{}\"]=", name),
        format!("window['{}'] = ", name),
        format!("window['{}']= ", name),
        format!("window['{}']=", name),
        format!("{} = ", name),
        format!("{}=", name),
        format!("\"{}\":", name),
    ];
    for needle in needles {
        if let Some(pos) = html.find(needle.as_str()) {
            let after = &html[pos + needle.len()..];
            let trimmed = after.trim_start();
            if trimmed.starts_with('{') || trimmed.starts_with('[') {
                if let Some(end) = find_json_end(trimmed) {
                    return Some(&trimmed[..end]);
                }
            }
        }
    }
    None
}

/// Find the end of a JSON object or array in the string (handles nesting + strings).
fn find_json_end(s: &str) -> Option<usize> {
    let mut depth: i32 = 0;
    let mut in_string = false;
    let mut escaped = false;
    let bytes = s.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        if escaped {
            escaped = false;
            continue;
        }
        match b {
            b'\\' if in_string => escaped = true,
            b'"' => in_string = !in_string,
            b'{' | b'[' if !in_string => depth += 1,
            b'}' | b']' if !in_string => {
                depth -= 1;
                if depth == 0 {
                    return Some(i + 1);
                }
            }
            _ => {}
        }
    }
    None
}

/// Render a JSON value as simple readable HTML, depth-limited.
fn render_json_as_html(val: &serde_json::Value, buf: &mut String, depth: usize) {
    if depth > 4 {
        buf.push_str("<span style='color:#888'>[...]</span>");
        return;
    }
    match val {
        serde_json::Value::Object(map) => {
            buf.push_str("<table style='border-collapse:collapse;width:100%;font-size:13px'>");
            for (k, v) in map.iter().take(30) {
                buf.push_str("<tr>");
                buf.push_str(&format!(
                    "<td style='padding:2px 8px 2px 0;color:#666;vertical-align:top;white-space:nowrap'>{}</td><td style='padding:2px 0'>",
                    html_escape(k)
                ));
                render_json_as_html(v, buf, depth + 1);
                buf.push_str("</td></tr>");
            }
            if map.len() > 30 {
                buf.push_str(&format!(
                    "<tr><td colspan=2 style='color:#888;font-size:11px'>...{} more</td></tr>",
                    map.len() - 30
                ));
            }
            buf.push_str("</table>");
        }
        serde_json::Value::Array(arr) => {
            buf.push_str(&format!(
                "<span style='color:#888'>[{} items]</span>",
                arr.len()
            ));
        }
        serde_json::Value::String(s) => {
            let display = if s.len() > 200 {
                format!("{}…", &s[..200])
            } else {
                s.clone()
            };
            buf.push_str(&html_escape(&display));
        }
        serde_json::Value::Number(n) => buf.push_str(&n.to_string()),
        serde_json::Value::Bool(b) => buf.push_str(if *b { "true" } else { "false" }),
        serde_json::Value::Null => buf.push_str("<span style='color:#888'>null</span>"),
    }
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#x27;")
}

/// Inject HTML string into a body node, replacing all existing children.
fn inject_html_into_body(doc: &mut Document, body_id: NodeId, html: &str) {
    use crate::dom::parser::HtmlParser;
    let wrapped = format!("<html><body>{}</body></html>", html);
    let parser = HtmlParser::new();
    if let Ok(frag) = parser.parse(&wrapped, "") {
        if let Some(frag_body) = frag.find_element("body") {
            let src_children: Vec<NodeId> = frag
                .get(frag_body)
                .map(|n| n.children.clone())
                .unwrap_or_default();
            doc.replace_inner_html(body_id, &frag, &src_children);
        }
    }
}
