// ── App state ─────────────────────────────────────────────────────────────────
pub use a_z::browser::video::VideoInfo;
pub use a_z::dom::forms::Form;
// Shared DOM walker types — used by both TUI and GUI frontends
pub use a_z::dom::walker::{Link, RichLine};

#[derive(Debug, Clone, PartialEq)]
pub enum AppMode {
    NewTab,
    Loading,
    Page,
    Input,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ViewMode {
    Terminal,
    PageView,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct PageContent {
    pub url: String,
    pub title: String,
    pub html: String,
    pub links: Vec<Link>,
    pub scripts: Vec<String>,
    pub load_time_ms: u64,
    pub status: u16,
    pub videos: Vec<VideoInfo>,
    pub forms: Vec<Form>,
    /// Inline-rendered lines, ready for the terminal renderer.
    pub rendered_lines: Vec<RichLine>,
}

#[allow(dead_code)]
pub struct App {
    pub mode: AppMode,
    pub view_mode: ViewMode,
    pub url_input: String,
    pub search_input: String,
    pub current_page: Option<PageContent>,
    pub scroll_offset: u16,
    pub selected_link: Option<usize>,
    pub links: Vec<Link>,
    pub status_text: String,
    pub complexity: u8,
    pub history: Vec<String>,
    pub history_idx: usize,
    pub bookmarks: Vec<(&'static str, &'static str)>,
    pub selected_bm: usize,
    pub should_quit: bool,
    pub bitfox_status: String,
    pub dom_nodes: u32,
    pub script_count: u32,
    pub loading_url: Option<String>,
}

impl App {
    pub fn new() -> Self {
        Self {
            mode: AppMode::NewTab,
            view_mode: ViewMode::Terminal,
            url_input: String::new(),
            search_input: String::new(),
            current_page: None,
            scroll_offset: 0,
            selected_link: None,
            links: Vec::new(),
            status_text: "Ready — type to search, Enter to go".to_string(),
            complexity: 0,
            history: Vec::new(),
            history_idx: 0,
            bookmarks: vec![
                ("Search", "https://search.brave.com"),
                ("GitHub", "https://github.com"),
                ("Wikipedia", "https://wikipedia.org"),
                ("HN", "https://news.ycombinator.com"),
                ("YouTube", "https://youtube.com"),
                ("Reddit", "https://reddit.com"),
                ("Maps", "https://maps.google.com"),
            ],
            selected_bm: 0,
            should_quit: false,
            bitfox_status: "engine".to_string(),
            dom_nodes: 0,
            script_count: 0,
            loading_url: None,
        }
    }

    pub fn next_link(&mut self) {
        if self.links.is_empty() {
            return;
        }
        self.selected_link = Some(match self.selected_link {
            None => 0,
            Some(i) => (i + 1) % self.links.len(),
        });
        self.update_status_from_link();
    }

    pub fn prev_link(&mut self) {
        if self.links.is_empty() {
            return;
        }
        self.selected_link = Some(match self.selected_link {
            None => self.links.len().saturating_sub(1),
            Some(0) => self.links.len().saturating_sub(1),
            Some(i) => i - 1,
        });
        self.update_status_from_link();
    }

    fn update_status_from_link(&mut self) {
        if let Some(idx) = self.selected_link {
            if let Some(link) = self.links.get(idx) {
                self.status_text = format!("Link: {}", link.href);
            }
        }
    }

    pub fn follow_selected_link(&mut self) -> Option<String> {
        self.selected_link
            .and_then(|i| self.links.get(i))
            .map(|l| l.href.clone())
    }

    pub fn go_back(&mut self) -> Option<String> {
        if self.history_idx > 0 {
            self.history_idx -= 1;
            self.history.get(self.history_idx).cloned()
        } else {
            None
        }
    }

    pub fn push_history(&mut self, url: &str) {
        // Truncate forward history if we branched
        self.history.truncate(self.history_idx + 1);
        // Avoid duplicate consecutive entries
        if self.history.last().map(|u| u.as_str()) != Some(url) {
            self.history.push(url.to_string());
        }
        self.history_idx = self.history.len().saturating_sub(1);
    }

    pub fn load_page(&mut self, content: PageContent) {
        self.complexity = Self::detect_complexity(&content);
        self.script_count = content.scripts.len() as u32;
        self.links = content.links.clone();
        self.selected_link = None;
        self.scroll_offset = 0;
        self.status_text = format!(
            "Done — {} links, {}ms",
            content.links.len(),
            content.load_time_ms
        );
        self.url_input = content.url.clone();
        self.push_history(&content.url.clone());
        self.current_page = Some(content);
        self.mode = AppMode::Page;
        self.loading_url = None;
    }

    pub fn set_error(&mut self, msg: String) {
        self.status_text = format!("Error: {}", msg);
        self.mode = if self.current_page.is_some() {
            AppMode::Page
        } else {
            AppMode::NewTab
        };
        self.loading_url = None;
    }

    fn detect_complexity(content: &PageContent) -> u8 {
        let mut score: u8 = 0;
        if content.scripts.len() > 20 {
            score += 3;
        } else if content.scripts.len() > 10 {
            score += 2;
        }
        if content.links.len() > 100 {
            score += 1;
        }
        if content.html.contains("react")
            || content.html.contains("angular")
            || content.html.contains("vue")
        {
            score += 2;
        }
        score.min(8)
    }
}
