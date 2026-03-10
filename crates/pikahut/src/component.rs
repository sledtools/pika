use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio as StdStdio;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use tokio::process::{Child, Command};
use tracing::{info, warn};

use crate::config::ResolvedConfig;
use crate::health;

const PIKA_FIXTURE_RELAY_CMD_ENV: &str = "PIKA_FIXTURE_RELAY_CMD";
const PIKA_FIXTURE_SERVER_CMD_ENV: &str = "PIKA_FIXTURE_SERVER_CMD";

fn fixture_binary_override_value(env_key: &str, cmd: Option<String>) -> Result<Option<PathBuf>> {
    let Some(cmd) = cmd else {
        return Ok(None);
    };
    let path = PathBuf::from(&cmd);
    if path.exists() {
        return Ok(Some(path));
    }
    bail!("{env_key}={cmd} does not exist");
}

fn fixture_binary_override(env_key: &str) -> Result<Option<PathBuf>> {
    fixture_binary_override_value(env_key, std::env::var(env_key).ok())
}

pub struct Postgres {
    pub pgdata: PathBuf,
    pub database_url: String,
}

impl Postgres {
    pub async fn start(config: &ResolvedConfig) -> Result<Self> {
        let pgdata = config.pgdata();
        let db_name = "pika_server";

        std::fs::create_dir_all(&pgdata)?;

        if !pgdata.join("PG_VERSION").exists() {
            info!("[postgres] Initializing data dir...");
            let out = std::process::Command::new("initdb")
                .args(["--no-locale", "--encoding=UTF8", "-D"])
                .arg(&pgdata)
                .output()
                .context("initdb")?;
            if !out.status.success() {
                bail!("initdb failed: {}", String::from_utf8_lossy(&out.stderr));
            }
        }

        let conf_path = pgdata.join("postgresql.conf");
        let conf = std::fs::read_to_string(&conf_path).unwrap_or_default();
        let pgdata_str = pgdata.to_string_lossy();

        let mut additions = String::new();
        if !conf.contains("listen_addresses = ''") {
            additions.push_str("listen_addresses = ''\n");
        }
        let socket_line = format!("unix_socket_directories = '{pgdata_str}'");
        if !conf.contains(&socket_line) {
            additions.push_str(&socket_line);
            additions.push('\n');
        }
        if !additions.is_empty() {
            std::fs::OpenOptions::new()
                .append(true)
                .open(&conf_path)?
                .write_all(additions.as_bytes())
                .context("append postgresql.conf")?;
        }

        let status = std::process::Command::new("pg_ctl")
            .args(["status", "-D"])
            .arg(&pgdata)
            .stdout(StdStdio::null())
            .stderr(StdStdio::null())
            .status();

        let already_running = status.map(|s| s.success()).unwrap_or(false);

        if !already_running {
            info!("[postgres] Starting...");
            let log_path = pgdata.join("postgres.log");
            let out = std::process::Command::new("pg_ctl")
                .arg("start")
                .arg("-D")
                .arg(&pgdata)
                .arg("-l")
                .arg(&log_path)
                .arg("-o")
                .arg(format!("-k {pgdata_str}"))
                .output()
                .context("pg_ctl start")?;
            if !out.status.success() {
                bail!(
                    "pg_ctl start failed: {}",
                    String::from_utf8_lossy(&out.stderr)
                );
            }
        } else {
            info!("[postgres] Already running.");
        }

        health::wait_for_pg_isready(&pgdata, Duration::from_secs(10)).await?;

        let check = std::process::Command::new("psql")
            .args(["-h", &pgdata_str, "-d", "postgres", "-Atqc"])
            .arg(format!(
                "SELECT 1 FROM pg_database WHERE datname='{db_name}' LIMIT 1;"
            ))
            .output()
            .context("psql check db exists")?;

        let exists = String::from_utf8_lossy(&check.stdout).trim() == "1";
        if !exists {
            let out = std::process::Command::new("createdb")
                .args(["-h", &pgdata_str, db_name])
                .output()
                .context("createdb")?;
            if !out.status.success() {
                bail!("createdb failed: {}", String::from_utf8_lossy(&out.stderr));
            }
            info!("[postgres] Created database {db_name}.");
        }

        let database_url = format!("postgresql:///{db_name}?host={pgdata_str}");
        info!("[postgres] Ready (DATABASE_URL={database_url})");

        Ok(Self {
            pgdata,
            database_url,
        })
    }

    pub fn stop(&self) -> Result<()> {
        let status = std::process::Command::new("pg_ctl")
            .args(["status", "-D"])
            .arg(&self.pgdata)
            .stdout(StdStdio::null())
            .stderr(StdStdio::null())
            .status();

        if status.map(|s| s.success()).unwrap_or(false) {
            info!("[postgres] Stopping...");
            let _ = std::process::Command::new("pg_ctl")
                .args(["stop", "-D"])
                .arg(&self.pgdata)
                .args(["-m", "fast"])
                .stdout(StdStdio::null())
                .stderr(StdStdio::null())
                .status();
        }
        Ok(())
    }

