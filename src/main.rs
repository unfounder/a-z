mod tui;

use std::sync::Arc;

use a_z::browser::Browser;

#[tokio::main]
async fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();

    let args: Vec<String> = std::env::args().collect();

    let browser = Arc::new(Browser::new());

    if args.iter().any(|a| a == "--headless") {
        // Headless CDP server
        let cdp_port: u16 = args.iter()
            .position(|a| a == "--port")
            .and_then(|i| args.get(i + 1))
            .and_then(|s| s.parse().ok())
            .unwrap_or(9222);
        let addr = format!("127.0.0.1:{}", cdp_port);
        let server = Arc::new(a_z::cdp::server::CdpServer::new(Arc::clone(&browser)));
        if let Err(e) = server.listen(&addr).await {
            eprintln!("CDP error: {e}");
        }
    } else {
        // Default: TUI
        if let Err(e) = tui::run_tui(browser).await {
            eprintln!("TUI error: {e}");
        }
    }
}
