use super::document::Document;
use super::node::NodeId;
use crate::error::{BrowserError, BrowserResult};

pub struct DomQuery;

impl DomQuery {
    pub fn query_selector(doc: &Document, selector: &str) -> BrowserResult<Option<NodeId>> {
        let parsed = parse_selector(selector.trim())?;
        let mut stack = vec![doc.root()];
        while let Some(id) = stack.pop() {
            if let Some(node) = doc.get(id) {
                if id != doc.root() && matches_selector(node, &parsed) {
                    return Ok(Some(id));
                }
                for &child in node.children.iter().rev() {
                    stack.push(child);
                }
            }
        }
        Ok(None)
    }

    pub fn query_selector_all(doc: &Document, selector: &str) -> BrowserResult<Vec<NodeId>> {
        let parsed = parse_selector(selector.trim())?;
        let mut result = Vec::new();
        let mut stack = vec![doc.root()];
        while let Some(id) = stack.pop() {
            if let Some(node) = doc.get(id) {
                if id != doc.root() && matches_selector(node, &parsed) {
                    result.push(id);
                }
                for &child in node.children.iter().rev() {
                    stack.push(child);
                }
            }
        }
        Ok(result)
    }

    /// querySelector starting from a specific parent node (not always root)
    pub fn query_selector_from(
        doc: &Document,
        parent_id: NodeId,
        selector: &str,
    ) -> BrowserResult<Option<NodeId>> {
        let parsed = parse_selector(selector.trim())?;
        let mut stack: Vec<NodeId> = doc
            .get(parent_id)
            .map(|n| n.children.iter().rev().copied().collect())
            .unwrap_or_default();
        while let Some(id) = stack.pop() {
            if let Some(node) = doc.get(id) {
                if matches_selector(node, &parsed) {
                    return Ok(Some(id));
                }
                for &child in node.children.iter().rev() {
                    stack.push(child);
                }
            }
        }
        Ok(None)
    }

    /// querySelectorAll starting from a specific parent node
    pub fn query_selector_all_from(
        doc: &Document,
        parent_id: NodeId,
        selector: &str,
    ) -> BrowserResult<Vec<NodeId>> {
        let parsed = parse_selector(selector.trim())?;
        let mut result = Vec::new();
        let mut stack: Vec<NodeId> = doc
            .get(parent_id)
            .map(|n| n.children.iter().rev().copied().collect())
            .unwrap_or_default();
        while let Some(id) = stack.pop() {
            if let Some(node) = doc.get(id) {
                if matches_selector(node, &parsed) {
                    result.push(id);
                }
                for &child in node.children.iter().rev() {
                    stack.push(child);
                }
            }
        }
        Ok(result)
    }
}

#[derive(Debug)]
struct SimpleSelector {
    tag: Option<String>,
    id: Option<String>,
    classes: Vec<String>,
}

fn parse_selector(s: &str) -> BrowserResult<SimpleSelector> {
    let mut sel = SimpleSelector {
        tag: None,
        id: None,
        classes: Vec::new(),
    };
    let mut current = String::new();
    let mut mode = 't'; // 't'=tag, '#'=id, '.'=class

    for ch in s.chars() {
        match ch {
            '#' => {
                flush(&mut sel, mode, &current);
                current.clear();
                mode = '#';
            }
            '.' => {
                flush(&mut sel, mode, &current);
                current.clear();
                mode = '.';
            }
            ' ' | '>' | '+' | '~' => {
                // Combinators not supported — treat whole string as error if complex
                return Err(BrowserError::Parse(format!(
                    "Complex selector not supported: {}",
                    s
                )));
            }
            c => current.push(c),
        }
    }
    flush(&mut sel, mode, &current);
    Ok(sel)
}

fn flush(sel: &mut SimpleSelector, mode: char, token: &str) {
    if token.is_empty() {
        return;
    }
    match mode {
        't' => sel.tag = Some(token.to_ascii_lowercase()),
        '#' => sel.id = Some(token.to_string()),
        '.' => sel.classes.push(token.to_string()),
        _ => {}
    }
}

fn matches_selector(node: &super::node::Node, sel: &SimpleSelector) -> bool {
    if !node.is_element() {
        return false;
    }

    if let Some(tag) = &sel.tag {
        if node.tag_name() != Some(tag.as_str()) {
            return false;
        }
    }

    if let Some(id) = &sel.id {
        if node.attr("id") != Some(id.as_str()) {
            return false;
        }
    }

    for class in &sel.classes {
        let has_class = node
            .attr("class")
            .map(|c| c.split_whitespace().any(|t| t == class))
            .unwrap_or(false);
        if !has_class {
            return false;
        }
    }

    true
}
