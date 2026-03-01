use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};

use crate::config::{OverlayConfig, ProfileName, ResolvedConfig};
use crate::fixture as runtime_fixture;
use crate::manifest::Manifest;

use super::TestContext;

/// Declarative fixture configuration used by integration tests.
///
/// # Examples
///
/// ```no_run
/// use pikahut::config::ProfileName;
/// use pikahut::testing::FixtureSpec;
///
/// let spec = FixtureSpec::builder(ProfileName::RelayBot)
///     .wait_timeout_secs(45)
///     .build();
///
/// assert_eq!(spec.wait_timeout_secs, 45);
/// ```
#[derive(Debug, Clone)]
pub struct FixtureSpec {
    pub profile: ProfileName,
    pub overlay: Option<OverlayConfig>,
    pub relay_port: Option<u16>,
    pub moq_port: Option<u16>,
    pub server_port: Option<u16>,
    pub wait_timeout_secs: u64,
}

impl FixtureSpec {
    pub fn builder(profile: ProfileName) -> FixtureBuilder {
        FixtureBuilder::new(profile)
    }
}

/// Builder for [`FixtureSpec`].
#[derive(Debug, Clone)]
pub struct FixtureBuilder {
    profile: ProfileName,
    overlay: Option<OverlayConfig>,
    relay_port: Option<u16>,
    moq_port: Option<u16>,
    server_port: Option<u16>,
    wait_timeout_secs: u64,
}

impl FixtureBuilder {
    pub fn new(profile: ProfileName) -> Self {
        Self {
            profile,
            overlay: None,
            relay_port: Some(0),
            moq_port: None,
            server_port: None,
            wait_timeout_secs: 60,
        }
    }

    pub fn overlay(mut self, overlay: OverlayConfig) -> Self {
        self.overlay = Some(overlay);
        self
    }

    pub fn relay_port(mut self, relay_port: u16) -> Self {
        self.relay_port = Some(relay_port);
        self
    }

    pub fn relay_port_opt(mut self, relay_port: Option<u16>) -> Self {
        self.relay_port = relay_port;
        self
    }

    pub fn moq_port(mut self, moq_port: u16) -> Self {
        self.moq_port = Some(moq_port);
        self
    }

    pub fn moq_port_opt(mut self, moq_port: Option<u16>) -> Self {
        self.moq_port = moq_port;
        self
    }

    pub fn server_port(mut self, server_port: u16) -> Self {
        self.server_port = Some(server_port);
        self
    }

    pub fn server_port_opt(mut self, server_port: Option<u16>) -> Self {
        self.server_port = server_port;
        self
    }

    pub fn wait_timeout_secs(mut self, wait_timeout_secs: u64) -> Self {
        self.wait_timeout_secs = wait_timeout_secs;
        self
    }

    pub fn build(self) -> FixtureSpec {
        FixtureSpec {
            profile: self.profile,
            overlay: self.overlay,
            relay_port: self.relay_port,
            moq_port: self.moq_port,
            server_port: self.server_port,
            wait_timeout_secs: self.wait_timeout_secs,
        }
    }
}

/// Running fixture handle with manifest/env access and idempotent teardown.
///
/// # Examples
///
/// ```no_run
/// use pikahut::config::ProfileName;
/// use pikahut::testing::{FixtureSpec, TestContext, start_fixture};
///
/// # async fn demo() -> anyhow::Result<()> {
/// let context = TestContext::builder("example").build()?;
/// let spec = FixtureSpec::builder(ProfileName::Relay).build();
/// let mut fixture = start_fixture(&context, &spec).await?;
/// let _relay_url = fixture.manifest().relay_url.clone();
/// fixture.teardown().await?;
/// # Ok(())
/// # }
/// ```
#[derive(Debug)]
pub struct FixtureHandle {
    state_dir: PathBuf,
    manifest: Manifest,
    started: bool,
}

impl FixtureHandle {
    pub async fn start(context: &TestContext, spec: &FixtureSpec) -> Result<Self> {
        let resolved = ResolvedConfig::new(
            spec.profile,
            spec.overlay.clone(),
            false,
            spec.relay_port,
            spec.moq_port,
            spec.server_port,
            Some(context.state_dir().to_path_buf()),
        )
        .context("resolve fixture config")?;

        runtime_fixture::up_background(&resolved)
            .await
            .context("start fixture components")?;

        runtime_fixture::wait(context.state_dir(), spec.wait_timeout_secs)
            .await
            .with_context(|| {
                format!(
                    "wait for fixture readiness (timeout={}s)",
                    spec.wait_timeout_secs
                )
            })?;

        let manifest = Manifest::load(context.state_dir())?
            .ok_or_else(|| anyhow!("fixture manifest missing after startup"))?;

        Ok(Self {
            state_dir: context.state_dir().to_path_buf(),
            manifest,
            started: true,
        })
    }

    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    pub fn relay_url(&self) -> Option<&str> {
        self.manifest.relay_url.as_deref()
    }

    pub fn server_url(&self) -> Option<&str> {
        self.manifest.server_url.as_deref()
    }

    pub fn bot_npub(&self) -> Option<&str> {
        self.manifest.bot_npub.as_deref()
    }

