use std::path::Path;

use nostr_sdk::prelude::RelayUrl;
use serde::Deserialize;

use super::AppCore;

// "Popular ones" per user request; keep small for MVP.
const DEFAULT_RELAY_URLS: &[&str] = &["wss://relay.damus.io", "wss://relay.primal.net"];

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub(super) struct AppConfig {
    pub(super) disable_network: Option<bool>,
    pub(super) relay_urls: Option<Vec<String>>,
    pub(super) call_moq_url: Option<String>,
    pub(super) call_broadcast_prefix: Option<String>,
    pub(super) call_audio_backend: Option<String>,
}

pub(super) fn load_app_config(data_dir: &str) -> AppConfig {
    let path = Path::new(data_dir).join("pika_config.json");
    let Ok(bytes) = std::fs::read(&path) else {
        return AppConfig::default();
    };
    serde_json::from_slice::<AppConfig>(&bytes).unwrap_or_default()
}

impl AppCore {
    pub(super) fn network_enabled(&self) -> bool {
        // Used to keep Rust tests deterministic and offline.
        if let Some(disable) = self.config.disable_network {
            return !disable;
        }
        std::env::var("PIKA_DISABLE_NETWORK").ok().as_deref() != Some("1")
    }

    pub(super) fn default_relays(&self) -> Vec<RelayUrl> {
        if let Some(urls) = &self.config.relay_urls {
            let parsed: Vec<RelayUrl> = urls
                .iter()
                .filter_map(|u| RelayUrl::parse(u).ok())
                .collect();
            if !parsed.is_empty() {
                return parsed;
            }
        }
        DEFAULT_RELAY_URLS
            .iter()
            .filter_map(|u| RelayUrl::parse(u).ok())
            .collect()
    }
}
