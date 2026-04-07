use super::document::Document;
use super::node::{Attribute, NodeData, NodeId};
use crate::error::BrowserResult;
use html5ever::tendril::TendrilSink;
use html5ever::{parse_document, ParseOpts};
use markup5ever_rcdom::{Handle, NodeData as RcNodeData, RcDom};

pub struct HtmlParser;

impl HtmlParser {
    pub fn new() -> Self {
        HtmlParser
    }

    pub fn parse(&self, html: &str, url: &str) -> BrowserResult<Document> {
        let rcdom = parse_document(RcDom::default(), ParseOpts::default()).one(html.to_string());

        let mut doc = Document::with_url(url);
        walk_rc_node(&rcdom.document, &mut doc, 0);
        doc.inject_base(url);
        Ok(doc)
    }

    /// String-level base injection fallback (used for quick HTML passthrough)
    pub fn inject_base(&self, html: &str, base_url: &str) -> String {
        let base_tag = format!("<base href=\"{}\">", base_url);
        let lower = html.to_lowercase();
        if let Some(pos) = lower.find("<head>") {
            let at = pos + "<head>".len();
            format!("{}{}{}", &html[..at], base_tag, &html[at..])
        } else if let Some(pos) = lower.find("<html") {
            if let Some(end) = lower[pos..].find('>') {
                let at = pos + end + 1;
                format!("{}<head>{}</head>{}", &html[..at], base_tag, &html[at..])
            } else {
                format!("<head>{}</head>{}", base_tag, html)
            }
        } else {
            format!("<head>{}</head>{}", base_tag, html)
        }
    }
}

/// Maximum DOM nesting depth we will walk.
/// Cloudflare / obfuscated pages inject hundreds of nested wrappers to cause
/// stack overflows in recursive parsers. We stop descending beyond this point.
const MAX_WALK_DEPTH: usize = 512;

fn walk_rc_node(handle: &Handle, doc: &mut Document, parent_id: NodeId) {
    walk_rc_node_depth(handle, doc, parent_id, 0);
}

fn walk_rc_node_depth(handle: &Handle, doc: &mut Document, parent_id: NodeId, depth: usize) {
    if depth > MAX_WALK_DEPTH {
        return; // silently truncate — better than stack overflow
    }

    // Collect children first to avoid borrow issues
    let children: Vec<Handle> = handle.children.borrow().clone();

    for child in &children {
        let node_id = match &child.data {
            RcNodeData::Document => {
                // Nested document — recurse in-place at same depth
                walk_rc_node_depth(child, doc, parent_id, depth);
                continue;
            }
            RcNodeData::Doctype {
                name,
                public_id,
                system_id,
            } => doc.create_node(NodeData::Doctype {
                name: name.to_string(),
                public_id: public_id.to_string(),
                system_id: system_id.to_string(),
            }),
            RcNodeData::Text { contents } => {
                let text = contents.borrow().to_string();
                doc.create_node(NodeData::Text { content: text })
            }
            RcNodeData::Comment { contents } => doc.create_node(NodeData::Comment {
                content: contents.to_string(),
            }),
            RcNodeData::Element { name, attrs, .. } => {
                let tag_name = name.local.to_string().to_ascii_lowercase();
                let namespace = name.ns.to_string();
                let our_attrs: Vec<Attribute> = attrs
                    .borrow()
                    .iter()
                    .map(|a| Attribute {
                        name: a.name.local.to_string(),
                        value: a.value.to_string(),
                    })
                    .collect();
                doc.create_node(NodeData::Element {
                    tag_name,
                    namespace,
                    attrs: our_attrs,
                })
            }
            RcNodeData::ProcessingInstruction { target, contents } => {
                doc.create_node(NodeData::ProcessingInstruction {
                    target: target.to_string(),
                    data: contents.to_string(),
                })
            }
        };

        doc.append_child(parent_id, node_id);
        walk_rc_node_depth(child, doc, node_id, depth + 1);
    }
}
