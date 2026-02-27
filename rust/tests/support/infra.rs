#![allow(dead_code)]

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Provides relay + moq URLs for E2E tests.
///
/// In local mode (default): starts pikahub in the background with a temp state dir.
/// On Drop, runs `pikahub down` to clean up.
pub struct TestInfra {
    pub relay_url: String,
    pub moq_url: Option<String>,
    state_dir: Option<PathBuf>,
}

impl TestInfra {
    /// Start local infra via pikahub.  `need_moq` controls whether moq-relay is included.
    pub fn start_local(need_moq: bool) -> Self {
        let pikahub = pikahub_binary();
        let state_dir = tempfile::tempdir().expect("tempdir for pikahub").keep();
        let profile = if need_moq { "backend" } else { "relay" };

        // We use --relay-port 0 so pikahub picks a free port.
        let mut cmd = Command::new(&pikahub);
        cmd.arg("up")
            .arg("--profile")
            .arg(profile)
            .arg("--background")
            .arg("--relay-port")
            .arg("0")
            .arg("--state-dir")
            .arg(&state_dir);
        if need_moq {
            cmd.arg("--moq-port").arg("0");
        }
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

        let output = cmd
            .output()
            .unwrap_or_else(|e| panic!("pikahub up failed: {e}"));
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            panic!("pikahub up --profile {profile} failed:\nstdout: {stdout}\nstderr: {stderr}");
        }

        // Read manifest to get URLs.
        let manifest_path = state_dir.join("manifest.json");
        let manifest_raw = std::fs::read_to_string(&manifest_path)
            .unwrap_or_else(|e| panic!("read manifest at {}: {e}", manifest_path.display()));
        let manifest: serde_json::Value =
            serde_json::from_str(&manifest_raw).unwrap_or_else(|e| panic!("parse manifest: {e}"));

        let relay_url = manifest["relay_url"]
            .as_str()
            .expect("manifest missing relay_url")
            .to_string();

        let moq_url = manifest["moq_url"].as_str().map(|s| s.to_string());

        if need_moq && moq_url.is_none() {
            panic!("requested moq but pikahub manifest has no moq_url");
        }

        // Wait for relay to be healthy.
        wait_for_relay(&relay_url, Duration::from_secs(30));

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
        if let Some(ref state_dir) = self.state_dir {
            let pikahub = pikahub_binary();
            let _ = Command::new(&pikahub)
                .arg("down")
                .arg("--state-dir")
                .arg(state_dir)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
            // Best-effort cleanup of state dir.
            let _ = std::fs::remove_dir_all(state_dir);
        }
    }
}

fn pikahub_binary() -> String {
    let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let bin = repo_root.join("target/debug/pikahub");
    if bin.exists() {
        return bin.to_string_lossy().to_string();
    }
    // Fall back to PATH.
    "pikahub".to_string()
}

fn wait_for_relay(url: &str, timeout: Duration) {
    // Extract host:port from ws://host:port
    let addr = url
        .trim_start_matches("ws://")
        .trim_start_matches("wss://")
        .trim_end_matches('/');
    let start = Instant::now();
    while start.elapsed() < timeout {
        if let Ok(mut stream) = TcpStream::connect(addr) {
            stream.set_read_timeout(Some(Duration::from_secs(2))).ok();
            let req = format!("GET / HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
            if stream.write_all(req.as_bytes()).is_ok() {
                let mut buf = [0u8; 256];
                if let Ok(n) = stream.read(&mut buf) {
                    if n > 0 {
                        return;
                    }
                }
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("relay at {url} not healthy within {timeout:?}");
}
