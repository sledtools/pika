use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio as StdStdio;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use tokio::process::{Child, Command};
use tracing::{info, warn};

use crate::config::ResolvedConfig;
use crate::health;

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
    pub port: u16,
    pub url: String,
}

impl Relay {
    pub async fn start(config: &ResolvedConfig, state_dir: &Path) -> Result<Self> {
        let data_dir = config.relay_data_dir();
        let media_dir = config.relay_media_dir();
        std::fs::create_dir_all(&data_dir)?;
        std::fs::create_dir_all(&media_dir)?;

        let port = config.relay_port;
        let url = config.relay_url();
        let log_path = state_dir.join("relay.log");

        let relay_bin = find_or_build_relay(&config.workspace_root)?;

        info!("[relay] Starting pika-relay on port {port}...");
        let log_file = std::fs::File::create(&log_path)?;
        let stderr_file = log_file.try_clone()?;

        let child = Command::new(&relay_bin)
            .env("PORT", port.to_string())
            .env("DATA_DIR", &data_dir)
            .env("MEDIA_DIR", &media_dir)
            .env("SERVICE_URL", format!("http://localhost:{port}"))
            .stdout(StdStdio::from(log_file))
            .stderr(StdStdio::from(stderr_file))
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("spawn relay binary: {}", relay_bin.display()))?;

        let health_url = format!("http://127.0.0.1:{port}/health");
        health::wait_for_http(&health_url, Duration::from_secs(15)).await?;

        info!("[relay] Ready ({url})");
        Ok(Self { child, port, url })
    }

    pub fn pid(&self) -> Option<u32> {
        self.child.id()
    }
}

fn find_or_build_relay(workspace_root: &Path) -> Result<PathBuf> {
    if let Ok(cmd) = std::env::var("PIKA_FIXTURE_RELAY_CMD") {
        let p = PathBuf::from(&cmd);
        if p.exists() {
            return Ok(p);
        }
        bail!("PIKA_FIXTURE_RELAY_CMD={cmd} does not exist");
    }

    let target_bin = workspace_root.join("target/pika-relay");
    if target_bin.exists() {
        return Ok(target_bin);
    }

    info!("[relay] Building pika-relay binary (go build)...");
    let out = std::process::Command::new("go")
        .args(["build", "-o"])
        .arg(&target_bin)
        .arg("./cmd/pika-relay")
        .current_dir(workspace_root)
        .output()
        .context("go build pika-relay")?;

    if !out.status.success() {
        bail!(
            "go build pika-relay failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    Ok(target_bin)
}

pub struct Server {
    pub child: Child,
    pub port: u16,
    pub url: String,
    pub pubkey_hex: String,
}

impl Server {
    pub async fn start(
        config: &ResolvedConfig,
        state_dir: &Path,
        database_url: &str,
    ) -> Result<Self> {
        let identity_path = config.identity_json();
        let keys = pika_marmot_runtime::load_or_create_keys(&identity_path)?;
        let pubkey_hex = keys.public_key().to_hex();
        let secret_hex = keys.secret_key().to_secret_hex();

        info!("[server] Server pubkey: {pubkey_hex}");

        let port = config.server_port;
        let url = config.server_url();
        let relay_url = config.relay_url();
        let log_path = state_dir.join("server.log");

        let open_prov = if config.open_provisioning { "1" } else { "0" };

        info!("[server] Starting pika-server on port {port}...");
        let log_file = std::fs::File::create(&log_path)?;
        let stderr_file = log_file.try_clone()?;

        let child = Command::new("cargo")
            .args(["run", "-q", "-p", "pika-server"])
            .env("RELAYS", &relay_url)
            .env("DATABASE_URL", database_url)
            .env("NOTIFICATION_PORT", port.to_string())
            .env("PIKA_AGENT_CONTROL_ENABLED", "1")
            .env("PIKA_AGENT_CONTROL_NOSTR_SECRET", &secret_hex)
            .env("PIKA_AGENT_CONTROL_RELAYS", &relay_url)
            .env("PIKA_AGENT_CONTROL_ALLOW_OPEN_PROVISIONING", open_prov)
            .env(
                "RUST_LOG",
                std::env::var("RUST_LOG").unwrap_or_else(|_| "info".into()),
            )
            .current_dir(&config.workspace_root)
            .stdout(StdStdio::from(log_file))
            .stderr(StdStdio::from(stderr_file))
            .kill_on_drop(true)
            .spawn()
            .context("spawn pika-server")?;

        let health_url = format!("http://127.0.0.1:{port}/health-check");
        health::wait_for_http(&health_url, Duration::from_secs(60)).await?;

        info!("[server] Ready ({url})");
        Ok(Self {
            child,
            port,
            url,
            pubkey_hex,
        })
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
