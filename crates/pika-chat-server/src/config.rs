use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use rand::RngCore;

#[derive(Clone)]
pub struct ChatServerConfig {
    pub bind_addr: SocketAddr,
    pub state_path: PathBuf,
    pub trust_forwarded_host: bool,
    pub session_secret: [u8; 32],
    pub session_ttl_secs: u64,
    pub ephemeral_session_secret: bool,
}

impl ChatServerConfig {
    pub fn from_env() -> Result<Self> {
        let bind_addr = std::env::var("PIKA_CHAT_SERVER_BIND")
            .unwrap_or_else(|_| "127.0.0.1:9080".to_string())
            .parse::<SocketAddr>()
            .context("parse PIKA_CHAT_SERVER_BIND")?;
        let state_path = std::env::var("PIKA_CHAT_SERVER_STATE_PATH")
            .ok()
            .map(|raw| raw.trim().to_string())
            .filter(|raw| !raw.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("data/chat-server-state.json"));
        let trust_forwarded_host = env_truthy("PIKA_CHAT_SERVER_TRUST_X_FORWARDED_HOST");
        let session_ttl_secs = std::env::var("PIKA_CHAT_SERVER_SESSION_TTL_SECS")
            .ok()
            .map(|raw| raw.parse::<u64>())
            .transpose()
            .context("parse PIKA_CHAT_SERVER_SESSION_TTL_SECS")?
            .unwrap_or(30 * 24 * 60 * 60);
        let (session_secret, ephemeral_session_secret) = load_session_secret()?;

        Ok(Self {
            bind_addr,
            state_path,
            trust_forwarded_host,
            session_secret,
            session_ttl_secs,
            ephemeral_session_secret,
        })
    }
}

fn env_truthy(name: &str) -> bool {
    std::env::var(name)
        .ok()
        .map(|value| value == "1" || value.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

fn load_session_secret() -> Result<([u8; 32], bool)> {
    let Some(raw) = std::env::var("PIKA_CHAT_SERVER_SESSION_SECRET_HEX")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    else {
        let mut secret = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut secret);
        return Ok((secret, true));
    };

    let decoded = hex::decode(&raw).context("decode PIKA_CHAT_SERVER_SESSION_SECRET_HEX")?;
    if decoded.len() != 32 {
        bail!(
            "PIKA_CHAT_SERVER_SESSION_SECRET_HEX must decode to 32 bytes, got {}",
            decoded.len()
        );
    }

    let mut secret = [0u8; 32];
    secret.copy_from_slice(&decoded);
    Ok((secret, false))
}
