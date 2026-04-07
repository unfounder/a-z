/// Phase 4: Extract external asset URLs from a Document.
/// Returns lists of CSS, JS, image, and font URLs found in <link> / <script src> / <img> nodes.
/// These are returned as metadata so the caller (CDP, MCP, UI) can pre-fetch
/// or display them — the Bose engine does not currently fetch sub-resources itself.
use crate::dom::document::Document;
use crate::dom::node::{NodeData, NodeId};

#[derive(Debug, Clone, Default)]
pub struct PageAssets {
    pub stylesheets: Vec<String>,
    pub scripts: Vec<String>,
    pub images: Vec<String>,
    pub fonts: Vec<String>,
    pub prefetch: Vec<String>,
}

impl PageAssets {
    pub fn total(&self) -> usize {
        self.stylesheets.len()
            + self.scripts.len()
            + self.images.len()
            + self.fonts.len()
            + self.prefetch.len()
    }
}

/// Walk the document and collect all external asset URLs.
pub fn extract_assets(doc: &Document, base_url: &str) -> PageAssets {
    let mut assets = PageAssets::default();
    walk(doc, 0, base_url, &mut assets);
    assets
}

fn walk(doc: &Document, node_id: NodeId, base_url: &str, assets: &mut PageAssets) {
    walk_depth(doc, node_id, base_url, assets, 0);
}

fn walk_depth(
    doc: &Document,
    node_id: NodeId,
    base_url: &str,
    assets: &mut PageAssets,
    depth: usize,
) {
    if depth > 512 {
        return;
    }
    let node = match doc.get(node_id) {
        Some(n) => n,
        None => return,
    };

    match &node.data {
        NodeData::Element {
            tag_name, attrs, ..
        } => {
            match tag_name.as_str() {
                "link" => {
                    let rel = attrs
                        .iter()
                        .find(|a| a.name == "rel")
                        .map(|a| a.value.as_str())
                        .unwrap_or("");
                    let href = attrs
                        .iter()
                        .find(|a| a.name == "href")
                        .map(|a| a.value.as_str())
                        .unwrap_or("");
                    if href.is_empty() { /* skip */
                    } else if rel.contains("stylesheet") {
                        assets.stylesheets.push(resolve(href, base_url));
                    } else if rel == "preload" || rel == "prefetch" {
                        let as_attr = attrs
                            .iter()
                            .find(|a| a.name == "as")
                            .map(|a| a.value.as_str())
                            .unwrap_or("");
                        match as_attr {
                            "font" => assets.fonts.push(resolve(href, base_url)),
                            "style" => assets.stylesheets.push(resolve(href, base_url)),
                            "script" => assets.scripts.push(resolve(href, base_url)),
                            _ => assets.prefetch.push(resolve(href, base_url)),
                        }
                    }
                }
                "script" => {
                    if let Some(src) = attrs
                        .iter()
                        .find(|a| a.name == "src")
                        .map(|a| a.value.as_str())
                    {
                        if !src.is_empty() {
                            assets.scripts.push(resolve(src, base_url));
                        }
                    }
                }
                "img" => {
                    if let Some(src) = attrs
                        .iter()
                        .find(|a| a.name == "src")
                        .map(|a| a.value.as_str())
                    {
                        if !src.is_empty() {
                            assets.images.push(resolve(src, base_url));
                        }
                    }
                    // srcset
                    if let Some(srcset) = attrs
                        .iter()
                        .find(|a| a.name == "srcset")
                        .map(|a| a.value.as_str())
                    {
                        for part in srcset.split(',') {
                            let url = part.trim().split_whitespace().next().unwrap_or("");
                            if !url.is_empty() {
                                assets.images.push(resolve(url, base_url));
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        _ => {}
    }

    let children = node.children.clone();
    for child_id in children {
        walk_depth(doc, child_id, base_url, assets, depth + 1);
    }
}

/// Resolve a potentially relative URL against the page's base URL.
fn resolve(href: &str, base: &str) -> String {
    if href.starts_with("http://") || href.starts_with("https://") || href.starts_with("//") {
        return href.to_string();
    }
    if href.starts_with('/') {
        // Absolute path — prepend origin
        if let Ok(u) = url::Url::parse(base) {
            let origin = format!("{}://{}", u.scheme(), u.host_str().unwrap_or(""));
            return format!("{}{}", origin, href);
        }
    }
    // Relative — resolve against base
    if let Ok(base_url) = url::Url::parse(base) {
        if let Ok(resolved) = base_url.join(href) {
            return resolved.to_string();
        }
    }
    href.to_string()
}
