#![allow(dead_code)]

use std::future::Future;
use std::path::PathBuf;

use anyhow::{anyhow, Result};
use pikahub::config::{ProfileName, ResolvedConfig};
use pikahub::{fixture, manifest::Manifest};

/// Provides relay + moq URLs for E2E tests.
///
/// In local mode (default): starts pikahub in the background with a temp state dir.
/// On Drop, calls `pikahub::fixture::down_sync` to clean up.
pub struct TestInfra {
    pub relay_url: String,
    pub moq_url: Option<String>,
    state_dir: Option<PathBuf>,
}

impl TestInfra {
    /// Start local infra via pikahub. `need_moq` controls whether moq-relay is included.
    pub fn start_local(need_moq: bool) -> Self {
        let state_dir = tempfile::tempdir().expect("tempdir for pikahub").keep();
        let profile = if need_moq {
            ProfileName::RelayMoq
        } else {
            ProfileName::Relay
        };
        let resolved = ResolvedConfig::new(
            profile,
            None,
            false,
            Some(0),
            need_moq.then_some(0),
            None,
            Some(state_dir.clone()),
        )
        .unwrap_or_else(|e| panic!("resolve pikahub config failed: {e:#}"));

        let startup: Result<Manifest> = run_async(async {
            fixture::up_background(&resolved).await?;
            let manifest = Manifest::load(&state_dir)?.ok_or_else(|| {
                anyhow!(
                    "manifest missing after pikahub startup at {}",
                    state_dir.display()
                )
            })?;
            fixture::wait(&state_dir, 30).await?;
            Ok(manifest)
        });
        let manifest = startup.unwrap_or_else(|e| panic!("start pikahub fixture failed: {e:#}"));

        let relay_url = manifest.relay_url.expect("manifest missing relay_url");
        let moq_url = manifest.moq_url;

        if need_moq && moq_url.is_none() {
            panic!("requested moq but pikahub manifest has no moq_url");
        }

        eprintln!("[TestInfra] local relay={relay_url}");
        if let Some(ref moq) = moq_url {
            eprintln!("[TestInfra] local moq={moq}");
        }

        Self {
            relay_url,
            moq_url,
            state_dir: Some(state_dir),
        }
    }

    /// Start relay-only local infra.
    pub fn start_relay() -> Self {
        Self::start_local(false)
    }

    /// Start relay + moq local infra.
    pub fn start_relay_and_moq() -> Self {
        Self::start_local(true)
    }
}

impl Drop for TestInfra {
    fn drop(&mut self) {
        if let Some(state_dir) = self.state_dir.take() {
            if let Err(e) = fixture::down_sync(&state_dir) {
                eprintln!(
                    "[TestInfra] WARNING: pikahub down failed for {}: {e:#}",
                    state_dir.display()
                );
            }
            if let Err(e) = std::fs::remove_dir_all(&state_dir) {
                eprintln!(
                    "[TestInfra] WARNING: failed to remove {}: {e}",
                    state_dir.display()
                );
            }
        }
    }
}

fn run_async<F, T>(future: F) -> T
where
    F: Future<Output = T>,
{
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("create tokio runtime for TestInfra");
    runtime.block_on(future)
}