    pub fn pid(&self) -> Option<u32> {
        let pid_file = self.pgdata.join("postmaster.pid");
        let content = std::fs::read_to_string(pid_file).ok()?;
        content.lines().next()?.trim().parse().ok()
    }
}

pub struct Relay {
    pub child: Child,
    pub url: String,
}

impl Relay {
    pub async fn start(config: &ResolvedConfig, state_dir: &Path) -> Result<Self> {
        let data_dir = config.relay_data_dir();
        let media_dir = config.relay_media_dir();
        std::fs::create_dir_all(&data_dir)?;
        std::fs::create_dir_all(&media_dir)?;

        let requested_port = config.relay_port;
        let log_path = state_dir.join("relay.log");

        let relay_bin = find_or_build_relay(&config.workspace_root)?;

        info!("[relay] Starting pika-relay on port {requested_port}...");
        let log_file = std::fs::File::create(&log_path)?;
        let stderr_file = log_file.try_clone()?;

        let mut cmd = Command::new(&relay_bin);
        cmd.env("PORT", requested_port.to_string())
            .env("DATA_DIR", &data_dir)
            .env("MEDIA_DIR", &media_dir)
            // Debug attach-point for local E2E investigations.
            .env("PIKA_RELAY_LOG_EVENTS", "1")
            .stdout(StdStdio::from(log_file))
            .stderr(StdStdio::from(stderr_file))
            .kill_on_drop(true);
        // Only set SERVICE_URL when we know the port up-front; the Go relay
        // derives it from the actual bound port when SERVICE_URL is unset.
        if requested_port != 0 {
            cmd.env("SERVICE_URL", format!("http://localhost:{requested_port}"));
        }
        let mut child = cmd
            .spawn()
            .with_context(|| format!("spawn relay binary: {}", relay_bin.display()))?;

        // The relay prints "PIKA_RELAY_PORT=<N>" to its log, which tells us
        // the actual port (important when requested_port is 0).
        // Poll the log, but also check whether the child exited early (e.g.
        // bind failure) so we can surface the real error immediately.
        let port_line = {
            let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
            loop {
                if let Some(status) = child.try_wait()? {
                    let log_tail = std::fs::read_to_string(&log_path).unwrap_or_default();
                    bail!("relay exited early (status {status}):\n{log_tail}");
                }
                if log_path.exists() {
                    let content = tokio::fs::read_to_string(&log_path)
                        .await
                        .unwrap_or_default();
                    if let Some(line) = content.lines().find(|l| l.contains("PIKA_RELAY_PORT=")) {
                        break line.to_string();
                    }
                }
                if tokio::time::Instant::now() >= deadline {
                    bail!(
                        "relay did not report its port within 15 s; log:\n{}",
                        std::fs::read_to_string(&log_path).unwrap_or_default()
                    );
                }
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
        };

        let port: u16 = port_line
            .split("PIKA_RELAY_PORT=")
            .nth(1)
            .unwrap_or("")
            .trim()
            .parse()
            .context("failed to parse relay port from log")?;

        let url = format!("ws://localhost:{port}");

        let health_url = format!("http://127.0.0.1:{port}/health");
        health::wait_for_http(&health_url, Duration::from_secs(15)).await?;

        info!("[relay] Ready ({url})");
        Ok(Self { child, url })
    }

    pub fn pid(&self) -> Option<u32> {
        self.child.id()
    }
}

fn find_or_build_relay(workspace_root: &Path) -> Result<PathBuf> {
    if let Some(path) = fixture_binary_override(PIKA_FIXTURE_RELAY_CMD_ENV)? {
        return Ok(path);
    }

    let target_bin = workspace_root.join("target/pika-relay");
    if target_bin.exists() {
        return Ok(target_bin);
    }

    info!("[relay] Building pika-relay binary (go build)...");
    let relay_dir = workspace_root.join("cmd/pika-relay");
    let mut build = std::process::Command::new("go");
    build
        .args(["build", "-o"])
        .arg(&target_bin)
        .arg(".")
        .current_dir(&relay_dir);

    if cfg!(target_os = "macos") {
        // Keep this Go/cgo build independent from Xcode-pinned shell defaults.
        // We intentionally clear DEVELOPER_DIR and force plain clang/clang++ so
        // the relay build uses the Nix toolchain from PATH instead of Xcode.
        build
            .env_remove("DEVELOPER_DIR")
            .env("CC", "clang")
            .env("CXX", "clang++");
    }

    let out = build.output().context("go build pika-relay")?;

    if !out.status.success() {
        bail!(
            "go build pika-relay failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    Ok(target_bin)
}

pub struct MoqRelay {
    pub child: Child,
    pub url: String,
}

impl MoqRelay {
    pub async fn start(config: &ResolvedConfig, state_dir: &Path) -> Result<Self> {
        let requested_port = config.moq_port;
        let log_path = state_dir.join("moq.log");

        // Pick a free UDP port when port is 0.
        let port = if requested_port == 0 {
            let sock = std::net::UdpSocket::bind("127.0.0.1:0")
                .context("bind UDP socket for moq-relay port discovery")?;
            let p = sock.local_addr()?.port();
            drop(sock);
            p
        } else {
            requested_port
        };

        let url = format!("https://127.0.0.1:{port}/anon");

        info!("[moq] Starting moq-relay on port {port}...");
        let log_file = std::fs::File::create(&log_path)?;
        let stderr_file = log_file.try_clone()?;

        let mut child = Command::new("moq-relay")
            .arg("--server-bind")
            .arg(format!("127.0.0.1:{port}"))
            .arg("--tls-generate")
            .arg("127.0.0.1")
            .arg("--auth-public")
            .arg("anon")
            .arg("--log-level")
            .arg("info")
            .stdout(StdStdio::from(log_file))
            .stderr(StdStdio::from(stderr_file))
            .kill_on_drop(true)
            .spawn()
            .context("spawn moq-relay (is moq-relay on PATH?)")?;

        // moq-relay has no HTTP health endpoint (QUIC only). Give it a moment
        // to bind, then check it hasn't exited early.
        tokio::time::sleep(Duration::from_millis(500)).await;

        if let Some(status) = child.try_wait()? {
            let log_tail = std::fs::read_to_string(&log_path).unwrap_or_default();
            bail!("moq-relay exited early (status {status}):\n{log_tail}");
        }

        info!("[moq] Ready ({url})");
        Ok(Self { child, url })
    }

    pub fn pid(&self) -> Option<u32> {
        self.child.id()
    }
}

pub struct Server {
    pub child: Child,
    pub url: String,
}

impl Server {
    pub async fn start(
        config: &ResolvedConfig,
        state_dir: &Path,
        database_url: &str,
        relay_url: &str,
    ) -> Result<Self> {
        let port = config.server_port;
        let url = config.server_url();
        let log_path = state_dir.join("server.log");

        info!("[server] Starting pika-server on port {port}...");
        let log_file = std::fs::File::create(&log_path)?;
        let stderr_file = log_file.try_clone()?;

        // Helper: inherit env var if set, otherwise use the provided default.
        let env_or = |key: &str, default: &str| -> (String, String) {
            let val = std::env::var(key).unwrap_or_else(|_| default.to_string());
            (key.to_string(), val)
        };

        let mut child_cmd =
            if let Some(server_bin) = fixture_binary_override(PIKA_FIXTURE_SERVER_CMD_ENV)? {
                Command::new(server_bin)
            } else {
                let mut cmd = Command::new("cargo");
                cmd.args(["run", "-q", "-p", "pika-server"]);
                cmd
            };
        child_cmd
            .env("RELAYS", relay_url)
            .env("DATABASE_URL", database_url)
            .env("NOTIFICATION_PORT", port.to_string());

        for (key, val) in [
            env_or("PIKA_AGENT_MICROVM_KIND", "openclaw"),
            env_or("PIKA_AGENT_MICROVM_SPAWNER_URL", "http://127.0.0.1:1"),
            env_or(
                "PIKA_ADMIN_BOOTSTRAP_NPUBS",
                "npub1u8lnhlw5usp3t9vmpz60ejpyt649z33hu82wc2hpv6m5xdqmuxhs46turz",
            ),
            env_or("PIKA_ADMIN_SESSION_SECRET", "pikahut-test-secret-0000"),
            env_or("RUST_LOG", "info"),
        ] {
            child_cmd.env(key, val);
        }

        let child = child_cmd
            .current_dir(&config.workspace_root)
            .stdout(StdStdio::from(log_file))
            .stderr(StdStdio::from(stderr_file))
            .kill_on_drop(true)
            .spawn()
            .context("spawn pika-server")?;

        let health_url = format!("http://127.0.0.1:{port}/health-check");
        health::wait_for_http(&health_url, Duration::from_secs(60)).await?;

        info!("[server] Ready ({url})");
        Ok(Self { child, url })
    }

    pub fn pid(&self) -> Option<u32> {
        self.child.id()
    }
}

pub struct Bot {
    pub child: Child,
    pub npub: String,
    pub pubkey_hex: String,
}

impl Bot {
    pub async fn start(config: &ResolvedConfig, state_dir: &Path, relay_url: &str) -> Result<Self> {
        let bot_state_dir = state_dir.join("bot");
        std::fs::create_dir_all(&bot_state_dir)?;
        let log_path = state_dir.join("bot.log");

        info!("[bot] Starting E2E bot...");
        let log_file = std::fs::File::create(&log_path)?;
        let stderr_file = log_file.try_clone()?;

        let child = Command::new("cargo")
            .args(["run", "-q", "-p", "pikachat", "--", "--state-dir"])
            .arg(&bot_state_dir)
            .args(["--relay", relay_url, "bot", "--timeout-sec"])
            .arg(config.bot_timeout_secs.to_string())
            .current_dir(&config.workspace_root)
            .stdout(StdStdio::from(log_file))
            .stderr(StdStdio::from(stderr_file))
            .kill_on_drop(true)
            .spawn()
            .context("spawn e2e bot")?;

        let ready_line = health::wait_for_log_line(
            &log_path,
            "ready pubkey=",
            Duration::from_secs(config.bot_timeout_secs),
        )
        .await?;

        let pubkey_hex = ready_line
            .split("pubkey=")
            .nth(1)
            .unwrap_or("")
            .split_whitespace()
            .next()
            .unwrap_or("")
            .to_string();

        let npub = ready_line
            .split("npub=")
            .nth(1)
            .unwrap_or("")
            .split_whitespace()
            .next()
            .unwrap_or("")
            .to_string();

        info!("[bot] Ready (npub={npub})");
        Ok(Self {
            child,
            npub,
            pubkey_hex,
        })
    }

    pub fn pid(&self) -> Option<u32> {
        self.child.id()
    }
}

/// Fingerprint a process by its start time + command line via `ps`.
/// Start time alone is second-resolution; adding the command args makes
/// a collision with a reused PID within the same second negligible.
pub fn get_process_fingerprint(pid: u32) -> Option<String> {
    let output = std::process::Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "lstart=,args="])
        .output()
        .ok()?;
    let s = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}

/// Kill `pid` only if its fingerprint matches what we recorded at spawn time.
/// If `expected_fingerprint` is None (old manifest, failed capture), we refuse
/// to kill -- the caller must use `kill_pid` directly for the live-teardown
/// path where we know the process is ours.
pub fn kill_pid_safe(pid: u32, expected_fingerprint: Option<&str>) {
    let Some(expected) = expected_fingerprint else {
        warn!(
            "PID {pid}: no recorded fingerprint; skipping kill \
             (cannot verify process identity)"
        );
        return;
    };
    match get_process_fingerprint(pid) {
        Some(ref actual) if actual == expected => kill_pid(pid),
        Some(actual) => {
            warn!(
                "PID {pid} fingerprint mismatch; skipping kill (likely PID reuse)\n  \
                 expected: {expected}\n  actual:   {actual}"
            );
        }
        None => {} // process already gone
    }
}

pub fn kill_pid(pid: u32) {
    use nix::sys::signal::{Signal, kill};
    use nix::unistd::Pid;

    let nix_pid = Pid::from_raw(pid as i32);

    if kill(nix_pid, None).is_err() {
        return;
    }

    let _ = kill(nix_pid, Signal::SIGTERM);

    for _ in 0..20 {
        std::thread::sleep(Duration::from_millis(100));
        if kill(nix_pid, None).is_err() {
            return;
        }
    }

    warn!("PID {pid} did not exit after SIGTERM, sending SIGKILL");
    let _ = kill(nix_pid, Signal::SIGKILL);
}

#[cfg(test)]
mod tests {
    use super::fixture_binary_override_value;
    use std::fs;

    #[test]
    fn fixture_binary_override_returns_none_when_env_missing() {
        assert!(
            fixture_binary_override_value("PIKAHUT_DOES_NOT_EXIST_FOR_TESTS", None)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn fixture_binary_override_requires_existing_path() {
        let key = "PIKAHUT_TEST_FIXTURE_BINARY_OVERRIDE_INVALID";
        let err = fixture_binary_override_value(
            key,
            Some("/definitely/missing/pikahut-test-binary".to_string()),
        )
        .expect_err("invalid override should fail");
        assert!(err
            .to_string()
            .contains("PIKAHUT_TEST_FIXTURE_BINARY_OVERRIDE_INVALID=/definitely/missing/pikahut-test-binary does not exist"));
    }

    #[test]
    fn fixture_binary_override_accepts_existing_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("fixture-bin");
        fs::write(&path, "ok").expect("write temp fixture binary");
        let resolved = fixture_binary_override_value(
            "PIKAHUT_TEST_FIXTURE_BINARY_OVERRIDE_VALID",
            Some(path.display().to_string()),
        )
        .expect("resolve override");
        assert_eq!(resolved.as_deref(), Some(path.as_path()));
    }
}
