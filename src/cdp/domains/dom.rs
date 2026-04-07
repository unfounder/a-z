use crate::dom::document::Document;
use crate::dom::node::{NodeData, NodeId};
use serde_json::{json, Value};

pub struct DomConverter;

impl DomConverter {
    /// Convert a node in our arena to a CDP Node object.
    /// `depth` controls how many levels of children to include (-1 = unlimited).
    pub fn node_to_cdp(doc: &Document, node_id: NodeId, depth: i32) -> Value {
        let node = match doc.get(node_id) {
            Some(n) => n,
            None => return json!(null),
        };

        let (node_type, node_name, local_name, node_value) = match &node.data {
            NodeData::Document => (
                9u32,
                "#document".to_string(),
                "".to_string(),
                "".to_string(),
            ),
            NodeData::Doctype { name, .. } => {
                (10, name.to_string(), "".to_string(), "".to_string())
            }
            NodeData::Element { tag_name, .. } => (
                1,
                tag_name.to_uppercase(),
                tag_name.to_lowercase(),
                "".to_string(),
            ),
            NodeData::Text { content } => (3, "#text".to_string(), "".to_string(), content.clone()),
            NodeData::Comment { content } => {
                (8, "#comment".to_string(), "".to_string(), content.clone())
            }
            NodeData::ProcessingInstruction { target, data } => (
                7,
                target.to_uppercase(),
                target.to_lowercase(),
                data.clone(),
            ),
        };

        let child_count = node.children.len();
        let children_ids: Vec<NodeId> = node.children.clone();

        let mut obj = json!({
            "nodeId":        node_id,
            "backendNodeId": node_id,
            "nodeType":      node_type,
            "nodeName":      node_name,
            "localName":     local_name,
            "nodeValue":     node_value,
            "childNodeCount": child_count
        });

        // Flat [name, value, name, value...] attribute list for elements
        if node_type == 1 {
            if let NodeData::Element { attrs, .. } = &node.data {
                let flat: Vec<String> = attrs
                    .iter()
                    .flat_map(|a| vec![a.name.clone(), a.value.clone()])
                    .collect();
                obj["attributes"] = json!(flat);
            }
        }

        // Children (recursively) when depth allows
        if (depth > 0 || depth == -1) && !children_ids.is_empty() {
            let next_depth = if depth == -1 { -1 } else { depth - 1 };
            let children: Vec<Value> = children_ids
                .iter()
                .map(|&cid| Self::node_to_cdp(doc, cid, next_depth))
                .collect();
            obj["children"] = json!(children);
        }

        obj
    }
}
