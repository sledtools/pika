#![allow(dead_code)]

use std::future::Future;
use std::path::PathBuf;
use std::time::Duration;

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
    resolved: Option<ResolvedConfig>,
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

        let startup_resolved = resolved.clone();
        let startup_state_dir = state_dir.clone();
        let startup: Result<Manifest> = run_async(async move {
            fixture::up_background(&startup_resolved).await?;
            let manifest = Manifest::load(&startup_state_dir)?.ok_or_else(|| {
                anyhow!(
                    "manifest missing after pikahub startup at {}",
                    startup_state_dir.display()
                )
            })?;
            fixture::wait(&startup_state_dir, 30).await?;
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
            resolved: Some(resolved),
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

    /// Restart all fixture components using the same resolved config.
    pub fn restart(&mut self) {
        self.restart_with_downtime(Duration::from_secs(0));
    }

    /// Restart all fixture components with an optional forced downtime.
    pub fn restart_with_downtime(&mut self, downtime: Duration) {
        let state_dir = self
            .state_dir
            .clone()
            .expect("restart requires managed state_dir");
        let resolved = self
            .resolved
            .clone()
            .expect("restart requires stored resolved config");
        let state_dir_for_restart = state_dir.clone();

        let restart: Result<()> = run_async(async move {
            fixture::down(&state_dir_for_restart).await?;
            if !downtime.is_zero() {
                tokio::time::sleep(downtime).await;
            }
            fixture::up_background(&resolved).await?;
            let manifest = Manifest::load(&state_dir_for_restart)?
                .ok_or_else(|| anyhow!("manifest missing after fixture restart"))?;
            fixture::wait(&state_dir_for_restart, 30).await?;
            eprintln!(
                "[TestInfra] restarted relay={}",
                manifest.relay_url.clone().unwrap_or_default()
            );
            if let Some(ref moq) = manifest.moq_url {
                eprintln!("[TestInfra] restarted moq={moq}");
            }
            Ok(())
        });
        restart.unwrap_or_else(|e| panic!("restart pikahub fixture failed: {e:#}"));

        let manifest = Manifest::load(&state_dir)
            .unwrap_or_else(|e| panic!("load manifest after restart failed: {e:#}"))
            .unwrap_or_else(|| panic!("manifest missing after restart at {}", state_dir.display()));
        self.relay_url = manifest.relay_url.expect("manifest missing relay_url");
        self.moq_url = manifest.moq_url;
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
    F: Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    if tokio::runtime::Handle::try_current().is_ok() {
        let join = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("create tokio runtime for TestInfra worker");
            runtime.block_on(future)
        })
        .join();
        match join {
            Ok(output) => output,
            Err(_) => panic!("tokio startup worker thread panicked"),
        }
    } else {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("create tokio runtime for TestInfra");
        runtime.block_on(future)
    }
}
