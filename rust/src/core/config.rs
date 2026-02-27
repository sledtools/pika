use std::collections::BTreeSet;
use std::path::Path;

use nostr_sdk::prelude::RelayUrl;
use pika_relay_profiles::{app_default_key_package_relays, app_default_message_relays};
use serde::Deserialize;

use super::AppCore;

const DEFAULT_CALL_MOQ_URL: &str = "https://us-east.moq.logos.surf/anon";
const DEFAULT_CALL_BROADCAST_PREFIX: &str = "pika/calls";
const LEGACY_APP_DEFAULT_MESSAGE_RELAYS: &[&str] = &[
    "wss://relay.primal.net",
    "wss://nos.lol",
    "wss://relay.damus.io",
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
}

pub(super) fn load_app_config(data_dir: &str) -> AppConfig {
    let path = Path::new(data_dir).join("pika_config.json");
    let Ok(bytes) = std::fs::read(&path) else {
        return AppConfig::default();
    };
    serde_json::from_slice::<AppConfig>(&bytes).unwrap_or_default()
}

pub(super) fn default_app_config_json() -> String {
    let relay_urls = app_default_message_relays();
    let key_package_relay_urls = app_default_key_package_relays();
    serde_json::json!({
        "relay_urls": relay_urls,
        "key_package_relay_urls": key_package_relay_urls,
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
        obj.insert(
            "relay_urls".into(),
            serde_json::json!(app_default_message_relays()),
        );
        obj.insert(
            "key_package_relay_urls".into(),
            serde_json::json!(app_default_key_package_relays()),
        );
    }

    value.to_string()
}

fn blossom_servers_or_default(values: Option<&[String]>) -> Vec<String> {
    pika_relay_profiles::app_blossom_servers_or_default(values.unwrap_or(&[]))
}

fn is_legacy_app_default_message_relays(values: &[String]) -> bool {
    let normalized: Option<BTreeSet<String>> = values
        .iter()
        .map(|raw| {
            let trimmed = raw.trim();
            RelayUrl::parse(trimmed)
                .ok()
                .map(|u| u.as_str_without_trailing_slash().to_string())
        })
        .collect();
    let Some(normalized) = normalized else {
        return false;
    };
    let legacy: BTreeSet<String> = LEGACY_APP_DEFAULT_MESSAGE_RELAYS
        .iter()
        .map(|u| (*u).to_string())
        .collect();
    normalized == legacy
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
            if is_legacy_app_default_message_relays(urls) {
                return app_default_message_relays()
                    .iter()
                    .filter_map(|u| RelayUrl::parse(u).ok())
                    .collect();
            }
            let parsed: Vec<RelayUrl> = urls
                .iter()
                .filter_map(|u| RelayUrl::parse(u).ok())
                .collect();
            if !parsed.is_empty() {
                return parsed;
            }
        }
        app_default_message_relays()
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
        app_default_key_package_relays()
            .iter()
            .filter_map(|u| RelayUrl::parse(u).ok())
            .collect()
    }

    pub(super) fn blossom_servers(&self) -> Vec<String> {
        blossom_servers_or_default(self.config.blossom_servers.as_deref())
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

#[cfg(test)]
mod tests {
    use super::*;
    use pika_relay_profiles::app_default_blossom_servers;

    #[test]
    fn default_app_config_json_uses_shared_profile_defaults() {
        let value: serde_json::Value =
            serde_json::from_str(&default_app_config_json()).expect("parse config json");
        assert_eq!(
            value["relay_urls"],
            serde_json::json!(app_default_message_relays())
        );
        assert_eq!(
            value["key_package_relay_urls"],
            serde_json::json!(app_default_key_package_relays())
        );
        assert_eq!(value["call_moq_url"], DEFAULT_CALL_MOQ_URL);
        assert_eq!(
            value["call_broadcast_prefix"],
            DEFAULT_CALL_BROADCAST_PREFIX
        );
    }

    #[test]
    fn relay_reset_replaces_relays_and_preserves_other_fields() {
        let existing = r#"{
            "relay_urls": ["wss://invalid.example"],
            "key_package_relay_urls": ["wss://invalid-kp.example"],
            "disable_network": true
        }"#;
        let value: serde_json::Value =
            serde_json::from_str(&relay_reset_config_json(Some(existing)))
                .expect("parse reset config json");
        assert_eq!(
            value["relay_urls"],
            serde_json::json!(app_default_message_relays())
        );
        assert_eq!(
            value["key_package_relay_urls"],
            serde_json::json!(app_default_key_package_relays())
        );
        assert_eq!(value["disable_network"], serde_json::json!(true));
    }

    #[test]
    fn relay_reset_handles_invalid_input_json() {
        let value: serde_json::Value = serde_json::from_str(&relay_reset_config_json(Some("{")))
            .expect("parse reset config json");
        assert_eq!(
            value["relay_urls"],
            serde_json::json!(app_default_message_relays())
        );
        assert_eq!(
            value["key_package_relay_urls"],
            serde_json::json!(app_default_key_package_relays())
        );
    }

    #[test]
    fn blossom_servers_or_default_falls_back_for_missing_or_invalid_values() {
        assert_eq!(
            blossom_servers_or_default(None),
            app_default_blossom_servers()
        );
        let invalid = vec!["".to_string(), "not-a-url".to_string()];
        assert_eq!(
            blossom_servers_or_default(Some(&invalid)),
            app_default_blossom_servers()
        );
    }

    #[test]
    fn blossom_servers_or_default_keeps_valid_values() {
        let values = vec!["https://blossom.example.com".to_string()];
        assert_eq!(
            blossom_servers_or_default(Some(&values)),
            vec!["https://blossom.example.com".to_string()]
        );
    }

    #[test]
    fn detects_legacy_default_message_relays() {
        let urls = vec![
            "wss://nos.lol/".to_string(),
            "wss://relay.damus.io".to_string(),
            "wss://relay.primal.net".to_string(),
        ];
        assert!(is_legacy_app_default_message_relays(&urls));
    }

    #[test]
    fn ignores_non_legacy_or_custom_relay_lists() {
        let urls = vec![
            "wss://relay.primal.net".to_string(),
            "wss://nos.lol".to_string(),
            "wss://us-east.nostr.pikachat.org".to_string(),
        ];
        assert!(!is_legacy_app_default_message_relays(&urls));
    }
}
