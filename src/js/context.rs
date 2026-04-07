use super::runtime::JsContext;
use crate::error::BrowserResult;
use std::sync::Arc;

pub struct ScriptContext {
    js_context: Arc<JsContext>,
}

impl ScriptContext {
    pub fn new(js_context: Arc<JsContext>) -> Self {
        Self { js_context }
    }

    pub async fn execute(&self, script: &str) -> BrowserResult<String> {
        self.js_context.execute_script(script).await
    }
}
