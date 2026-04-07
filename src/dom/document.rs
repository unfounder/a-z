use super::node::{Attribute, Node, NodeData, NodeId};

#[derive(Clone)]
pub struct Document {
    pub nodes: Vec<Node>,
    pub url: String,
    /// Recycled node slots available for reuse (free-list to prevent unbounded growth).
    free_list: Vec<NodeId>,
}

impl Document {
    pub fn new() -> Self {
        let root = Node::new(0, NodeData::Document);
        Document {
            nodes: vec![root],
            url: String::new(),
            free_list: Vec::new(),
        }
    }

    pub fn with_url(url: &str) -> Self {
        let mut doc = Document::new();
        doc.url = url.to_string();
        doc
    }

    pub fn root(&self) -> NodeId {
        0
    }

    pub fn get(&self, id: NodeId) -> Option<&Node> {
        self.nodes.get(id)
    }

    pub fn create_node(&mut self, data: NodeData) -> NodeId {
        if let Some(id) = self.free_list.pop() {
            self.nodes[id] = Node::new(id, data);
            id
        } else {
            let id = self.nodes.len();
            self.nodes.push(Node::new(id, data));
            id
        }
    }

    /// Release an entire subtree into the free-list so slots can be reused.
    /// Iterative to avoid stack overflow on deep trees.
    fn release_subtree(&mut self, root: NodeId) {
        if root == 0 {
            return;
        } // never free the document root
        let mut stack = vec![root];
        while let Some(id) = stack.pop() {
            if id == 0 {
                continue;
            }
            let children: Vec<NodeId> = self
                .nodes
                .get(id)
                .map(|n| n.children.clone())
                .unwrap_or_default();
            for child in children {
                stack.push(child);
            }
            if let Some(node) = self.nodes.get_mut(id) {
                node.parent = None;
                node.children.clear();
                node.data = NodeData::Text {
                    content: String::new(),
                };
            }
            self.free_list.push(id);
        }
    }

    pub fn append_child(&mut self, parent: NodeId, child: NodeId) {
        self.nodes[child].parent = Some(parent);
        self.nodes[parent].children.push(child);
    }

    pub fn remove_from_parent(&mut self, node: NodeId) {
        if let Some(parent_id) = self.nodes[node].parent {
            self.nodes[parent_id].children.retain(|&c| c != node);
            self.nodes[node].parent = None;
        }
    }

    /// Find first element with given tag name, depth-first
    pub fn find_element(&self, tag: &str) -> Option<NodeId> {
        let mut stack = vec![0usize];
        while let Some(id) = stack.pop() {
            if let Some(node) = self.nodes.get(id) {
                if node.tag_name().map(|t| t == tag).unwrap_or(false) {
                    return Some(id);
                }
                // Push children in reverse so we visit left-to-right
                for &child in node.children.iter().rev() {
                    stack.push(child);
                }
            }
        }
        None
    }

    /// Collect all inline <script> texts (no src attribute)
    pub fn collect_inline_scripts(&self) -> Vec<String> {
        let mut scripts = Vec::new();
        let mut stack = vec![0usize];
        while let Some(id) = stack.pop() {
            if let Some(node) = self.nodes.get(id) {
                match &node.data {
                    super::node::NodeData::Element {
                        tag_name, attrs, ..
                    } if tag_name == "script" => {
                        let has_src = attrs.iter().any(|a| a.name == "src");
                        if !has_src {
                            let text = node.text_content(&self.nodes);
                            let trimmed = text.trim().to_string();
                            if !trimmed.is_empty() {
                                scripts.push(trimmed);
                            }
                        }
                        // Don't recurse into script children
                    }
                    _ => {
                        for &child in node.children.iter().rev() {
                            stack.push(child);
                        }
                    }
                }
            }
        }
        scripts
    }

    /// Inject <base href="url"> as first child of <head>
    pub fn inject_base(&mut self, url: &str) {
        if let Some(head_id) = self.find_element("head") {
            // Check if base already exists
            let has_base = self.nodes[head_id]
                .children
                .iter()
                .any(|&c| self.nodes.get(c).and_then(|n| n.tag_name()) == Some("base"));
            if has_base {
                return;
            }

            let base_id = self.create_node(NodeData::Element {
                tag_name: "base".to_string(),
                namespace: "http://www.w3.org/1999/xhtml".to_string(),
                attrs: vec![Attribute {
                    name: "href".to_string(),
                    value: url.to_string(),
                }],
            });
            self.nodes[base_id].parent = Some(head_id);
            self.nodes[head_id].children.insert(0, base_id);
        }
    }

    /// Get element by id attribute
    pub fn get_element_by_id(&self, id: &str) -> Option<NodeId> {
        let mut stack = vec![0usize];
        while let Some(node_id) = stack.pop() {
            if let Some(node) = self.nodes.get(node_id) {
                if node.attr("id") == Some(id) {
                    return Some(node_id);
                }
                for &child in node.children.iter().rev() {
                    stack.push(child);
                }
            }
        }
        None
    }

    /// Get all elements with given tag name
    pub fn get_elements_by_tag(&self, tag: &str) -> Vec<NodeId> {
        let mut result = Vec::new();
        let mut stack = vec![0usize];
        while let Some(id) = stack.pop() {
            if let Some(node) = self.nodes.get(id) {
                if node.tag_name().map(|t| t == tag).unwrap_or(false) {
                    result.push(id);
                }
                for &child in node.children.iter().rev() {
                    stack.push(child);
                }
            }
        }
        result
    }

    // ── Attribute manipulation ─────────────────────────────────────────────
    pub fn get_attribute(&self, node_id: NodeId, name: &str) -> Option<&str> {
        self.nodes.get(node_id)?.attr(name)
    }

