use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use nostr::event::Event;
use nostr::key::PublicKey;
use nostr::nips::nip19::ToBech32;
use rand::Rng;

use crate::storage::Store;

const CHALLENGE_TTL: Duration = Duration::from_secs(120);
const TOKEN_TTL_DAYS: i64 = 90;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AccessState {
    pub can_chat: bool,
    pub is_admin: bool,
    pub can_forge_write: bool,
}

pub struct AuthState {
    challenges: Mutex<HashMap<String, (String, Instant)>>,
    store: Store,
    bootstrap_admin_npubs: HashSet<String>,
    legacy_allowed_npubs: HashSet<String>,
}

impl AuthState {
    pub fn new(
        bootstrap_admin_npubs: &[String],
        legacy_allowed_npubs: &[String],
        store: Store,
    ) -> Self {
        let bootstrap_admin_npubs = bootstrap_admin_npubs
            .iter()
            .filter_map(|npub| normalize_npub(npub).ok())
            .collect();
        let legacy_allowed_npubs = legacy_allowed_npubs
            .iter()
            .filter_map(|npub| normalize_npub(npub).ok())
            .collect();

        // Clean up expired tokens on startup
        if let Err(e) = store.cleanup_expired_tokens(TOKEN_TTL_DAYS) {
            eprintln!("warning: failed to cleanup expired tokens: {}", e);
        }

        Self {
            challenges: Mutex::new(HashMap::new()),
            store,
            bootstrap_admin_npubs,
            legacy_allowed_npubs,
        }
    }

    pub fn create_challenge(&self) -> String {
        let nonce: String = hex::encode(rand::thread_rng().gen::<[u8; 16]>());
        let mut challenges = self.challenges.lock().unwrap();
        // Clean expired
        challenges.retain(|_, (_, created)| created.elapsed() < CHALLENGE_TTL);
        challenges.insert(nonce.clone(), (nonce.clone(), Instant::now()));
        nonce
    }

    pub fn verify_event(&self, event_json: &str) -> Result<(String, String, bool), String> {
        let event: Event =
            serde_json::from_str(event_json).map_err(|e| format!("invalid event JSON: {}", e))?;

        // Verify signature
        event
            .verify()
            .map_err(|e| format!("invalid signature: {}", e))?;

        // Check kind 27235 (NIP-98)
        if event.kind.as_u16() != 27235 {
            return Err(format!("expected kind 27235, got {}", event.kind.as_u16()));
        }

        // Check challenge nonce is in content
        let content = event.content.to_string();
        {
            let mut challenges = self.challenges.lock().unwrap();
            if challenges.remove(&content).is_none() {
                return Err("challenge nonce not found or expired".to_string());
            }
        }

        // Issue token and persist to SQLite
        let token = hex::encode(rand::thread_rng().gen::<[u8; 32]>());
        let npub = event
            .pubkey
            .to_bech32()
            .map_err(|e| format!("failed to encode npub: {}", e))?
            .to_lowercase();

        let access = self.access_for_npub(&npub);
        if !(access.can_chat || access.can_forge_write) {
            return Err("pubkey not authorized".to_string());
        }

        self.store
            .insert_auth_token(&token, &npub)
            .map_err(|e| format!("failed to persist token: {}", e))?;

        Ok((token, npub, access.is_admin))
    }

    pub fn validate_token(&self, token: &str) -> Option<String> {
        self.store
            .validate_auth_token(token, TOKEN_TTL_DAYS)
            .ok()
            .flatten()
    }

