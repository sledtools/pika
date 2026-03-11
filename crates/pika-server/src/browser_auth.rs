use anyhow::Context;
use axum::http::{header, HeaderMap, HeaderValue};
use axum::response::Response;
use base64::Engine;
use hmac::{Hmac, Mac};
use nostr_sdk::prelude::Keys;
use nostr_sdk::ToBech32;
use rand::Rng;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

const TEST_NSEC_ENV: &str = "PIKA_TEST_NSEC";
const CHALLENGE_TTL_SECS: i64 = 120;

#[derive(Clone, Debug)]
pub struct BrowserAuthConfig {
    session_secret: Vec<u8>,
    pub dev_mode: bool,
    pub dev_npub: Option<String>,
    cookie_secure: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct SessionPayload {
    kind: String,
    npub: String,
    exp: i64,
}

#[derive(Debug, Serialize, Deserialize)]
struct ChallengePayload {
    kind: String,
    nonce: String,
    exp: i64,
}

impl BrowserAuthConfig {
    pub fn from_env(
        session_secret_env: &str,
        dev_mode_env: &str,
        cookie_secure_env: &str,
    ) -> anyhow::Result<Self> {
        let dev_mode = env_truthy(dev_mode_env);
        let dev_npub = if dev_mode {
            std::env::var(TEST_NSEC_ENV)
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
                .map(|nsec| {
                    Keys::parse(&nsec)
                        .context("parse PIKA_TEST_NSEC")
                        .and_then(|keys| {
                            keys.public_key()
                                .to_bech32()
                                .context("derive npub from PIKA_TEST_NSEC")
                        })
                })
                .transpose()?
        } else {
            None
        };

        let session_secret = std::env::var(session_secret_env)
            .with_context(|| format!("missing {session_secret_env}"))?
            .into_bytes();
        let cookie_secure = std::env::var(cookie_secure_env)
            .ok()
            .map(|value| value == "1" || value.eq_ignore_ascii_case("true"))
            .unwrap_or(!dev_mode);

        Self::new(session_secret, cookie_secure, dev_mode, dev_npub)
    }

    pub fn new(
        session_secret: Vec<u8>,
        cookie_secure: bool,
        dev_mode: bool,
        dev_npub: Option<String>,
    ) -> anyhow::Result<Self> {
        anyhow::ensure!(
            session_secret.len() >= 16,
            "session secret must be at least 16 bytes"
        );
        Ok(Self {
            session_secret,
            dev_mode,
            dev_npub,
            cookie_secure,
        })
    }

    pub fn issue_challenge(&self, kind: &str) -> anyhow::Result<String> {
        let payload = ChallengePayload {
            kind: kind.to_string(),
            nonce: hex::encode(rand::thread_rng().gen::<[u8; 16]>()),
            exp: now_unix() + CHALLENGE_TTL_SECS,
        };
        sign_token(&self.session_secret, &payload)
    }

    pub fn verify_challenge(&self, token: &str, expected_kind: &str) -> anyhow::Result<()> {
        let payload: ChallengePayload = verify_token(&self.session_secret, token)?;
        anyhow::ensure!(
            payload.kind == expected_kind,
            "invalid challenge payload kind"
        );
        anyhow::ensure!(payload.exp >= now_unix(), "challenge expired");
        Ok(())
    }

    pub fn issue_session_token(
        &self,
        session_kind: &str,
        npub: &str,
        ttl_secs: i64,
    ) -> anyhow::Result<String> {
        let payload = SessionPayload {
            kind: session_kind.to_string(),
            npub: npub.to_string(),
            exp: now_unix() + ttl_secs,
        };
        sign_token(&self.session_secret, &payload)
    }

    pub fn verify_session_token(&self, token: &str, expected_kind: &str) -> anyhow::Result<String> {
        let payload: SessionPayload = verify_token(&self.session_secret, token)?;
        anyhow::ensure!(
            payload.kind == expected_kind,
            "invalid session payload kind"
        );
        anyhow::ensure!(payload.exp >= now_unix(), "session expired");
        Ok(payload.npub)
    }

    pub fn set_session_cookie(
        &self,
        response: &mut Response,
        cookie_name: &str,
        token: &str,
        ttl_secs: i64,
    ) -> anyhow::Result<()> {
        let secure = if self.cookie_secure { "; Secure" } else { "" };
        let value = format!(
            "{cookie_name}={token}; Path=/; HttpOnly; SameSite=Lax{secure}; Max-Age={ttl_secs}"
        );
        response.headers_mut().append(
            header::SET_COOKIE,
            HeaderValue::from_str(&value).context("build Set-Cookie header")?,
        );
        Ok(())
    }

