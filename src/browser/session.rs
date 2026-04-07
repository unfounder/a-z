use crate::browser::Browser;
use crate::error::BrowserResult;
use std::sync::Arc;

pub struct Page {
    pub url: String,
    pub title: String,
}

pub struct Session {
    pub id: String,
    browser: Arc<Browser>,
    history: Vec<String>,
    cursor: usize,
}

impl Session {
    pub fn new(browser: Arc<Browser>) -> Self {
        Session {
            id: uuid::Uuid::new_v4().to_string(),
            browser,
            history: Vec::new(),
            cursor: 0,
        }
    }

    pub async fn navigate(&mut self, url: &str) -> BrowserResult<String> {
        let result = self.browser.navigate(url).await?;
        // Truncate forward history
        self.history.truncate(self.cursor);
        self.history.push(url.to_string());
        self.cursor = self.history.len();
        Ok(result)
    }

    pub fn current_url(&self) -> Option<&str> {
        if self.cursor > 0 {
            self.history.get(self.cursor - 1).map(|s| s.as_str())
        } else {
            None
        }
    }

    pub fn can_go_back(&self) -> bool {
        self.cursor > 1
    }
    pub fn can_go_forward(&self) -> bool {
        self.cursor < self.history.len()
    }

    pub async fn back(&mut self) -> Option<BrowserResult<String>> {
        if self.can_go_back() {
            self.cursor -= 1;
            let url = self.history[self.cursor - 1].clone();
            Some(self.browser.navigate(&url).await)
        } else {
            None
        }
    }

    pub async fn forward(&mut self) -> Option<BrowserResult<String>> {
        if self.can_go_forward() {
            self.cursor += 1;
            let url = self.history[self.cursor - 1].clone();
            Some(self.browser.navigate(&url).await)
        } else {
            None
        }
    }
}