    pub fn access_for_npub(&self, npub: &str) -> AccessState {
        let normalized = match normalize_npub(npub) {
            Ok(value) => value,
            Err(_) => {
                return AccessState {
                    can_chat: false,
                    is_admin: false,
                    can_forge_write: false,
                };
            }
        };
        let is_admin = self.bootstrap_admin_npubs.contains(&normalized);
        let can_forge_write = is_admin
            || self.legacy_allowed_npubs.contains(&normalized)
            || self
                .store
                .is_chat_allowlist_forge_writer(&normalized)
                .ok()
                .unwrap_or(false);
        let can_chat = is_admin
            || self.legacy_allowed_npubs.contains(&normalized)
            || self
                .store
                .is_chat_allowlist_active(&normalized)
                .ok()
                .unwrap_or(false);
        AccessState {
            can_chat,
            is_admin,
            can_forge_write,
        }
    }

    pub fn is_admin(&self, npub: &str) -> bool {
        self.access_for_npub(npub).is_admin
    }

    pub fn is_legacy_allowed(&self, npub: &str) -> bool {
        let normalized = match normalize_npub(npub) {
            Ok(value) => value,
            Err(_) => return false,
        };
        !self.bootstrap_admin_npubs.contains(&normalized)
            && self.legacy_allowed_npubs.contains(&normalized)
    }

    pub fn is_config_managed_chat_principal(&self, npub: &str) -> bool {
        self.is_admin(npub) || self.is_legacy_allowed(npub)
    }

    pub fn bootstrap_admin_npubs(&self) -> Vec<String> {
        let mut npubs: Vec<String> = self.bootstrap_admin_npubs.iter().cloned().collect();
        npubs.sort();
        npubs
    }

    pub fn legacy_allowed_npubs(&self) -> Vec<String> {
        let mut npubs: Vec<String> = self.legacy_allowed_npubs.iter().cloned().collect();
        npubs.sort();
        npubs
    }

    pub fn chat_enabled(&self) -> bool {
        !self.bootstrap_admin_npubs.is_empty()
            || !self.legacy_allowed_npubs.is_empty()
            || self
                .store
                .has_active_chat_allowlist_entries()
                .ok()
                .unwrap_or(false)
    }

    pub fn auth_enabled(&self) -> bool {
        self.chat_enabled()
            || self
                .store
                .has_chat_allowlist_forge_writers()
                .ok()
                .unwrap_or(false)
    }
}

pub fn normalize_npub(input: &str) -> Result<String, String> {
    PublicKey::parse(input.trim())
        .map_err(|e| format!("invalid nostr public key: {}", e))?
        .to_bech32()
        .map(|npub| npub.to_lowercase())
        .map_err(|e| format!("failed to encode npub: {}", e))
}

#[cfg(test)]
mod tests {
    use nostr::key::{Keys, PublicKey};
    use nostr::{EventBuilder, Kind, Tag, TagKind, ToBech32};

    use super::AuthState;
    use crate::storage::Store;

    const SAMPLE_NPUB: &str = "npub1zxu639qym0esxnn7rzrt48wycmfhdu3e5yvzwx7ja3t84zyc2r8qz8cx2y";

    #[test]
    fn legacy_allowed_user_can_chat_but_is_not_admin() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("pika-news.db");
        let store = Store::open(&db_path).expect("open store");
        let auth = AuthState::new(&[], &[SAMPLE_NPUB.to_string()], store);

