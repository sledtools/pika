use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use nostr::event::Event;
use nostr::key::PublicKey;
use nostr::nips::nip19::ToBech32;
use rand::Rng;

const CHALLENGE_TTL: Duration = Duration::from_secs(120);
const TOKEN_TTL: Duration = Duration::from_secs(3600 * 24);

pub struct AuthState {
    challenges: Mutex<HashMap<String, (String, Instant)>>,
    tokens: Mutex<HashMap<String, (String, Instant)>>,
    allowed_npubs_hex: Vec<String>,
}

impl AuthState {
    pub fn new(allowed_npubs: &[String]) -> Self {
        let allowed_npubs_hex = allowed_npubs
            .iter()
            .filter_map(|npub| PublicKey::parse(npub).ok().map(|pk| pk.to_hex()))
            .collect();

        Self {
            challenges: Mutex::new(HashMap::new()),
            tokens: Mutex::new(HashMap::new()),
            allowed_npubs_hex,
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

    pub fn verify_event(&self, event_json: &str) -> Result<(String, String), String> {
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

        let pubkey_hex = event.pubkey.to_hex();

        // Check allowlist
        if !self.allowed_npubs_hex.is_empty() && !self.allowed_npubs_hex.contains(&pubkey_hex) {
            return Err("pubkey not in allowed list".to_string());
        }

        // Check challenge nonce is in content
        let content = event.content.to_string();
        {
            let mut challenges = self.challenges.lock().unwrap();
            if challenges.remove(&content).is_none() {
                return Err("challenge nonce not found or expired".to_string());
            }
        }

        // Issue token
        let token = hex::encode(rand::thread_rng().gen::<[u8; 32]>());
        let npub = PublicKey::from_hex(&pubkey_hex)
            .ok()
            .and_then(|pk| pk.to_bech32().ok())
            .unwrap_or(pubkey_hex.clone());
        {
            let mut tokens = self.tokens.lock().unwrap();
            tokens.retain(|_, (_, created)| created.elapsed() < TOKEN_TTL);
            tokens.insert(token.clone(), (npub.clone(), Instant::now()));
        }

        Ok((token, npub))
    }

    pub fn validate_token(&self, token: &str) -> Option<String> {
        let tokens = self.tokens.lock().unwrap();
        tokens.get(token).and_then(|(npub, created)| {
            if created.elapsed() < TOKEN_TTL {
                Some(npub.clone())
            } else {
                None
            }
        })
    }

    pub fn chat_enabled(&self) -> bool {
        !self.allowed_npubs_hex.is_empty()
    }
}
