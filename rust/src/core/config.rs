use std::collections::BTreeSet;
use std::path::Path;

use nostr_sdk::prelude::RelayUrl;
use pika_marmot_runtime::runtime::{plan_runtime_relay_roles, RuntimeRelayRolePlan};
use pika_relay_profiles::{
    app_default_key_package_relays, app_default_message_relays, LEGACY_APP_DEFAULT_MESSAGE_RELAYS,
};
use serde::Deserialize;

use super::AppCore;

const DEFAULT_CALL_MOQ_URL: &str = "https://us-east.moq.logos.surf/anon";
const DEFAULT_CALL_BROADCAST_PREFIX: &str = "pika/calls";

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub(super) struct AppConfig {
    pub(super) disable_network: Option<bool>,
    pub(super) disable_agent_allowlist_probe: Option<bool>,
    pub(super) enable_external_signer: Option<bool>,
    pub(super) relay_urls: Option<Vec<String>>,
    pub(super) key_package_relay_urls: Option<Vec<String>>,
    pub(super) blossom_servers: Option<Vec<String>>,
    pub(super) call_moq_url: Option<String>,
    pub(super) call_broadcast_prefix: Option<String>,
    pub(super) call_audio_backend: Option<String>,
    pub(super) notification_url: Option<String>,
    pub(super) agent_api_url: Option<String>,
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

    pub(super) fn agent_allowlist_probe_enabled(&self) -> bool {
        !self.config.disable_agent_allowlist_probe.unwrap_or(false)
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

    pub(super) fn temporary_key_package_relays(&self) -> Vec<RelayUrl> {
        self.key_package_relays()
    }

    pub(super) fn blossom_servers(&self) -> Vec<String> {
        blossom_servers_or_default(self.config.blossom_servers.as_deref())
    }

    pub(super) fn long_lived_session_relays(&self) -> Vec<RelayUrl> {
        self.default_relays()
    }

    pub(super) fn relay_role_plan(
        &self,
        active_group_relays: Vec<RelayUrl>,
    ) -> RuntimeRelayRolePlan {
        plan_runtime_relay_roles(
            self.long_lived_session_relays(),
            active_group_relays,
            self.temporary_key_package_relays(),
        )
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
    use pika_relay_profiles::{
        app_default_blossom_servers, legacy_app_default_message_relays, RELAY_PIKACHAT_US_EAST,
    };
    use std::sync::{Arc, RwLock};

    fn make_core_with_config(config: AppConfig) -> (AppCore, tempfile::TempDir) {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let data_dir = tempdir.path().to_string_lossy().into_owned();
        let (update_tx, _update_rx) = flume::unbounded();
        let (core_tx, _core_rx) = flume::unbounded();
        let external_signer_bridge: crate::external_signer::SharedExternalSignerBridge =
            Arc::new(RwLock::new(None));
        let bunker_signer_connector: crate::bunker_signer::SharedBunkerSignerConnector =
            Arc::new(RwLock::new(Arc::new(
                crate::bunker_signer::NostrConnectBunkerSignerConnector::default(),
            )));
        let mut core = AppCore::new(
            update_tx,
            core_tx,
            data_dir,
            String::new(),
            String::new(),
            Arc::new(RwLock::new(crate::state::AppState::empty())),
            external_signer_bridge,
            bunker_signer_connector,
        );
        core.config = config;
        (core, tempdir)
    }

    fn relay_urls(urls: &[&str]) -> Vec<RelayUrl> {
        urls.iter()
            .map(|url| RelayUrl::parse(url).expect("relay url"))
            .collect()
    }

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
    fn load_app_config_reads_disable_agent_allowlist_probe() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("pika_config.json"),
            r#"{"disable_agent_allowlist_probe":true}"#,
        )
        .expect("write config");

        let config = load_app_config(dir.path().to_str().expect("utf8 path"));
        assert_eq!(config.disable_agent_allowlist_probe, Some(true));
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
        let mut urls = legacy_app_default_message_relays();
        urls[0].push('/');
        assert!(is_legacy_app_default_message_relays(&urls));
    }

    #[test]
    fn ignores_non_legacy_or_custom_relay_lists() {
        let mut urls = legacy_app_default_message_relays();
        urls.pop();
        urls.push(RELAY_PIKACHAT_US_EAST.to_string());
        assert!(!is_legacy_app_default_message_relays(&urls));
    }

    #[test]
    fn long_lived_session_relays_follow_message_relay_config() {
        let (core, _tempdir) = make_core_with_config(AppConfig {
            relay_urls: Some(vec![
                "wss://message-1.example".to_string(),
                "wss://message-2.example".to_string(),
            ]),
            key_package_relay_urls: Some(vec!["wss://kp-1.example".to_string()]),
            ..AppConfig::default()
        });

        let expected = relay_urls(&["wss://message-1.example", "wss://message-2.example"]);
        assert_eq!(core.default_relays(), expected);
        assert_eq!(core.long_lived_session_relays(), expected);
    }

    #[test]
    fn long_lived_session_relays_do_not_include_temporary_key_package_relays() {
        let (core, _tempdir) = make_core_with_config(AppConfig {
            relay_urls: Some(vec!["wss://message-1.example".to_string()]),
            key_package_relay_urls: Some(vec![
                "wss://kp-1.example".to_string(),
                "wss://kp-2.example".to_string(),
            ]),
            ..AppConfig::default()
        });

        let long_lived: BTreeSet<RelayUrl> = core.long_lived_session_relays().into_iter().collect();
        let temporary: BTreeSet<RelayUrl> =
            core.temporary_key_package_relays().into_iter().collect();

        assert_eq!(
            long_lived,
            relay_urls(&["wss://message-1.example"])
                .into_iter()
                .collect()
        );
        assert!(long_lived.is_disjoint(&temporary));
    }
}
