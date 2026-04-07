use arti_client::{TorClient, TorClientConfig};
use reqwest::{Client, Proxy};
use std::sync::Mutex;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectivityMode {
    Direct,
    Tor,
    VPN(String), // Path to WireGuard config
}

pub struct NetworkController {
    pub mode: Mutex<ConnectivityMode>,
}

impl NetworkController {
    pub fn new() -> Self {
        Self {
            mode: Mutex::new(ConnectivityMode::Direct),
        }
    }

    pub fn set_mode(&self, mode: ConnectivityMode) {
        let mut m = self.mode.lock().unwrap();
        *m = mode;
    }

    pub fn get_mode(&self) -> ConnectivityMode {
        self.mode.lock().unwrap().clone()
    }

    /// Bootstraps the selected network mode and returns a reqwest client
    pub async fn get_client(&self) -> Result<Client, Box<dyn std::error::Error + Send + Sync>> {
        let mode = self.get_mode();
        match mode {
            ConnectivityMode::Direct => {
                Ok(Client::builder()
                    .user_agent("Bitfox-A-Z-Security-Browser")
                    .build()?)
            }
            ConnectivityMode::Tor => {
                // Initialize Tor (Arti) internally
                let config = TorClientConfig::default();
                let _tor_client = TorClient::create_bootstrapped(config).await?;
                
                // Route reqwest through the internal Tor SOCKS proxy (Arti default)
                let proxy = Proxy::all("socks5h://127.0.0.1:9050")?; 
                Ok(Client::builder()
                    .proxy(proxy)
                    .user_agent("Bitfox-A-Z-Security-Browser")
                    .build()?)
            }
            ConnectivityMode::VPN(_cfg_path) => {
                // This is where you'd hook BoringTun or WireGuard-rs
                Err("VPN mode not yet fully integrated".into())
            }
        }
    }
}

#[cfg(feature = "sniffer")]
pub fn start_sniffing(interface_name: &str, tx: std::sync::mpsc::Sender<String>) {
    use pcap::Capture;
    let mut cap = match Capture::from_device(interface_name) {
        Ok(d) => d
            .promisc(true)
            .snaplen(65535)
            .open(),
        Err(e) => {
            log::warn!("pcap: failed to open {}: {}", interface_name, e);
            return;
        }
    }.unwrap();

    while let Ok(packet) = cap.next_packet() {
        let msg = format!("Packet len: {} on {}", packet.header.len, interface_name);
        if tx.send(msg).is_err() {
            break;
        }
    }
}

#[cfg(not(feature = "sniffer"))]
pub fn start_sniffing(_interface_name: &str, _tx: std::sync::mpsc::Sender<String>) {
    log::warn!("Packet sniffer is disabled. Build with --features sniffer (requires Npcap SDK).");
}
