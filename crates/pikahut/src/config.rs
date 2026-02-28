use std::path::PathBuf;
use std::str::FromStr;

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

pub const DEFAULT_PORT_BASE: u16 = 19_400;
pub const DEFAULT_RELAY_PORT: u16 = DEFAULT_PORT_BASE; // 19400
pub const DEFAULT_SERVER_PORT: u16 = DEFAULT_PORT_BASE + 1; // 19401
pub const DEFAULT_MOQ_PORT: u16 = DEFAULT_PORT_BASE + 2; // 19402
pub const DEFAULT_STATE_DIR: &str = ".pikahut";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProfileName {
    Relay,
    RelayMoq,
    RelayBot,
    Backend,
    Postgres,
}

impl ProfileName {
    pub fn needs_postgres(self) -> bool {
        matches!(self, Self::Backend | Self::Postgres)
    }

    pub fn needs_relay(self) -> bool {
        matches!(
            self,
            Self::Relay | Self::RelayMoq | Self::RelayBot | Self::Backend
        )
    }

    pub fn needs_moq(self) -> bool {
        matches!(self, Self::RelayMoq | Self::Backend)
    }

    pub fn needs_server(self) -> bool {
        matches!(self, Self::Backend)
    }

    pub fn needs_bot(self) -> bool {
        matches!(self, Self::RelayBot)
    }
}

impl FromStr for ProfileName {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        match s {
            "relay" => Ok(Self::Relay),
            "relay-moq" => Ok(Self::RelayMoq),
            "relay-bot" => Ok(Self::RelayBot),
            "backend" => Ok(Self::Backend),
            "postgres" => Ok(Self::Postgres),
            _ => bail!(
                "unknown profile: {s} (expected: relay, relay-moq, relay-bot, backend, postgres)"
            ),
        }
    }
}

