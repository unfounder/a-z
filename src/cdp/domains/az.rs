use crate::error::BrowserResult;
use serde_json::{json, Value};

pub struct AzDomain;

impl AzDomain {
    pub fn enable() -> BrowserResult<Value> {
        Ok(json!({}))
    }

    pub fn get_state() -> BrowserResult<Value> {
        Ok(json!({
            "state": "ready",
            "version": "0.1.0"
        }))
    }
}