    pub fn env_map(&self) -> BTreeMap<String, String> {
        self.manifest.env_exports().into_iter().collect()
    }

    pub fn state_dir(&self) -> &Path {
        &self.state_dir
    }

    pub async fn wait_healthy(&self, timeout_secs: u64) -> Result<()> {
        runtime_fixture::wait(&self.state_dir, timeout_secs).await
    }

    pub async fn teardown(&mut self) -> Result<()> {
        if self.started {
            runtime_fixture::down(&self.state_dir)
                .await
                .context("teardown fixture")?;
            self.started = false;
        }
        Ok(())
    }
}

impl Drop for FixtureHandle {
    fn drop(&mut self) {
        if !self.started {
            return;
        }

        let _ = runtime_fixture::down_sync(&self.state_dir);
        self.started = false;
    }
}

pub async fn start_fixture(context: &TestContext, spec: &FixtureSpec) -> Result<FixtureHandle> {
    FixtureHandle::start(context, spec).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::BotOverlay;

    fn sample_manifest(state_dir: &Path) -> Manifest {
        Manifest {
            profile: "relay".to_string(),
            relay_url: Some("ws://127.0.0.1:7777".to_string()),
            relay_pid: None,
            relay_start_time: None,
            moq_url: None,
            moq_pid: None,
            moq_start_time: None,
            server_url: Some("http://127.0.0.1:18080".to_string()),
            server_pid: None,
            server_start_time: None,
            server_pubkey_hex: None,
            database_url: None,
            postgres_pid: None,
            bot_npub: Some("npub1test".to_string()),
            bot_pubkey_hex: None,
            bot_pid: None,
            bot_start_time: None,
            state_dir: state_dir.to_path_buf(),
            started_at: "2026-03-01T00:00:00Z".to_string(),
        }
    }

    #[tokio::test]
    async fn teardown_is_idempotent() {
        let state = tempfile::tempdir().unwrap();
        let mut handle = FixtureHandle {
            state_dir: state.path().to_path_buf(),
            manifest: sample_manifest(state.path()),
            started: true,
        };

        handle.teardown().await.unwrap();
        handle.teardown().await.unwrap();
        assert!(!handle.started);
    }

    #[test]
    fn manifest_accessors_are_available_without_cli_parsing() {
        let state = tempfile::tempdir().unwrap();
        let handle = FixtureHandle {
            state_dir: state.path().to_path_buf(),
            manifest: sample_manifest(state.path()),
            started: false,
        };

        assert_eq!(handle.relay_url(), Some("ws://127.0.0.1:7777"));
        assert_eq!(handle.server_url(), Some("http://127.0.0.1:18080"));
        assert_eq!(handle.bot_npub(), Some("npub1test"));
        assert_eq!(
            handle.env_map().get("RELAY_EU").map(String::as_str),
            Some("ws://127.0.0.1:7777")
        );
    }

    #[test]
    fn fixture_spec_models_relay_defaults() {
        let spec = FixtureSpec::builder(ProfileName::Relay).build();
        assert_eq!(spec.profile, ProfileName::Relay);
        assert_eq!(spec.relay_port, Some(0));
        assert_eq!(spec.moq_port, None);
        assert_eq!(spec.server_port, None);
        assert_eq!(spec.wait_timeout_secs, 60);
    }

    #[test]
    fn fixture_spec_models_relay_bot_overlay_and_timeout() {
        let overlay = OverlayConfig {
            bot: Some(BotOverlay {
                timeout_secs: Some(1200),
            }),
            ..OverlayConfig::default()
        };
        let spec = FixtureSpec::builder(ProfileName::RelayBot)
            .overlay(overlay.clone())
            .wait_timeout_secs(120)
            .build();

        assert_eq!(spec.profile, ProfileName::RelayBot);
        let timeout = spec
            .overlay
            .as_ref()
            .and_then(|value| value.bot.as_ref())
            .and_then(|bot| bot.timeout_secs);
        assert_eq!(timeout, Some(1200));
        assert_eq!(spec.wait_timeout_secs, 120);
    }

    #[test]
    fn fixture_spec_models_backend_with_explicit_ports() {
        let spec = FixtureSpec::builder(ProfileName::Backend)
            .relay_port(17777)
            .moq_port(4443)
            .server_port(18080)
            .wait_timeout_secs(90)
            .build();

        assert_eq!(spec.profile, ProfileName::Backend);
        assert_eq!(spec.relay_port, Some(17777));
        assert_eq!(spec.moq_port, Some(4443));
        assert_eq!(spec.server_port, Some(18080));
        assert_eq!(spec.wait_timeout_secs, 90);
    }

    #[test]
    fn fixture_spec_allows_optional_port_overrides() {
        let spec = FixtureSpec::builder(ProfileName::RelayBot)
            .relay_port_opt(None)
            .moq_port_opt(Some(3443))
            .server_port_opt(Some(28080))
            .build();

        assert_eq!(spec.relay_port, None);
        assert_eq!(spec.moq_port, Some(3443));
        assert_eq!(spec.server_port, Some(28080));
    }
}