impl std::fmt::Display for ProfileName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Relay => write!(f, "relay"),
            Self::RelayMoq => write!(f, "relay-moq"),
            Self::RelayBot => write!(f, "relay-bot"),
            Self::Backend => write!(f, "backend"),
            Self::Postgres => write!(f, "postgres"),
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct OverlayConfig {
    pub relay: Option<RelayOverlay>,
    pub moq: Option<MoqOverlay>,
    pub server: Option<ServerOverlay>,
    pub bot: Option<BotOverlay>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct RelayOverlay {
    pub port: Option<u16>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct MoqOverlay {
    pub port: Option<u16>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ServerOverlay {
    pub port: Option<u16>,
    pub open_provisioning: Option<bool>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct BotOverlay {
    pub timeout_secs: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct ResolvedConfig {
    pub profile: ProfileName,
    pub relay_port: u16,
    pub moq_port: u16,
    pub server_port: u16,
    pub state_dir: PathBuf,
    #[allow(dead_code)]
    pub ephemeral: bool,
    pub open_provisioning: bool,
    pub bot_timeout_secs: u64,
    pub workspace_root: PathBuf,
    /// Temp dir handle -- kept alive to prevent cleanup until dropped.
    pub _ephemeral_dir: Option<std::sync::Arc<tempfile::TempDir>>,
}

impl ResolvedConfig {
    pub fn new(
        profile: ProfileName,
        overlay: Option<OverlayConfig>,
        ephemeral: bool,
        relay_port_cli: Option<u16>,
        moq_port_cli: Option<u16>,
        server_port_cli: Option<u16>,
        state_dir_cli: Option<PathBuf>,
    ) -> Result<Self> {
        let overlay = overlay.unwrap_or_default();

        // relay_port=0 is passed through to the Go relay which binds :0 natively;
        // pikahut discovers the actual port from the relay's log output.
        // For non-zero ports, verify availability and fall back to 0 (Go native :0) on conflict.
        let relay_port_pref = relay_port_cli
            .or(overlay.relay.as_ref().and_then(|r| r.port))
            .unwrap_or(DEFAULT_RELAY_PORT);
        let relay_port = if relay_port_pref != 0 {
            if std::net::TcpListener::bind(("127.0.0.1", relay_port_pref)).is_ok() {
                relay_port_pref
            } else {
                tracing::warn!(
                    "relay port {relay_port_pref} is busy, falling back to OS-assigned port"
                );
                0
            }
        } else {
            0
        };

        // moq-relay uses QUIC/UDP but we still give it a deterministic default.
        let moq_port = moq_port_cli
            .or(overlay.moq.as_ref().and_then(|m| m.port))
            .unwrap_or(DEFAULT_MOQ_PORT);
        let moq_port = resolve_port(moq_port)?;

        // pika-server doesn't support port 0 natively, so resolve to a concrete port.
        let server_port = server_port_cli
            .or(overlay.server.as_ref().and_then(|s| s.port))
            .unwrap_or(DEFAULT_SERVER_PORT);
        let server_port = resolve_port(server_port)?;

        let open_provisioning = overlay
            .server
            .as_ref()
            .and_then(|s| s.open_provisioning)
            .unwrap_or(true);

        let bot_timeout_secs = overlay
            .bot
            .as_ref()
            .and_then(|b| b.timeout_secs)
            .unwrap_or(900);

        let workspace_root = find_workspace_root()?;

        let (state_dir, ephemeral_dir) = if ephemeral {
            let tmp = tempfile::TempDir::new()?;
            let path = tmp.path().to_path_buf();
            (path, Some(std::sync::Arc::new(tmp)))
        } else {
            let dir = state_dir_cli.unwrap_or_else(|| workspace_root.join(DEFAULT_STATE_DIR));
            (dir, None)
        };

        Ok(Self {
            profile,
            relay_port,
            moq_port,
            server_port,
            state_dir,
            ephemeral,
            open_provisioning,
            bot_timeout_secs,
            workspace_root,
            _ephemeral_dir: ephemeral_dir,
        })
    }

    pub fn pgdata(&self) -> PathBuf {
        self.state_dir.join("pgdata")
    }

    pub fn relay_data_dir(&self) -> PathBuf {
        self.state_dir.join("relay-data")
    }

    pub fn relay_media_dir(&self) -> PathBuf {
        self.state_dir.join("relay-media")
    }

    pub fn server_state_dir(&self) -> PathBuf {
        self.state_dir.join("server")
    }

    pub fn identity_json(&self) -> PathBuf {
        self.server_state_dir().join("identity.json")
    }

    pub fn server_url(&self) -> String {
        format!("http://localhost:{}", self.server_port)
    }
}

/// Try to bind the preferred port; if it's taken, fall back to an OS-assigned random port.
/// Port 0 always goes straight to random.
fn resolve_port(preferred: u16) -> Result<u16> {
    if preferred != 0 {
        if let Ok(listener) = std::net::TcpListener::bind(("127.0.0.1", preferred)) {
            drop(listener);
            return Ok(preferred);
        }
        tracing::warn!("port {preferred} is busy, picking a random free port instead");
    }
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    Ok(listener.local_addr()?.port())
}

/// Walk up from CWD looking for the workspace Cargo.toml (contains [workspace]).
pub fn find_workspace_root() -> Result<PathBuf> {
    let mut dir = std::env::current_dir()?;
    loop {
        let candidate = dir.join("Cargo.toml");
        if candidate.exists() {
            let content = std::fs::read_to_string(&candidate).unwrap_or_default();
            if content.contains("[workspace]") {
                return Ok(dir);
            }
        }
        if !dir.pop() {
            bail!(
                "could not find workspace Cargo.toml walking up from {}",
                std::env::current_dir()?.display()
            );
        }
    }
}

/// Resolve the state directory. If no explicit path is given, defaults to
/// `<workspace_root>/.pikahut` so that down/status/env/logs/exec/wait
/// work regardless of CWD.
pub fn resolve_state_dir(state_dir: Option<PathBuf>) -> Result<PathBuf> {
    match state_dir {
        Some(p) => Ok(p),
        None => {
            let root = find_workspace_root()?;
            Ok(root.join(DEFAULT_STATE_DIR))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_parse_valid() {
        assert_eq!("relay".parse::<ProfileName>().unwrap(), ProfileName::Relay);
        assert_eq!(
            "relay-moq".parse::<ProfileName>().unwrap(),
            ProfileName::RelayMoq
        );
        assert_eq!(
            "relay-bot".parse::<ProfileName>().unwrap(),
            ProfileName::RelayBot
        );
        assert_eq!(
            "backend".parse::<ProfileName>().unwrap(),
            ProfileName::Backend
        );
        assert_eq!(
            "postgres".parse::<ProfileName>().unwrap(),
            ProfileName::Postgres
        );
    }

    #[test]
    fn profile_parse_invalid() {
        assert!("unknown".parse::<ProfileName>().is_err());
    }

    #[test]
    fn profile_display_round_trip() {
        for name in &["relay", "relay-moq", "relay-bot", "backend", "postgres"] {
            let parsed: ProfileName = name.parse().unwrap();
            assert_eq!(&parsed.to_string(), name);
        }
    }

    #[test]
    fn profile_component_needs() {
        assert!(!ProfileName::Relay.needs_postgres());
        assert!(ProfileName::Relay.needs_relay());
        assert!(!ProfileName::Relay.needs_moq());
        assert!(!ProfileName::Relay.needs_server());
        assert!(!ProfileName::Relay.needs_bot());

        assert!(!ProfileName::RelayMoq.needs_postgres());
        assert!(ProfileName::RelayMoq.needs_relay());
        assert!(ProfileName::RelayMoq.needs_moq());
        assert!(!ProfileName::RelayMoq.needs_server());
        assert!(!ProfileName::RelayMoq.needs_bot());

        assert!(ProfileName::Backend.needs_postgres());
        assert!(ProfileName::Backend.needs_relay());
        assert!(ProfileName::Backend.needs_moq());
        assert!(ProfileName::Backend.needs_server());
        assert!(!ProfileName::Backend.needs_bot());

        assert!(!ProfileName::RelayBot.needs_postgres());
        assert!(ProfileName::RelayBot.needs_relay());
        assert!(!ProfileName::RelayBot.needs_moq());
        assert!(!ProfileName::RelayBot.needs_server());
        assert!(ProfileName::RelayBot.needs_bot());

        assert!(ProfileName::Postgres.needs_postgres());
        assert!(!ProfileName::Postgres.needs_relay());
        assert!(!ProfileName::Postgres.needs_moq());
        assert!(!ProfileName::Postgres.needs_server());
        assert!(!ProfileName::Postgres.needs_bot());
    }

    #[test]
    fn overlay_config_deserializes_empty() {
        let cfg: OverlayConfig = toml::from_str("").unwrap();
        assert!(cfg.relay.is_none());
        assert!(cfg.server.is_none());
        assert!(cfg.bot.is_none());
    }

    #[test]
    fn overlay_config_deserializes_full() {
        let toml_str = r#"
[relay]
port = 4444

[server]
port = 9090
open_provisioning = false

[bot]
timeout_secs = 120
"#;
        let cfg: OverlayConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.relay.unwrap().port, Some(4444));
        let server = cfg.server.unwrap();
        assert_eq!(server.port, Some(9090));
        assert_eq!(server.open_provisioning, Some(false));
        assert_eq!(cfg.bot.unwrap().timeout_secs, Some(120));
    }
}
