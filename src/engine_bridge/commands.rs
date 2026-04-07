use crate::engine_bridge::BridgeCommand;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct NavigateCommand {
    pub url: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ExecuteScriptCommand {
    pub script: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GetDocumentCommand;

impl From<NavigateCommand> for BridgeCommand {
    fn from(cmd: NavigateCommand) -> Self {
        BridgeCommand::Navigate { url: cmd.url }
    }
}

impl From<ExecuteScriptCommand> for BridgeCommand {
    fn from(cmd: ExecuteScriptCommand) -> Self {
        BridgeCommand::ExecuteScript { script: cmd.script }
    }
}
