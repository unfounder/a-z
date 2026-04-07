use thiserror::Error;

#[derive(Debug, Error)]
pub enum BrowserError {
    #[error("HTTP error: {0}")]
    Http(String),

    #[error("URL parse error: {0}")]
    UrlParse(#[from] url::ParseError),

    #[error("Parse error: {0}")]
    Parse(String),

    #[error("JavaScript error: {0}")]
    JavaScript(String),

    #[error("CDP error: {0}")]
    Cdp(String),

    #[error("Navigation error: {0}")]
    Navigation(String),

    #[error("Timeout after {0}ms")]
    Timeout(u64),

    #[error("Not found: {0}")]
    NotFound(String),

    #[error("Engine bridge error: {0}")]
    Bridge(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

pub type BrowserResult<T> = Result<T, BrowserError>;

impl From<rquickjs::Error> for BrowserError {
    fn from(e: rquickjs::Error) -> Self {
        BrowserError::JavaScript(e.to_string())
    }
}
