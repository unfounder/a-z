use crate::error::BrowserResult;
use serde_json::{json, Value};

pub struct PageDomain;

impl PageDomain {
    pub fn enable() -> BrowserResult<Value> {
        Ok(json!({}))
    }

    pub fn disable() -> BrowserResult<Value> {
        Ok(json!({}))
    }

    pub fn navigate(_url: &str) -> BrowserResult<Value> {
        Ok(json!({
            "frameId": "main",
            "loaderId": "loader1"
        }))
    }
}
