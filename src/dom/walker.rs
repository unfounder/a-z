/// Lynx-style DOM walker shared between the terminal UI and the canvas GUI.
///
/// Produces `RichLine` content (styled text with link indices) from a document
/// node tree. The same types are used by both frontends so page content only
/// needs to be computed once after each navigation.
use super::{Node, NodeData};

// ── Output types ──────────────────────────────────────────────────────────────

/// A single span of styled text in the rendered view.
#[derive(Debug, Clone)]
pub struct RichSpan {
    pub text: String,
    pub link_idx: Option<usize>,
    pub heading: bool,
}

/// A rendered line is a sequence of styled spans.
pub type RichLine = Vec<RichSpan>;

/// A hyperlink discovered during the walk.
#[derive(Debug, Clone)]
pub struct Link {
    pub text: String,
    pub href: String,
}

/// Everything the walker produces from one document traversal.
pub struct WalkResult {
    pub lines: Vec<RichLine>,
    pub links: Vec<Link>,
    pub title: String,
    pub script_count: usize,
}

// ── Public entry point ────────────────────────────────────────────────────────

/// Walk the document starting at `start_id` and produce rendered lines.
///
/// Returns [`WalkResult`] containing the rendered content, the link table,
/// the page title (from `<title>`), and the number of inline scripts found.
pub fn walk_document(nodes: &[Node], start_id: usize, base_url: &str) -> WalkResult {
    let mut ctx = WalkCtx {
        nodes,
        base_url,
        links: Vec::new(),
        scripts: 0,
        lines: Vec::new(),
        current: Vec::new(),
        in_heading: false,
        in_a: None,
    };
    ctx.walk(start_id);
    ctx.flush_line();

    // Extract title from <title> element
    let title = nodes
        .iter()
        .find(|n| n.tag_name() == Some("title"))
        .map(|n| collect_text(nodes, &n.children))
        .unwrap_or_default();
    let title = title.trim().to_string();
    let title = if title.is_empty() {
        base_url.to_string()
    } else {
        title
    };

    // Deduplicate links by href (keep first occurrence)
    let mut seen = std::collections::HashSet::new();
    ctx.links.retain(|l| seen.insert(l.href.clone()));

    // Append a lynx-style References section at the bottom
    if !ctx.links.is_empty() {
        ctx.lines.push(vec![]);
        ctx.lines.push(vec![RichSpan {
            text: "References".to_string(),
            link_idx: None,
            heading: true,
        }]);
        ctx.lines.push(vec![]);
        for (i, link) in ctx.links.iter().enumerate() {
            ctx.lines.push(vec![
                RichSpan {
                    text: format!("   [{}] ", i + 1),
                    link_idx: None,
                    heading: false,
                },
                RichSpan {
                    text: link.href.clone(),
                    link_idx: Some(i),
                    heading: false,
                },
            ]);
        }
    }

    WalkResult {
        lines: ctx.lines,
        links: ctx.links,
        title,
        script_count: ctx.scripts,
    }
}

// ── Walker internals ──────────────────────────────────────────────────────────

struct WalkCtx<'a> {
    nodes: &'a [Node],
    base_url: &'a str,
    links: Vec<Link>,
    scripts: usize,
    lines: Vec<RichLine>,
    current: RichLine,
    in_heading: bool,
    in_a: Option<usize>,
}

impl<'a> WalkCtx<'a> {
    fn push_text(&mut self, text: &str) {
        let t = text.replace(['\n', '\r', '\t'], " ");
        let mut out = String::new();
        let mut prev_space = true; // trim leading whitespace
        for ch in t.chars() {
            if ch == ' ' {
                if !prev_space {
                    out.push(' ');
                }
                prev_space = true;
            } else {
                out.push(ch);
                prev_space = false;
            }
        }
        if out.is_empty() {
            return;
        }
        self.current.push(RichSpan {
            text: out,
            link_idx: self.in_a,
            heading: self.in_heading,
        });
    }

    fn flush_line(&mut self) {
        let has_content = self.current.iter().any(|s| !s.text.trim().is_empty());
        if has_content {
            self.lines.push(std::mem::take(&mut self.current));
        } else {
            self.current.clear();
        }
    }

    fn blank_line(&mut self) {
        self.flush_line();
        if self
            .lines
            .last()
            .map(|l: &RichLine| !l.is_empty())
            .unwrap_or(false)
        {
            self.lines.push(vec![]);
        }
    }

