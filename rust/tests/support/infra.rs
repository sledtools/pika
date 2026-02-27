#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;

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
        let profile = if need_moq { "relay-moq" } else { "relay" };

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

        // Use pikahub wait for reliable health checking instead of manual TCP probe.
        let wait_output = Command::new(&pikahub)
            .arg("wait")
            .arg("--timeout")
            .arg("30")
            .arg("--state-dir")
            .arg(&state_dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .unwrap_or_else(|e| panic!("pikahub wait failed: {e}"));
        if !wait_output.status.success() {
            let stderr = String::from_utf8_lossy(&wait_output.stderr);
            panic!(
                "pikahub wait --state-dir {} failed:\n{stderr}",
                state_dir.display()
            );
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
        if let Some(ref state_dir) = self.state_dir {
            let pikahub = pikahub_binary();
            let status = Command::new(&pikahub)
                .arg("down")
                .arg("--state-dir")
                .arg(state_dir)
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .status();
            if let Ok(s) = &status {
                if !s.success() {
                    eprintln!(
                        "[TestInfra] WARNING: pikahub down failed (exit {})",
                        s.code().unwrap_or(-1)
                    );
                }
            }
            if let Err(e) = std::fs::remove_dir_all(state_dir) {
                eprintln!(
                    "[TestInfra] WARNING: failed to remove {}: {e}",
                    state_dir.display()
                );
            }
        }
    }
}

fn pikahub_binary() -> String {
    static PIKAHUB_BIN: OnceLock<String> = OnceLock::new();
    PIKAHUB_BIN.get_or_init(resolve_pikahub_binary).clone()
}

fn resolve_pikahub_binary() -> String {
    // CARGO_BIN_EXE_pikahub is set by cargo when the workspace has a [[bin]] target
    // named "pikahub" and the test depends on that crate. This handles non-default
    // target dirs and release profiles correctly.
    if let Ok(bin) = std::env::var("CARGO_BIN_EXE_pikahub") {
        if Path::new(&bin).exists() {
            return bin;
        }
        eprintln!(
            "[TestInfra] WARNING: CARGO_BIN_EXE_pikahub points to missing path: {bin}; falling back"
        );
    }

    // Fallback: look relative to the workspace root.
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let bin = repo_root.join("target/debug/pikahub");

    let build = Command::new("cargo")
        .arg("build")
        .arg("-p")
        .arg("pikahub")
        .current_dir(&repo_root)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap_or_else(|e| panic!("failed to run cargo build -p pikahub: {e}"));

    if !build.status.success() {
        let stdout = String::from_utf8_lossy(&build.stdout);
        let stderr = String::from_utf8_lossy(&build.stderr);
        panic!("cargo build -p pikahub failed:\nstdout: {stdout}\nstderr: {stderr}");
    }

    assert!(
        bin.exists(),
        "pikahub binary not found at {} after build. Build it with: cargo build -p pikahub",
        bin.display()
    );
    bin.to_string_lossy().to_string()
}
