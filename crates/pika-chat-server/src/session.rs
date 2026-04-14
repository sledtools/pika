use anyhow::{bail, Context, Result};
use axum::http::{header, HeaderMap};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionClaims {
    pub version: u8,
    pub npub: String,
    pub issued_at: u64,
    pub expires_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionTokenResponse {
    pub access_token: String,
    pub token_type: String,
    pub npub: String,
    pub expires_at: u64,
}

#[derive(Clone)]
pub struct SessionManager {
    secret: [u8; 32],
    ttl_secs: u64,
}

impl SessionManager {
    pub fn new(secret: [u8; 32], ttl_secs: u64) -> Self {
        Self { secret, ttl_secs }
    }

    pub fn issue_token(&self, npub: &str, now: u64) -> Result<SessionTokenResponse> {
        let claims = SessionClaims {
            version: 1,
            npub: npub.to_string(),
            issued_at: now,
            expires_at: now.saturating_add(self.ttl_secs),
        };
        let payload = serde_json::to_vec(&claims).context("serialize session claims")?;
        let signature = self.sign(&payload)?;
        let access_token = format!(
            "{}.{}",
            URL_SAFE_NO_PAD.encode(payload),
            URL_SAFE_NO_PAD.encode(signature)
        );
        Ok(SessionTokenResponse {
            access_token,
            token_type: "Bearer".to_string(),
            npub: claims.npub,
            expires_at: claims.expires_at,
        })
    }

    pub fn claims_from_bearer(&self, headers: &HeaderMap, now: u64) -> Result<SessionClaims> {
        let token = bearer_token(headers)?;
        self.verify_token(token, now)
    }

    pub fn verify_token(&self, token: &str, now: u64) -> Result<SessionClaims> {
        let (encoded_payload, encoded_signature) = token
            .split_once('.')
            .ok_or_else(|| anyhow::anyhow!("invalid session token format"))?;
        let payload = URL_SAFE_NO_PAD
            .decode(encoded_payload)
            .context("decode session payload")?;
        let signature = URL_SAFE_NO_PAD
            .decode(encoded_signature)
            .context("decode session signature")?;
        self.verify_signature(&payload, &signature)?;

        let claims: SessionClaims =
            serde_json::from_slice(&payload).context("decode session claims")?;
        if claims.version != 1 {
            bail!("unsupported session token version {}", claims.version);
        }
        if claims.expires_at <= now {
            bail!("session token expired");
        }
        Ok(claims)
    }

    fn sign(&self, payload: &[u8]) -> Result<Vec<u8>> {
        let mut mac =
            HmacSha256::new_from_slice(&self.secret).context("init session signing key")?;
        mac.update(payload);
        Ok(mac.finalize().into_bytes().to_vec())
    }

    fn verify_signature(&self, payload: &[u8], signature: &[u8]) -> Result<()> {
        let mut mac =
            HmacSha256::new_from_slice(&self.secret).context("init session signing key")?;
        mac.update(payload);
        mac.verify_slice(signature)
            .map_err(|_| anyhow::anyhow!("invalid session token signature"))
    }
}

fn bearer_token(headers: &HeaderMap) -> Result<&str> {
    let auth = headers
        .get(header::AUTHORIZATION)
        .ok_or_else(|| anyhow::anyhow!("missing Authorization header"))?;
    let auth = auth
        .to_str()
        .ok()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow::anyhow!("invalid Authorization header value"))?;
    auth.strip_prefix("Bearer ")
        .or_else(|| auth.strip_prefix("bearer "))
        .ok_or_else(|| anyhow::anyhow!("Authorization header must use Bearer scheme"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manager() -> SessionManager {
        SessionManager::new([7u8; 32], 600)
    }

    #[test]
    fn token_round_trip() {
        let manager = manager();
        let response = manager
            .issue_token("npub1alice", 1_000)
            .expect("issue token");
        let claims = manager
            .verify_token(&response.access_token, 1_100)
            .expect("verify token");

        assert_eq!(
            claims,
            SessionClaims {
                version: 1,
                npub: "npub1alice".to_string(),
                issued_at: 1_000,
                expires_at: 1_600,
            }
        );
    }

    #[test]
    fn tampered_token_is_rejected() {
        let manager = manager();
        let response = manager
            .issue_token("npub1alice", 1_000)
            .expect("issue token");
        let mut parts = response.access_token.split('.').collect::<Vec<_>>();
        let mut payload = URL_SAFE_NO_PAD
            .decode(parts[0])
            .expect("decode original payload");
        payload[0] ^= 0x01;
        let tampered_payload = URL_SAFE_NO_PAD.encode(payload);
        parts[0] = &tampered_payload;
        let tampered_token = format!("{}.{}", parts[0], parts[1]);

        let err = manager
            .verify_token(&tampered_token, 1_100)
            .expect_err("tampered token should fail");
        assert!(
            err.to_string().contains("invalid session token signature"),
            "unexpected error: {err:#}"
        );
    }

    #[test]
    fn expired_token_is_rejected() {
        let manager = manager();
        let response = manager
            .issue_token("npub1alice", 1_000)
            .expect("issue token");
        let err = manager
            .verify_token(&response.access_token, 1_600)
            .expect_err("expired token should fail");
        assert!(err.to_string().contains("session token expired"));
    }
}
