use anyhow::Context;
use pika_chat_server::{router, AppState, ChatServerConfig, SessionManager};
use tracing::{info, warn};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let config = ChatServerConfig::from_env()?;
    if config.ephemeral_session_secret {
        warn!("PIKA_CHAT_SERVER_SESSION_SECRET_HEX not set; using an ephemeral session secret");
    }

    let state = AppState {
        sessions: SessionManager::new(config.session_secret, config.session_ttl_secs),
        trust_forwarded_host: config.trust_forwarded_host,
    };

    let listener = tokio::net::TcpListener::bind(config.bind_addr)
        .await
        .with_context(|| format!("bind chat server on {}", config.bind_addr))?;
    info!(addr = %config.bind_addr, "pika-chat-server listening");

    axum::serve(listener, router(state))
        .await
        .context("serve pika-chat-server")
}
