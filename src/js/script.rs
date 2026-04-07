use super::context::ScriptContext;
use crate::error::BrowserResult;

pub struct ScriptExecutor {
    context: ScriptContext,
}

impl ScriptExecutor {
    pub fn new(context: ScriptContext) -> Self {
        Self { context }
    }

    pub async fn execute(&self, script: &str) -> BrowserResult<String> {
        self.context.execute(script).await
    }
}
