use crate::engine_bridge::BridgeEvent;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct NavigatedEvent {
    pub url: String,
    pub success: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ScriptResultEvent {
    pub result: String,
    pub error: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DocumentContentEvent {
    pub content: String,
}

impl From<NavigatedEvent> for BridgeEvent {
    fn from(e: NavigatedEvent) -> Self {
        BridgeEvent::Navigated {
            url: e.url,
            success: e.success,
        }
    }
}

impl From<DocumentContentEvent> for BridgeEvent {
    fn from(e: DocumentContentEvent) -> Self {
        BridgeEvent::DocumentContent { content: e.content }
    }
}
