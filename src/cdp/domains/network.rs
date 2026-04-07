use crate::error::BrowserResult;
use serde_json::{json, Value};

pub struct NetworkDomain;

impl NetworkDomain {
    pub fn enable() -> BrowserResult<Value> {
        Ok(json!({}))
    }

    pub fn disable() -> BrowserResult<Value> {
        Ok(json!({}))
    }

    pub fn set_user_agent(_user_agent: &str) -> BrowserResult<Value> {
        Ok(json!({}))
    }
}
