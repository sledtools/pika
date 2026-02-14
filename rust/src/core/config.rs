use std::collections::BTreeSet;
use std::path::Path;

use nostr_sdk::prelude::RelayUrl;
use serde::Deserialize;

use super::AppCore;

// "Popular ones" per user request; keep small for MVP.
const DEFAULT_RELAY_URLS: &[&str] = &[
    "wss://relay.damus.io",
    "wss://relay.primal.net",
    "wss://nos.lol",
];

// Key packages (kind 443) are NIP-70 "protected" in modern MDK.
// Many popular relays (incl. Damus/Primal/nos.lol) currently reject protected events.
// Default these to relays that accept protected kind 443 publishes (manual probe).
const DEFAULT_KEY_PACKAGE_RELAY_URLS: &[&str] = &[
    "wss://nostr-pub.wellorder.net",
    "wss://nostr-01.yakihonne.com",
    "wss://nostr-02.yakihonne.com",
    "wss://relay.satlantis.io",
];

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub(super) struct AppConfig {
    pub(super) disable_network: Option<bool>,
    pub(super) relay_urls: Option<Vec<String>>,
    pub(super) key_package_relay_urls: Option<Vec<String>>,
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

    pub(super) fn key_package_relays(&self) -> Vec<RelayUrl> {
        if let Some(urls) = &self.config.key_package_relay_urls {
            let parsed: Vec<RelayUrl> = urls
                .iter()
                .filter_map(|u| RelayUrl::parse(u).ok())
                .collect();
            if !parsed.is_empty() {
                return parsed;
            }
        }
        DEFAULT_KEY_PACKAGE_RELAY_URLS
            .iter()
            .filter_map(|u| RelayUrl::parse(u).ok())
            .collect()
    }

    pub(super) fn all_session_relays(&self) -> Vec<RelayUrl> {
        // Ensure the single nostr-sdk client can publish/fetch both:
        // - normal traffic on general relays
        // - key packages (kind 443) on key-package relays
        let mut set: BTreeSet<RelayUrl> = BTreeSet::new();
        for r in self.default_relays() {
            set.insert(r);
        }
        for r in self.key_package_relays() {
            set.insert(r);
        }
        set.into_iter().collect()
    }
}