        assert!(auth.access_for_npub(SAMPLE_NPUB).can_chat);
        assert!(!auth.is_admin(SAMPLE_NPUB));
        assert!(auth.access_for_npub(SAMPLE_NPUB).can_forge_write);
    }

    #[test]
    fn hex_legacy_allowed_user_normalizes_for_chat_access() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("pika-news.db");
        let store = Store::open(&db_path).expect("open store");
        let pk = PublicKey::parse(SAMPLE_NPUB).expect("parse sample npub");
        let auth = AuthState::new(&[], &[pk.to_hex()], store);

        assert!(auth.access_for_npub(SAMPLE_NPUB).can_chat);
        assert!(!auth.is_admin(SAMPLE_NPUB));
        assert!(auth.access_for_npub(SAMPLE_NPUB).can_forge_write);
    }

    #[test]
    fn config_managed_principals_are_detected() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("pika-news.db");
        let store = Store::open(&db_path).expect("open store");
        let legacy_hex = PublicKey::parse(SAMPLE_NPUB)
            .expect("parse sample npub")
            .to_hex();
        let auth = AuthState::new(&[SAMPLE_NPUB.to_string()], &[legacy_hex], store);

        assert!(auth.is_config_managed_chat_principal(SAMPLE_NPUB));
    }

    #[test]
    fn bootstrap_admin_has_admin_and_chat_access() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("pika-news.db");
        let store = Store::open(&db_path).expect("open store");
        let auth = AuthState::new(&[SAMPLE_NPUB.to_string()], &[], store);

        let access = auth.access_for_npub(SAMPLE_NPUB);
        assert!(access.is_admin);
        assert!(access.can_chat);
        assert!(access.can_forge_write);
    }

    #[test]
    fn managed_chat_allowlist_user_cannot_write_forge() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("pika-news.db");
        let store = Store::open(&db_path).expect("open store");
        store
            .upsert_chat_allowlist_entry(SAMPLE_NPUB, true, false, Some("chat"), "npub1admin")
            .expect("insert chat allowlist entry");
        let auth = AuthState::new(&[], &[], store);

        let access = auth.access_for_npub(SAMPLE_NPUB);
        assert!(access.can_chat);
        assert!(!access.is_admin);
        assert!(!access.can_forge_write);
    }

    #[test]
    fn managed_trusted_contributor_can_write_forge() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("pika-news.db");
        let store = Store::open(&db_path).expect("open store");
        store
            .upsert_chat_allowlist_entry(SAMPLE_NPUB, true, true, Some("trusted"), "npub1admin")
            .expect("insert trusted allowlist entry");
        let auth = AuthState::new(&[], &[], store);

        let access = auth.access_for_npub(SAMPLE_NPUB);
        assert!(access.can_chat);
        assert!(!access.is_admin);
        assert!(access.can_forge_write);
    }

    #[test]
    fn inactive_forge_writer_can_auth_without_chat_access() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("pika-news.db");
        let store = Store::open(&db_path).expect("open store");
        store
            .upsert_chat_allowlist_entry(SAMPLE_NPUB, false, true, Some("forge-only"), "npub1admin")
            .expect("insert forge-only allowlist entry");
        let auth = AuthState::new(&[], &[], store);

        let access = auth.access_for_npub(SAMPLE_NPUB);
        assert!(!access.can_chat);
        assert!(access.can_forge_write);
        assert!(auth.auth_enabled());
    }

    #[test]
    fn verify_event_issues_token_for_forge_only_principal() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("pika-news.db");
        let store = Store::open(&db_path).expect("open store");
        let keys = Keys::generate();
        let npub = keys.public_key().to_bech32().expect("encode npub");
        store
            .upsert_chat_allowlist_entry(&npub, false, true, Some("forge-only"), "npub1admin")
            .expect("insert forge-only allowlist entry");
        let auth = AuthState::new(&[], &[], store.clone());
        let challenge = auth.create_challenge();
        let verify_url = "https://news.pikachat.org/news/auth/verify";
        let event = EventBuilder::new(Kind::Custom(27235), challenge.clone())
            .tags([
                Tag::custom(TagKind::custom("u"), [verify_url]),
                Tag::custom(TagKind::custom("method"), ["POST"]),
            ])
            .sign_with_keys(&keys)
            .expect("sign event");
        let event_json = serde_json::to_string(&event).expect("serialize event");

        let (token, verified_npub, is_admin) = auth.verify_event(&event_json).expect("verify");

        assert!(!token.is_empty());
        assert_eq!(verified_npub, npub.to_lowercase());
        assert!(!is_admin);
        assert_eq!(
            store
                .validate_auth_token(&token, super::TOKEN_TTL_DAYS)
                .expect("validate token"),
            Some(npub.to_lowercase())
        );
    }
}
