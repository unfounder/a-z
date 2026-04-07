// ── Ratatui TUI for A-Z Browser ────────────────────────────────────────────────

pub mod app;
pub mod events;
pub mod renderer;
pub mod widgets;

use std::sync::Arc;
use std::time::Instant;

use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture, Event, EventStream},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use futures_util::StreamExt;
use ratatui::{backend::CrosstermBackend, Terminal};

use a_z::browser::Browser;
use a_z::dom::walker::walk_document;
use a_z::error::BrowserResult;

use self::app::{App, AppMode, PageContent, VideoInfo};
use self::events::handle_key;
use self::renderer::render;

use a_z::browser::video::detect_videos;

// ── Color palette ──────────────────────────────────────────────────────────────

#[derive(Clone, Copy)]
#[allow(dead_code)]
pub struct Palette {
    pub bg: ratatui::style::Color,
    pub bg_dark: ratatui::style::Color,
    pub text: ratatui::style::Color,
    pub text_dim: ratatui::style::Color,
    pub green: ratatui::style::Color,
    pub blue: ratatui::style::Color,
    pub yellow: ratatui::style::Color,
    pub orange: ratatui::style::Color,
    pub red: ratatui::style::Color,
    pub accent: ratatui::style::Color,
}

use ratatui::style::Color;

pub static COLORS: Palette = Palette {
    bg: Color::Rgb(26, 26, 46),
    bg_dark: Color::Rgb(13, 13, 26),
    text: Color::Rgb(200, 200, 200),
    text_dim: Color::Rgb(85, 85, 102),
    green: Color::Rgb(0, 255, 65),
    blue: Color::Rgb(0, 191, 255),
    yellow: Color::Rgb(255, 255, 0),
    orange: Color::Rgb(255, 107, 53),
    red: Color::Rgb(255, 95, 87),
    accent: Color::Rgb(0, 194, 168),
};

// ── Navigation result channel ──────────────────────────────────────────────────

enum NavResult {
    Ok(PageContent),
    Err(String),
}

// ── TUI entry point ────────────────────────────────────────────────────────────

pub async fn run_tui(browser: Arc<Browser>) -> BrowserResult<()> {
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new();
    let mut event_reader = EventStream::new();
    let (nav_tx, mut nav_rx) = tokio::sync::mpsc::channel::<NavResult>(4);

    loop {
        terminal.draw(|frame| render(frame, &app))?;

        if app.should_quit {
            break;
        }

        tokio::select! {
            Some(Ok(event)) = event_reader.next() => {
                if let Event::Key(key) = event {
                    use crossterm::event::KeyEventKind;
                    if key.kind != KeyEventKind::Press { continue; }

                    if let Some(url) = handle_key(&mut app, key) {
                        app.loading_url = Some(url.clone());
                        app.mode = AppMode::Loading;
                        app.status_text = format!("Connecting to {}...", url);

                        let browser = Arc::clone(&browser);
                        let tx = nav_tx.clone();
                        tokio::spawn(async move {
                            let start = Instant::now();
                            match browser.navigate(&url).await {
                                Ok(_html) => {
                                    let page = extract_page_content(
                                        &browser,
                                        &url,
                                        start.elapsed().as_millis() as u64,
                                    );
                                    let _ = tx.send(NavResult::Ok(page)).await;
                                }
                                Err(e) => {
                                    let _ = tx.send(NavResult::Err(e.to_string())).await;
                                }
                            }
                        });
                    }
                }
            }

            Some(result) = nav_rx.recv() => {
                match result {
                    NavResult::Ok(page) => app.load_page(page),
                    NavResult::Err(msg) => app.set_error(msg),
                }
            }
        }
    }

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture,
    )?;
    terminal.show_cursor()?;

    Ok(())
}

// ── Page content extraction ────────────────────────────────────────────────────

fn extract_page_content(browser: &Browser, url: &str, elapsed_ms: u64) -> PageContent {
    let guard = browser.page_state.lock().expect("page_state lock");

    let Some(state) = guard.as_ref() else {
        return PageContent {
            url: url.to_string(),
            title: "Error".to_string(),
            html: String::new(),
            links: Vec::new(),
            scripts: Vec::new(),
            load_time_ms: elapsed_ms,
            status: 0,
            videos: Vec::new(),
            forms: Vec::new(),
            rendered_lines: Vec::new(),
        };
    };

    let nodes = &state.document.nodes;
    let body_id = nodes
        .iter()
        .find(|n| n.tag_name() == Some("body"))
        .map(|n| n.id)
        .unwrap_or(0);

    let result = walk_document(nodes, body_id, &state.url);

    let scripts: Vec<String> = (0..result.script_count).map(|_| String::new()).collect();
    let videos: Vec<VideoInfo> = detect_videos("", &state.url);

    PageContent {
        url: state.url.clone(),
        title: result.title,
        html: String::new(),
        links: result.links,
        scripts,
        load_time_ms: elapsed_ms,
        status: 200,
        videos,
        forms: Vec::new(),
        rendered_lines: result.lines,
    }
}
