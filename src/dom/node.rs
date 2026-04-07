pub type NodeId = usize;

#[derive(Debug, Clone)]
pub struct Attribute {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone)]
pub enum NodeData {
    Document,
    Doctype {
        name: String,
        public_id: String,
        system_id: String,
    },
    Element {
        tag_name: String, // always lowercase
        namespace: String,
        attrs: Vec<Attribute>,
    },
    Text {
        content: String,
    },
    Comment {
        content: String,
    },
    ProcessingInstruction {
        target: String,
        data: String,
    },
}

#[derive(Debug, Clone)]
pub struct Node {
    pub id: NodeId,
    pub data: NodeData,
    pub parent: Option<NodeId>,
    pub children: Vec<NodeId>,
}

impl Node {
    pub fn new(id: NodeId, data: NodeData) -> Self {
        Node {
            id,
            data,
            parent: None,
            children: Vec::new(),
        }
    }

    pub fn is_element(&self) -> bool {
        matches!(self.data, NodeData::Element { .. })
    }

    pub fn tag_name(&self) -> Option<&str> {
        match &self.data {
            NodeData::Element { tag_name, .. } => Some(tag_name),
            _ => None,
        }
    }

    pub fn attr(&self, name: &str) -> Option<&str> {
        match &self.data {
            NodeData::Element { attrs, .. } => attrs
                .iter()
                .find(|a| a.name == name)
                .map(|a| a.value.as_str()),
            _ => None,
        }
    }

    pub fn text_content<'a>(&self, arena: &'a [Node]) -> String {
        match &self.data {
            NodeData::Text { content } => content.clone(),
            _ => self
                .children
                .iter()
                .filter_map(|&id| arena.get(id))
                .map(|n| n.text_content(arena))
                .collect::<Vec<_>>()
                .join(""),
        }
    }
}
