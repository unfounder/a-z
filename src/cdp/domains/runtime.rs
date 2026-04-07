use crate::error::BrowserResult;
use serde_json::{json, Value};

pub struct RuntimeDomain;

impl RuntimeDomain {
    pub fn enable() -> BrowserResult<Value> {
        Ok(json!({}))
    }

    pub fn disable() -> BrowserResult<Value> {
        Ok(json!({}))
    }

    pub fn evaluate(_expression: &str) -> BrowserResult<Value> {
        Ok(json!({
            "result": {
                "type": "string",
                "value": "evaluation result"
            }
        }))
    }
}
