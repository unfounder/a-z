use tauri::{command, State, Window};
use std::sync::Arc;
use tokio::sync::RwLock;
use crate::browser::Browser;
use crate::error::{BrowserResult};

pub struct UiState {
    browser: Arc<RwLock<Browser>>,
}

#[command]
pub async fn navigate(url: String, state: State<'_, UiState>) -> Result<String, String> {
    let browser = state.browser.read().await;
    browser.fetch_url(&url).await
        .map_err(|e| e.to_string())
}

#[command]
pub async fn get_version(state: State<'_, UiState>) -> Result<String, String> {
    Ok("AZ Browser v0.1.0".to_string())
}

pub fn create_tauri_app(browser: Arc<crate::browser::Browser>) -> tauri::App {
    let state = UiState {
        browser: Arc::new(RwLock::new(browser.clone())),
    };

    tauri::Builder::default()
        .manage(state)
        .invoke_handler(tauri::generate_handler![navigate, get_version])
        .build(tauri::generate_context!())
        .expect("Failed to build Tauri app")
}