    pub fn set_attribute(&mut self, node_id: NodeId, name: &str, value: &str) {
        if let Some(node) = self.nodes.get_mut(node_id) {
            if let NodeData::Element { attrs, .. } = &mut node.data {
                if let Some(a) = attrs.iter_mut().find(|a| a.name == name) {
                    a.value = value.to_string();
                } else {
                    attrs.push(Attribute {
                        name: name.to_string(),
                        value: value.to_string(),
                    });
                }
            }
        }
    }

    pub fn remove_attribute(&mut self, node_id: NodeId, name: &str) {
        if let Some(node) = self.nodes.get_mut(node_id) {
            if let NodeData::Element { attrs, .. } = &mut node.data {
                attrs.retain(|a| a.name != name);
            }
        }
    }

    // ── Text content ───────────────────────────────────────────────────────
    pub fn get_text_content(&self, node_id: NodeId) -> String {
        self.nodes
            .get(node_id)
            .map(|n| n.text_content(&self.nodes))
            .unwrap_or_default()
    }

    pub fn set_text_content(&mut self, node_id: NodeId, text: &str) {
        let old_children: Vec<NodeId> = self
            .nodes
            .get(node_id)
            .map(|n| n.children.clone())
            .unwrap_or_default();
        // Clear children list before releasing so release_subtree doesn't see stale parent links
        if let Some(node) = self.nodes.get_mut(node_id) {
            node.children.clear();
        }
        for child in old_children {
            self.release_subtree(child);
        }
        let text_id = self.create_node(NodeData::Text {
            content: text.to_string(),
        });
        self.append_child(node_id, text_id);
    }

    // ── Sibling navigation ─────────────────────────────────────────────────
    pub fn get_next_sibling(&self, node_id: NodeId) -> Option<NodeId> {
        let parent_id = self.nodes.get(node_id)?.parent?;
        let siblings = &self.nodes[parent_id].children;
        let pos = siblings.iter().position(|&c| c == node_id)?;
        siblings.get(pos + 1).copied()
    }

    pub fn get_prev_sibling(&self, node_id: NodeId) -> Option<NodeId> {
        let parent_id = self.nodes.get(node_id)?.parent?;
        let siblings = &self.nodes[parent_id].children;
        let pos = siblings.iter().position(|&c| c == node_id)?;
        if pos == 0 {
            None
        } else {
            siblings.get(pos - 1).copied()
        }
    }

    // ── Node creation helpers ──────────────────────────────────────────────
    pub fn create_element_node(&mut self, tag: &str) -> NodeId {
        self.create_node(NodeData::Element {
            tag_name: tag.to_ascii_lowercase(),
            namespace: "http://www.w3.org/1999/xhtml".to_string(),
            attrs: Vec::new(),
        })
    }

    pub fn create_text_node(&mut self, text: &str) -> NodeId {
        self.create_node(NodeData::Text {
            content: text.to_string(),
        })
    }

    // ── Deep clone from another document ──────────────────────────────────
    pub fn deep_clone_from(&mut self, src: &Document, src_node_id: NodeId) -> NodeId {
        self.deep_clone_inner(src, src_node_id, 0)
    }

    fn deep_clone_inner(&mut self, src: &Document, src_node_id: NodeId, depth: u32) -> NodeId {
        // Hard cap: maliciously crafted HTML with thousands of nested tags would
        // cause a stack overflow without this guard.
        if depth > 512 {
            return self.create_node(NodeData::Text {
                content: String::new(),
            });
        }
        let data = match src.get(src_node_id) {
            Some(node) => node.data.clone(),
            None => {
                return self.create_node(NodeData::Text {
                    content: String::new(),
                })
            }
        };
        let new_id = self.create_node(data);
        let children: Vec<NodeId> = src
            .nodes
            .get(src_node_id)
            .map(|n| n.children.clone())
            .unwrap_or_default();
        for child_src_id in children {
            let child_new_id = self.deep_clone_inner(src, child_src_id, depth + 1);
            self.append_child(new_id, child_new_id);
        }
        new_id
    }

    /// Replace all children of node_id with deep-cloned nodes from src_doc
    pub fn replace_inner_html(
        &mut self,
        node_id: NodeId,
        src_doc: &Document,
        src_children: &[NodeId],
    ) {
        let old: Vec<NodeId> = self
            .nodes
            .get(node_id)
            .map(|n| n.children.clone())
            .unwrap_or_default();
        // Clear children list first so release_subtree doesn't see stale parent links
        if let Some(node) = self.nodes.get_mut(node_id) {
            node.children.clear();
        }
        for child in old {
            self.release_subtree(child);
        }
        for &src_child in src_children {
            let new_id = self.deep_clone_from(src_doc, src_child);
            self.append_child(node_id, new_id);
        }
    }

    /// Get tag name of a node (empty string for non-elements)
    pub fn get_tag_name(&self, node_id: NodeId) -> &str {
        self.nodes
            .get(node_id)
            .and_then(|n| n.tag_name())
            .unwrap_or("")
    }

    /// Title text (reads/writes <title> element text content)
    pub fn get_title(&self) -> String {
        self.find_element("title")
            .map(|id| self.get_text_content(id))
            .unwrap_or_default()
    }

    pub fn set_title(&mut self, title: &str) {
        if let Some(title_id) = self.find_element("title") {
            self.set_text_content(title_id, title);
        } else if let Some(head_id) = self.find_element("head") {
            let title_el = self.create_element_node("title");
            let text_id = self.create_text_node(title);
            self.append_child(title_el, text_id);
            self.append_child(head_id, title_el);
        }
    }
}

impl Default for Document {
    fn default() -> Self {
        Document::new()
    }
}