    pub fn clear_session_cookie(
        &self,
        response: &mut Response,
        cookie_name: &str,
    ) -> anyhow::Result<()> {
        let secure = if self.cookie_secure { "; Secure" } else { "" };
        let value = format!("{cookie_name}=; Path=/; HttpOnly; SameSite=Lax{secure}; Max-Age=0");
        response.headers_mut().append(
            header::SET_COOKIE,
            HeaderValue::from_str(&value).context("build clear Set-Cookie header")?,
        );
        Ok(())
    }

    pub fn session_npub_from_headers(
        &self,
        headers: &HeaderMap,
        cookie_name: &str,
        session_kind: &str,
    ) -> Option<String> {
        let cookie = headers.get(header::COOKIE)?.to_str().ok()?;
        for pair in cookie.split(';') {
            let Some((name, value)) = pair.trim().split_once('=') else {
                continue;
            };
            if name == cookie_name {
                if let Ok(npub) = self.verify_session_token(value, session_kind) {
                    return Some(npub);
                }
            }
        }
        None
    }
}

fn env_truthy(name: &str) -> bool {
    std::env::var(name)
        .ok()
        .map(|value| value == "1" || value.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

fn now_unix() -> i64 {
    chrono::Utc::now().timestamp()
}

fn sign_token<T: Serialize>(secret: &[u8], payload: &T) -> anyhow::Result<String> {
    let body = serde_json::to_vec(payload)?;
    let body_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(body);
    let mut mac = HmacSha256::new_from_slice(secret).context("init hmac")?;
    mac.update(body_b64.as_bytes());
    let sig_b64 =
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());
    Ok(format!("{body_b64}.{sig_b64}"))
}

fn verify_token<T: DeserializeOwned>(secret: &[u8], token: &str) -> anyhow::Result<T> {
    let (body_b64, sig_b64) = token
        .split_once('.')
        .ok_or_else(|| anyhow::anyhow!("invalid token format"))?;
    let mut mac = HmacSha256::new_from_slice(secret).context("init hmac")?;
    mac.update(body_b64.as_bytes());
    let actual = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(sig_b64)
        .context("decode token signature")?;
    mac.verify_slice(&actual)
        .map_err(|_| anyhow::anyhow!("invalid token signature"))?;

    let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(body_b64)
        .context("decode token payload")?;
    let parsed = serde_json::from_slice(&payload).context("decode token JSON payload")?;
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::header;

    fn test_browser_auth_config() -> BrowserAuthConfig {
        BrowserAuthConfig::new(
            b"0123456789abcdef0123456789abcdef".to_vec(),
            true,
            false,
            None,
        )
        .expect("test browser auth config")
    }

    #[test]
    fn session_cookie_parsing_skips_malformed_pairs() {
        let config = test_browser_auth_config();
        let npub = "npub1zxu639qym0esxnn7rzrt48wycmfhdu3e5yvzwx7ja3t84zyc2r8qz8cx2y";
        let token = config
            .issue_session_token("customer_session", npub, 123)
            .expect("issue session token");
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            format!("broken-cookie; pika_customer_session={token}")
                .parse()
                .expect("cookie header"),
        );

        assert_eq!(
            config.session_npub_from_headers(&headers, "pika_customer_session", "customer_session"),
            Some(npub.to_string())
        );
    }

    #[test]
    fn verify_token_rejects_tampered_signature() {
        let config = test_browser_auth_config();
        let token = config
            .issue_session_token(
                "customer_session",
                "npub1zxu639qym0esxnn7rzrt48wycmfhdu3e5yvzwx7ja3t84zyc2r8qz8cx2y",
                123,
            )
            .expect("issue session token");
        let (body, sig) = token.split_once('.').expect("token parts");
        let mut tampered = sig.as_bytes().to_vec();
        tampered[0] = if tampered[0] == b'a' { b'b' } else { b'a' };
        let tampered_sig = String::from_utf8(tampered).expect("tampered signature utf8");

        let err = verify_token::<SessionPayload>(
            &config.session_secret,
            &format!("{body}.{tampered_sig}"),
        )
        .expect_err("tampered signature must fail");
        assert!(err.to_string().contains("invalid token signature"));
    }
}
