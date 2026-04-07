pub mod cache;
pub mod client;
pub mod cookies;
pub mod headers;

use std::time::Duration;
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);
