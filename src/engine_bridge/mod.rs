use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub enum BridgeCommand {
    Navigate { url: String },
    ExecuteScript { script: String },
    GetDocument,
    GoBack,
    GoForward,
    Reload,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum BridgeEvent {
    Navigated {
        url: String,
        success: bool,
    },
    ScriptResult {
        result: String,
        error: Option<String>,
    },
    DocumentContent {
        content: String,
    },
    TitleChanged {
        title: String,
    },
    LoadStart {
        url: String,
    },
    LoadComplete {
        url: String,
    },
}

pub struct EngineBridge;

impl EngineBridge {
    pub fn new() -> Self {
        EngineBridge
    }
}

pub mod commands;
pub mod events;
