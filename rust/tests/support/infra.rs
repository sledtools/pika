#![allow(dead_code)]

use std::future::Future;
use std::path::PathBuf;

use anyhow::{anyhow, Result};
use pikahut::config::{ProfileName, ResolvedConfig};
use pikahut::{fixture, manifest::Manifest};

/// Provides relay + moq URLs for E2E tests.
///
/// In local mode (default): starts pikahut in the background with a temp state dir.
/// On Drop, calls `pikahut::fixture::down_sync` to clean up.
pub struct TestInfra {
    pub relay_url: String,
    pub moq_url: Option<String>,
    pub server_url: Option<String>,
    state_dir: Option<PathBuf>,
}

impl TestInfra {
    /// Pre-build pika-server so `cargo run -p pika-server` doesn't compile during the
    /// health-check timeout window.
    fn prebuild_server() {
        if let Ok(server_bin) = std::env::var("PIKA_FIXTURE_SERVER_CMD") {
            let path = std::path::Path::new(&server_bin);
            assert!(
                path.exists(),
                "PIKA_FIXTURE_SERVER_CMD={} does not exist",
                path.display()
            );
            eprintln!(
                "[TestInfra] using staged pika-server binary at {}",
                path.display()
            );
            return;
        }
        eprintln!("[TestInfra] pre-building pika-server...");
        let status = std::process::Command::new("cargo")
            .args(["build", "-p", "pika-server"])
            .status()
            .expect("cargo build -p pika-server");
        assert!(status.success(), "pika-server pre-build failed");
        eprintln!("[TestInfra] pika-server pre-build done");
    }

    /// Start local infra via pikahut.
    fn start_with_profile(profile: ProfileName, need_moq: bool) -> Self {
        if profile.needs_server() {
            Self::prebuild_server();
        }
        let state_dir = tempfile::tempdir().expect("tempdir for pikahut").keep();
        let need_server = profile.needs_server();
        let resolved = ResolvedConfig::new(
            profile,
            None,
            false,
            Some(0),
            need_moq.then_some(0),
            need_server.then_some(0),
            Some(state_dir.clone()),
        )
        .unwrap_or_else(|e| panic!("resolve pikahut config failed: {e:#}"));

        let startup_resolved = resolved.clone();
        let startup_state_dir = state_dir.clone();
        let wait_secs = if need_server { 120 } else { 30 };
        let startup: Result<Manifest> = run_async(async move {
            fixture::up_background(&startup_resolved).await?;
            let manifest = Manifest::load(&startup_state_dir)?.ok_or_else(|| {
                anyhow!(
                    "manifest missing after pikahut startup at {}",
                    startup_state_dir.display()
                )
            })?;
            fixture::wait(&startup_state_dir, wait_secs).await?;
            Ok(manifest)
        });
        let manifest = startup.unwrap_or_else(|e| {
            // Dump component logs to help diagnose startup failures.
            for log in &["server.log", "relay.log", "postgres.log"] {
                let path = state_dir.join(log);
                if let Ok(content) = std::fs::read_to_string(&path) {
                    eprintln!("[TestInfra] === {log} ===\n{content}");
                }
            }
            panic!("start pikahut fixture failed: {e:#}");
        });

        let relay_url = manifest.relay_url.expect("manifest missing relay_url");
        let moq_url = manifest.moq_url;
        let server_url = manifest.server_url;

        if need_moq && moq_url.is_none() {
            panic!("requested moq but pikahut manifest has no moq_url");
        }
        if need_server && server_url.is_none() {
            panic!("requested server but pikahut manifest has no server_url");
        }

        eprintln!("[TestInfra] local relay={relay_url}");
        if let Some(ref moq) = moq_url {
            eprintln!("[TestInfra] local moq={moq}");
        }
        if let Some(ref srv) = server_url {
            eprintln!("[TestInfra] local server={srv}");
        }

        Self {
            relay_url,
            moq_url,
            server_url,
            state_dir: Some(state_dir),
        }
    }

    /// Start local infra via pikahut. `need_moq` controls whether moq-relay is included.
    pub fn start_local(need_moq: bool) -> Self {
        let profile = if need_moq {
            ProfileName::RelayMoq
        } else {
            ProfileName::Relay
        };
        Self::start_with_profile(profile, need_moq)
    }

    /// Start relay-only local infra.
    pub fn start_relay() -> Self {
        Self::start_local(false)
    }

    /// Start relay + moq local infra.
    pub fn start_relay_and_moq() -> Self {
        Self::start_local(true)
    }

    /// Start relay + postgres + pika-server (no moq).
    pub fn start_backend() -> Self {
        Self::start_with_profile(ProfileName::RelayServer, false)
    }
}

impl Drop for TestInfra {
    fn drop(&mut self) {
        if let Some(state_dir) = self.state_dir.take() {
            if let Err(e) = fixture::down_sync(&state_dir) {
                eprintln!(
                    "[TestInfra] WARNING: pikahut down failed for {}: {e:#}",
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
