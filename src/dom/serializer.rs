use super::document::Document;
use super::node::{NodeData, NodeId};
use crate::error::BrowserResult;

// HTML5 void elements — no closing tag
const VOID_ELEMENTS: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source",
    "track", "wbr",
];

// Raw text elements — content is not HTML-escaped
const RAW_TEXT_ELEMENTS: &[&str] = &["script", "style"];

pub struct DomSerializer;

impl DomSerializer {
    pub fn serialize(doc: &Document) -> BrowserResult<String> {
        let mut buf = String::with_capacity(8192);
        write_node(doc, doc.root(), &mut buf);
        Ok(buf)
    }

    /// Serialize the *children* of a node (innerHTML)
    pub fn serialize_children(doc: &Document, node_id: NodeId) -> BrowserResult<String> {
        let mut buf = String::with_capacity(512);
        if let Some(node) = doc.get(node_id) {
            let children = node.children.clone();
            for child_id in children {
                write_node(doc, child_id, &mut buf);
            }
        }
        Ok(buf)
    }

    /// Serialize a single node including its tag (outerHTML)
    pub fn serialize_node(doc: &Document, node_id: NodeId) -> BrowserResult<String> {
        let mut buf = String::with_capacity(512);
        write_node(doc, node_id, &mut buf);
        Ok(buf)
    }
}

pub(crate) fn write_node(doc: &Document, id: NodeId, buf: &mut String) {
    let node = match doc.get(id) {
        Some(n) => n,
        None => return,
    };

    match &node.data {
        NodeData::Document => {
            let children: Vec<NodeId> = node.children.clone();
            for child_id in children {
                write_node(doc, child_id, buf);
            }
        }
        NodeData::Doctype { name, .. } => {
            buf.push_str("<!DOCTYPE ");
            buf.push_str(name);
            buf.push('>');
        }
        NodeData::Text { content } => {
            // Check if parent is a raw text element
            let in_raw = node
                .parent
                .and_then(|p| doc.get(p))
                .and_then(|p| p.tag_name())
                .map(|t| RAW_TEXT_ELEMENTS.contains(&t))
                .unwrap_or(false);

            if in_raw {
                buf.push_str(content);
            } else {
                buf.push_str(&escape_text(content));
            }
        }
        NodeData::Comment { content } => {
            buf.push_str("<!--");
            buf.push_str(content);
            buf.push_str("-->");
        }
        NodeData::ProcessingInstruction { target, data } => {
            buf.push_str("<?");
            buf.push_str(target);
            buf.push(' ');
            buf.push_str(data);
            buf.push_str("?>");
        }
        NodeData::Element {
            tag_name, attrs, ..
        } => {
            buf.push('<');
            buf.push_str(tag_name);
            for attr in attrs {
                buf.push(' ');
                buf.push_str(&attr.name);
                buf.push_str("=\"");
                buf.push_str(&escape_attr(&attr.value));
                buf.push('"');
            }

            if VOID_ELEMENTS.contains(&tag_name.as_str()) {
                buf.push('>');
                return; // no children, no closing tag
            }

            buf.push('>');

            let children: Vec<NodeId> = node.children.clone();
            for child_id in children {
                write_node(doc, child_id, buf);
            }

            buf.push_str("</");
            buf.push_str(tag_name);
            buf.push('>');
        }
    }
}

fn escape_text(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn escape_attr(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
}
