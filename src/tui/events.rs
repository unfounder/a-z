// ── Input event handling ───────────────────────────────────────────────────────

use super::app::{App, AppMode};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Returns Some(url) if the user triggered a navigation, None otherwise.
pub fn handle_key(app: &mut App, key: KeyEvent) -> Option<String> {
    // Ctrl+C always quits
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        app.should_quit = true;
        return None;
    }

    match app.mode {
        AppMode::NewTab => handle_newtab(app, key),
        AppMode::Page => handle_page(app, key),
        AppMode::Input => handle_input(app, key),
        AppMode::Loading => None,
    }
}

fn handle_newtab(app: &mut App, key: KeyEvent) -> Option<String> {
    match key.code {
        KeyCode::Enter => {
            let q = app.search_input.trim().to_string();
            if q.is_empty() {
                return None;
            }
            app.search_input.clear();
            Some(resolve_input(q))
        }
        KeyCode::Backspace => {
            app.search_input.pop();
            None
        }
        // 'q' quits only when search bar is empty (not mid-typing)
        KeyCode::Char('q') if app.search_input.is_empty() => {
            app.should_quit = true;
            None
        }
        KeyCode::Char(c) => {
            app.search_input.push(c);
            None
        }
        _ => None,
    }
}

fn handle_page(app: &mut App, key: KeyEvent) -> Option<String> {
    match key.code {
        KeyCode::Tab | KeyCode::Down => {
            app.next_link();
            None
        }
        KeyCode::BackTab | KeyCode::Up => {
            app.prev_link();
            None
        }
        // Right arrow / Enter: follow selected link (lynx key)
        KeyCode::Right | KeyCode::Enter => app.follow_selected_link().map(resolve_input),

        // Left arrow / b: go back (lynx key)
        KeyCode::Left => app.go_back(),

        KeyCode::Char('b') | KeyCode::Char('B') | KeyCode::Backspace => {
            let url = app.go_back();
            if url.is_none() {
                app.status_text = "No more history.".to_string();
            }
            url
        }

        KeyCode::Char('/') | KeyCode::Char('g') => {
            app.mode = AppMode::Input;
            app.url_input = String::new();
            None
        }

        KeyCode::Char('q') => {
            app.mode = AppMode::NewTab;
            app.current_page = None;
            app.links.clear();
            app.status_text = "Ready".to_string();
            None
        }

        KeyCode::Char('v') | KeyCode::Char('V') => {
            use super::app::ViewMode;
            app.view_mode = ViewMode::PageView;
            app.status_text = "Page view — press T to return to terminal".to_string();
            None
        }

        KeyCode::Char('t') | KeyCode::Char('T') => {
            use super::app::ViewMode;
            app.view_mode = ViewMode::Terminal;
            app.status_text = "Terminal mode".to_string();
            None
        }

        KeyCode::PageDown | KeyCode::Char('j') => {
            app.scroll_offset = app.scroll_offset.saturating_add(5);
            None
        }
        KeyCode::PageUp | KeyCode::Char('k') => {
            app.scroll_offset = app.scroll_offset.saturating_sub(5);
            None
        }

        KeyCode::Char('p') | KeyCode::Char('P') => {
            if let Some(page) = &app.current_page {
                if let Some(video) = page.videos.first() {
                    match a_z::browser::video::launch_video(video) {
                        Ok(_) => app.status_text = "Launching external player...".into(),
                        Err(e) => app.status_text = format!("Player error: {}", e),
                    }
                }
            }
            None
        }

        KeyCode::Esc => {
            app.selected_link = None;
            app.status_text = "Ready".to_string();
            None
        }

        _ => None,
    }
}

fn handle_input(app: &mut App, key: KeyEvent) -> Option<String> {
    match key.code {
        KeyCode::Enter => {
            let url = app.url_input.trim().to_string();
            if url.is_empty() {
                app.mode = AppMode::Page;
                return None;
            }
            app.url_input.clear();
            app.mode = AppMode::Loading;
            Some(resolve_input(url))
        }
        KeyCode::Esc => {
            app.mode = if app.current_page.is_some() {
                AppMode::Page
            } else {
                AppMode::NewTab
            };
            None
        }
        KeyCode::Char(c) => {
            app.url_input.push(c);
            None
        }
        KeyCode::Backspace => {
            app.url_input.pop();
            None
        }
        _ => None,
    }
}

pub fn resolve_input(input: String) -> String {
    let s = input.trim();
    if s.starts_with("http://") || s.starts_with("https://") {
        s.to_string()
    } else if s.contains('.') && !s.contains(' ') {
        format!("https://{}", s)
    } else {
        // Encode the search query manually using percent-encoding
        let encoded: String = s
            .bytes()
            .flat_map(|b| match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                    vec![b as char]
                }
                b' ' => vec!['+'],
                _ => format!("%{:02X}", b).chars().collect::<Vec<_>>(),
            })
            .collect();
        format!("https://search.brave.com/search?q={}", encoded)
    }
}