    fn walk(&mut self, id: usize) {
        let node = match self.nodes.get(id) {
            Some(n) => n.clone(),
            None => return,
        };

        match &node.data {
            NodeData::Element { tag_name, .. } => {
                let tag = tag_name.as_str();

                // Skip these entirely
                if matches!(
                    tag,
                    "script"
                        | "style"
                        | "noscript"
                        | "svg"
                        | "head"
                        | "canvas"
                        | "iframe"
                        | "meta"
                        | "link"
                        | "template"
                ) {
                    if tag == "script" {
                        self.scripts += 1;
                    }
                    return;
                }

                // Headings
                if matches!(tag, "h1" | "h2" | "h3" | "h4" | "h5" | "h6") {
                    self.blank_line();
                    self.in_heading = true;
                    for &child in &node.children {
                        self.walk(child);
                    }
                    self.in_heading = false;
                    self.flush_line();
                    self.lines.push(vec![]);
                    return;
                }

                // Anchor
                if tag == "a" {
                    if let Some(href) = node.attr("href") {
                        let href = href.to_string();
                        if !href.is_empty()
                            && !href.starts_with('#')
                            && !href.starts_with("javascript")
                        {
                            let full = resolve_url(self.base_url, &href);
                            let link_text = collect_text(self.nodes, &node.children)
                                .split_whitespace()
                                .collect::<Vec<_>>()
                                .join(" ");
                            let link_text = if link_text.is_empty() {
                                href.clone()
                            } else {
                                link_text
                            };

                            let idx = self.links.len();
                            self.links.push(Link {
                                text: link_text,
                                href: full,
                            });

                            let saved_a = self.in_a;
                            self.in_a = Some(idx);
                            self.current.push(RichSpan {
                                text: format!("[{}]", idx + 1),
                                link_idx: Some(idx),
                                heading: false,
                            });
                            for &child in &node.children {
                                self.walk(child);
                            }
                            self.in_a = saved_a;
                            return;
                        }
                    }
                    for &child in &node.children {
                        self.walk(child);
                    }
                    return;
                }

                // Image → [alt text]
                if tag == "img" {
                    let alt = node.attr("alt").unwrap_or("image");
                    let alt = alt.trim();
                    if !alt.is_empty() {
                        self.current.push(RichSpan {
                            text: format!("[{}]", alt),
                            link_idx: None,
                            heading: false,
                        });
                    }
                    return;
                }

                // Block-level elements
                let is_block = matches!(
                    tag,
                    "p" | "div"
                        | "section"
                        | "article"
                        | "main"
                        | "aside"
                        | "header"
                        | "footer"
                        | "nav"
                        | "ul"
                        | "ol"
                        | "dl"
                        | "li"
                        | "dt"
                        | "dd"
                        | "blockquote"
                        | "pre"
                        | "figure"
                        | "form"
                        | "fieldset"
                        | "table"
                        | "thead"
                        | "tbody"
                        | "tfoot"
                        | "tr"
                        | "th"
                        | "td"
                        | "br"
                        | "hr"
                );

                if is_block {
                    if tag == "br" || tag == "hr" {
                        self.flush_line();
                        return;
                    }
                    self.flush_line();
                    for &child in &node.children {
                        self.walk(child);
                    }
                    self.flush_line();
                } else {
                    for &child in &node.children {
                        self.walk(child);
                    }
                }
            }

            NodeData::Text { content } => {
                self.push_text(content);
            }

            _ => {
                for &child in &node.children {
                    self.walk(child);
                }
            }
        }
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Recursively collect plain text from a list of child node IDs.
pub fn collect_text(nodes: &[Node], children: &[usize]) -> String {
    let mut out = String::new();
    for &id in children {
        let Some(node) = nodes.get(id) else { continue };
        match &node.data {
            NodeData::Text { content } => {
                out.push_str(content);
                out.push(' ');
            }
            NodeData::Element { tag_name, .. } => {
                if !matches!(tag_name.as_str(), "script" | "style") {
                    out.push_str(&collect_text(nodes, &node.children));
                }
            }
            _ => {
                out.push_str(&collect_text(nodes, &node.children));
            }
        }
    }
    out
}

/// Resolve a possibly-relative href against a base URL.
pub fn resolve_url(base: &str, href: &str) -> String {
    if href.starts_with("http://") || href.starts_with("https://") {
        return href.to_string();
    }
    if href.starts_with("//") {
        return format!("https:{}", href);
    }
    if let Ok(base_url) = url::Url::parse(base) {
        if let Ok(resolved) = base_url.join(href) {
            return resolved.to_string();
        }
    }
    href.to_string()
}
