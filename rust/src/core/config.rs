use std::collections::BTreeSet;
use std::path::Path;

use nostr_sdk::prelude::{RelayUrl, Url};
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
];
const DEFAULT_CALL_MOQ_URL: &str = "https://us-east.moq.logos.surf/anon";
const DEFAULT_CALL_BROADCAST_PREFIX: &str = "pika/calls";

const DEFAULT_BLOSSOM_SERVERS: &[&str] = &[
    "https://us-east.nostr.pikachat.org",
    "https://eu.nostr.pikachat.org",
];

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub(super) struct AppConfig {
    pub(super) disable_network: Option<bool>,
    pub(super) enable_external_signer: Option<bool>,
    pub(super) relay_urls: Option<Vec<String>>,
    pub(super) key_package_relay_urls: Option<Vec<String>>,
    pub(super) blossom_servers: Option<Vec<String>>,
    pub(super) call_moq_url: Option<String>,
    pub(super) call_broadcast_prefix: Option<String>,
    pub(super) call_audio_backend: Option<String>,
    pub(super) notification_url: Option<String>,
    // Dev-only: run a one-shot QUIC+TLS probe on startup and log PASS/FAIL.
    pub(super) moq_probe_on_start: Option<bool>,
    // Multi-device: automatically add new devices to all groups (default: true).
    pub(super) auto_add_devices: Option<bool>,
}

pub(super) fn load_app_config(data_dir: &str) -> AppConfig {
    let path = Path::new(data_dir).join("pika_config.json");
    let Ok(bytes) = std::fs::read(&path) else {
        return AppConfig::default();
    };
    serde_json::from_slice::<AppConfig>(&bytes).unwrap_or_default()
}

pub(super) fn default_app_config_json() -> String {
    serde_json::json!({
        "relay_urls": DEFAULT_RELAY_URLS,
        "key_package_relay_urls": DEFAULT_KEY_PACKAGE_RELAY_URLS,
        "call_moq_url": DEFAULT_CALL_MOQ_URL,
        "call_broadcast_prefix": DEFAULT_CALL_BROADCAST_PREFIX,
    })
    .to_string()
}

pub(super) fn relay_reset_config_json(existing_json: Option<&str>) -> String {
    let mut value = existing_json
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
        .unwrap_or_else(|| {
            serde_json::from_str::<serde_json::Value>(&default_app_config_json())
                .unwrap_or_else(|_| serde_json::json!({}))
        });

    if !value.is_object() {
        value = serde_json::json!({});
    }

    if let Some(obj) = value.as_object_mut() {
        obj.insert("relay_urls".into(), serde_json::json!(DEFAULT_RELAY_URLS));
        obj.insert(
            "key_package_relay_urls".into(),
            serde_json::json!(DEFAULT_KEY_PACKAGE_RELAY_URLS),
        );
    }

    value.to_string()
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

    pub(super) fn blossom_servers(&self) -> Vec<String> {
        if let Some(urls) = &self.config.blossom_servers {
            let parsed: Vec<String> = urls
                .iter()
                .filter_map(|u| {
                    let t = u.trim();
                    if t.is_empty() {
                        return None;
                    }
                    Url::parse(t).ok().map(|_| t.to_string())
                })
                .collect();
            if !parsed.is_empty() {
                return parsed;
            }
        }

        DEFAULT_BLOSSOM_SERVERS
            .iter()
            .filter_map(|u| Url::parse(u).ok().map(|_| (*u).to_string()))
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

    pub(super) fn external_signer_enabled(&self) -> bool {
        if let Some(enabled) = self.config.enable_external_signer {
            return enabled;
        }
        matches!(
            std::env::var("PIKA_ENABLE_EXTERNAL_SIGNER").ok().as_deref(),
            Some("1") | Some("true") | Some("TRUE")
        )
    }
}
