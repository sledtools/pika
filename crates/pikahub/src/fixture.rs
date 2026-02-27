use std::path::Path;
use std::process::Stdio as StdStdio;

use anyhow::{Context, Result, bail};
use tokio::signal;
use tracing::{error, info, warn};

use crate::component::{
    Bot, MoqRelay, Postgres, Relay, Server, get_process_fingerprint, kill_pid, kill_pid_safe,
};
use crate::config::ResolvedConfig;
use crate::health;
use crate::manifest::Manifest;

fn kill_stale_manifest(state_dir: &Path) {
    let Ok(Some(m)) = Manifest::load(state_dir) else {
        return;
    };
    info!("Cleaning up stale fixture from previous run...");
    for (pid, start_time) in m.all_pids() {
        kill_pid_safe(pid, start_time);
    }
    if let Some(host) = m
        .database_url
        .as_deref()
        .and_then(|u| u.split("host=").nth(1))
    {
        let _ = std::process::Command::new("pg_ctl")
            .args(["stop", "-D", host, "-m", "fast"])
            .stdout(StdStdio::null())
            .stderr(StdStdio::null())
            .status();
    }
    let _ = Manifest::remove(state_dir);
}

struct RunningFixture {
    postgres: Option<Postgres>,
    relay: Option<Relay>,
    moq: Option<MoqRelay>,
    server: Option<Server>,
    bot: Option<Bot>,
}

impl RunningFixture {
    fn teardown(&mut self) {
        if let Some(ref mut bot) = self.bot
            && let Some(pid) = bot.pid()
        {
            info!("[bot] Stopping (pid={pid})...");
            kill_pid(pid);
        }
        self.bot = None;

        if let Some(ref mut server) = self.server
            && let Some(pid) = server.pid()
        {
            info!("[server] Stopping (pid={pid})...");
            kill_pid(pid);
        }
        self.server = None;

        if let Some(ref mut moq) = self.moq
            && let Some(pid) = moq.pid()
        {
            info!("[moq] Stopping (pid={pid})...");
            kill_pid(pid);
        }
        self.moq = None;

        if let Some(ref mut relay) = self.relay
            && let Some(pid) = relay.pid()
        {
            info!("[relay] Stopping (pid={pid})...");
            kill_pid(pid);
        }
        self.relay = None;

        if let Some(ref pg) = self.postgres {
            let _ = pg.stop();
        }
        self.postgres = None;
    }
}

pub async fn up_foreground(config: &ResolvedConfig) -> Result<()> {
    let state_dir = &config.state_dir;
    std::fs::create_dir_all(state_dir)?;

    kill_stale_manifest(state_dir);

    let mut fixture = RunningFixture {
        postgres: None,
        relay: None,
        moq: None,
        server: None,
        bot: None,
    };

    let result = start_components(config, state_dir, &mut fixture).await;

    if let Err(ref e) = result {
        error!("Startup failed: {e:#}");
        fixture.teardown();
        let _ = Manifest::remove(state_dir);
        return result;
    }

    let manifest = build_manifest(config, &fixture);
    manifest.save(state_dir)?;

    print_summary(config, &manifest);

    eprintln!("Press Ctrl-C to stop all services.\n");

    signal::ctrl_c().await?;

    eprintln!("\nShutting down...");
    fixture.teardown();
    Manifest::remove(state_dir)?;
    eprintln!("Stopped.");

    Ok(())
}

pub async fn up_background(config: &ResolvedConfig) -> Result<()> {
    let state_dir = &config.state_dir;
    std::fs::create_dir_all(state_dir)?;

    kill_stale_manifest(state_dir);

    let mut fixture = RunningFixture {
        postgres: None,
        relay: None,
        moq: None,
        server: None,
        bot: None,
    };

    let result = start_components(config, state_dir, &mut fixture).await;

    if let Err(ref e) = result {
        error!("Startup failed: {e:#}");
        fixture.teardown();
        let _ = Manifest::remove(state_dir);
        return result;
    }

    let manifest = build_manifest(config, &fixture);
    manifest.save(state_dir)?;

    // Detach children so they outlive this process.
    std::mem::forget(fixture);

    let json = serde_json::to_string_pretty(&manifest)?;
    println!("{json}");

    Ok(())
}

async fn start_components(
    config: &ResolvedConfig,
    state_dir: &Path,
    fixture: &mut RunningFixture,
) -> Result<()> {
    if config.profile.needs_postgres() {
        fixture.postgres = Some(Postgres::start(config).await?);
    }

    if config.profile.needs_relay() {
        fixture.relay = Some(Relay::start(config, state_dir).await?);
    }

    if config.profile.needs_moq() {
        fixture.moq = Some(MoqRelay::start(config, state_dir).await?);
    }

    if config.profile.needs_server() {
        let db_url = fixture
            .postgres
            .as_ref()
            .map(|pg| pg.database_url.as_str())
            .unwrap_or("");
        let relay_url = fixture.relay.as_ref().map(|r| r.url.as_str()).unwrap_or("");
        fixture.server = Some(Server::start(config, state_dir, db_url, relay_url).await?);
    }

    if config.profile.needs_bot() {
        let relay_url = fixture.relay.as_ref().map(|r| r.url.as_str()).unwrap_or("");
        fixture.bot = Some(Bot::start(config, state_dir, relay_url).await?);
    }

    Ok(())
}

fn build_manifest(config: &ResolvedConfig, fixture: &RunningFixture) -> Manifest {
    let relay_pid = fixture.relay.as_ref().and_then(|r| r.pid());
    let moq_pid = fixture.moq.as_ref().and_then(|m| m.pid());
    let server_pid = fixture.server.as_ref().and_then(|s| s.pid());
    let bot_pid = fixture.bot.as_ref().and_then(|b| b.pid());

    Manifest {
        profile: config.profile.to_string(),
        relay_url: fixture.relay.as_ref().map(|r| r.url.clone()),
        relay_pid,
        relay_start_time: relay_pid.and_then(get_process_fingerprint),
        moq_url: fixture.moq.as_ref().map(|m| m.url.clone()),
        moq_pid,
        moq_start_time: moq_pid.and_then(get_process_fingerprint),
        server_url: fixture.server.as_ref().map(|s| s.url.clone()),
        server_pid,
        server_start_time: server_pid.and_then(get_process_fingerprint),
        server_pubkey_hex: fixture.server.as_ref().map(|s| s.pubkey_hex.clone()),
        database_url: fixture.postgres.as_ref().map(|pg| pg.database_url.clone()),
        postgres_pid: fixture.postgres.as_ref().and_then(|pg| pg.pid()),
        bot_npub: fixture.bot.as_ref().map(|b| b.npub.clone()),
        bot_pubkey_hex: fixture.bot.as_ref().map(|b| b.pubkey_hex.clone()),
        bot_pid,
        bot_start_time: bot_pid.and_then(get_process_fingerprint),
        state_dir: config.state_dir.clone(),
        started_at: chrono::Utc::now().to_rfc3339(),
    }
}

fn print_summary(config: &ResolvedConfig, manifest: &Manifest) {
    eprintln!();
    eprintln!("=== pikahub ready ({}) ===", config.profile);
    eprintln!();
    if let Some(ref url) = manifest.relay_url {
        eprintln!("  Relay:     {url}");
    }
    if let Some(ref url) = manifest.moq_url {
        eprintln!("  MoQ:       {url}");
    }
    if let Some(ref url) = manifest.server_url {
        eprintln!("  Server:    {url}");
    }
    if let Some(ref url) = manifest.database_url {
        eprintln!("  Postgres:  {url}");
    }
    if let Some(ref pk) = manifest.server_pubkey_hex {
        eprintln!("  Pubkey:    {pk}");
    }
    if let Some(ref npub) = manifest.bot_npub {
        eprintln!("  Bot:       {npub}");
    }
    eprintln!();
}

pub async fn down(state_dir: &Path) -> Result<()> {
    let Some(m) = Manifest::load(state_dir)? else {
        warn!(
            "No manifest found at {}. Nothing to stop.",
            state_dir.display()
        );
        return Ok(());
    };

    info!("Stopping fixture (profile={})...", m.profile);
    for (pid, start_time) in m.all_pids() {
        kill_pid_safe(pid, start_time);
    }
    if let Some(host) = m
        .database_url
        .as_deref()
        .and_then(|u| u.split("host=").nth(1))
    {
        let _ = std::process::Command::new("pg_ctl")
            .args(["stop", "-D", host, "-m", "fast"])
            .stdout(StdStdio::null())
            .stderr(StdStdio::null())
            .status();
    }
    Manifest::remove(state_dir)?;
    info!("Stopped.");

    Ok(())
}

pub fn down_sync(state_dir: &Path) -> Result<()> {
    if tokio::runtime::Handle::try_current().is_ok() {
        let state_dir = state_dir.to_path_buf();
        let join = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            runtime.block_on(down(&state_dir))
        })
        .join();
        match join {
            Ok(result) => result,
            Err(_) => bail!("down_sync worker thread panicked"),
        }
    } else {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        runtime.block_on(down(state_dir))
    }
}

pub async fn status(state_dir: &Path, json: bool) -> Result<()> {
    let Some(m) = Manifest::load(state_dir)? else {
        if json {
            println!("null");
        } else {
            eprintln!(
                "No fixture running (no manifest at {}).",
                state_dir.display()
            );
        }
        return Ok(());
    };

    if json {
        println!("{}", serde_json::to_string_pretty(&m)?);
        return Ok(());
    }

    eprintln!("Profile:   {}", m.profile);
    eprintln!("State dir: {}", m.state_dir.display());
    eprintln!("Started:   {}", m.started_at);
    if let Some(ref url) = m.relay_url {
        let tag = pid_status_tag(m.relay_pid, m.relay_start_time.as_deref());
        eprintln!("Relay:     {url} (pid={}, {tag})", m.relay_pid.unwrap_or(0));
    }
    if let Some(ref url) = m.moq_url {
        let tag = pid_status_tag(m.moq_pid, m.moq_start_time.as_deref());
        eprintln!("MoQ:       {url} (pid={}, {tag})", m.moq_pid.unwrap_or(0));
    }
    if let Some(ref url) = m.server_url {
        let tag = pid_status_tag(m.server_pid, m.server_start_time.as_deref());
        eprintln!(
            "Server:    {url} (pid={}, {tag})",
            m.server_pid.unwrap_or(0)
        );
    }
    if let Some(ref url) = m.database_url {
        let tag = pid_status_tag(m.postgres_pid, None);
        eprintln!(
            "Postgres:  {url} (pid={}, {tag})",
            m.postgres_pid.unwrap_or(0)
        );
    }
    if let Some(ref npub) = m.bot_npub {
        let tag = pid_status_tag(m.bot_pid, m.bot_start_time.as_deref());
        eprintln!("Bot:       {npub} (pid={}, {tag})", m.bot_pid.unwrap_or(0));
    }

    Ok(())
}

fn pid_status_tag(pid: Option<u32>, expected_fingerprint: Option<&str>) -> &'static str {
    let Some(pid) = pid else { return "n/a" };
    let Some(actual) = get_process_fingerprint(pid) else {
        return "dead";
    };
    match expected_fingerprint {
        Some(expected) if actual != expected => "dead (pid reused)",
        _ => "running",
    }
}

pub async fn logs(state_dir: &Path, follow: bool, component: Option<&str>) -> Result<()> {
    let log_files: Vec<(&str, std::path::PathBuf)> = match component {
        Some(name) => vec![(name, state_dir.join(format!("{name}.log")))],
        None => vec![
            ("relay", state_dir.join("relay.log")),
            ("moq", state_dir.join("moq.log")),
            ("server", state_dir.join("server.log")),
            ("bot", state_dir.join("bot.log")),
        ],
    };

    let existing: Vec<_> = log_files.into_iter().filter(|(_, p)| p.exists()).collect();

    if existing.is_empty() {
        bail!("No log files found in {}", state_dir.display());
    }

    if follow {
        let mut args = vec!["-f".to_string()];
        for (_, path) in &existing {
            args.push(path.to_string_lossy().to_string());
        }
        let status = tokio::process::Command::new("tail")
            .args(&args)
            .status()
            .await?;
        if !status.success() {
            bail!("tail exited with {status}");
        }
    } else {
        for (name, path) in &existing {
            let content = std::fs::read_to_string(path)?;
            if !content.is_empty() {
                eprintln!("=== {name} ===");
                eprint!("{content}");
                eprintln!();
            }
        }
    }

    Ok(())
}

pub async fn print_env(state_dir: &Path) -> Result<()> {
    let manifest =
        Manifest::load(state_dir)?.context("no manifest found -- is the fixture running?")?;

    println!("{}", manifest.shell_export_lines());

    Ok(())
}

pub async fn exec(state_dir: &Path, command: &[String]) -> Result<()> {
    if command.is_empty() {
        bail!("no command specified");
    }

    let manifest =
        Manifest::load(state_dir)?.context("no manifest found -- is the fixture running?")?;

    let mut cmd = tokio::process::Command::new(&command[0]);
    if command.len() > 1 {
        cmd.args(&command[1..]);
    }
    for (key, val) in manifest.env_exports() {
        cmd.env(key, val);
    }

    let status = cmd.status().await?;
    std::process::exit(status.code().unwrap_or(1));
}

pub async fn wait(state_dir: &Path, timeout_secs: u64) -> Result<()> {
    let manifest =
        Manifest::load(state_dir)?.context("no manifest found -- is the fixture running?")?;

    let timeout = std::time::Duration::from_secs(timeout_secs);

    if let Some(host) = manifest
        .database_url
        .as_deref()
        .and_then(|u| u.split("host=").nth(1))
    {
        health::wait_for_pg_isready(std::path::Path::new(host), timeout)
            .await
            .context("postgres not healthy")?;
        info!("[wait] postgres healthy");
    }

    if let Some(ref url) = manifest.relay_url {
        let health_url = url
            .replace("ws://", "http://")
            .replace("wss://", "https://");
        health::wait_for_http(&format!("{health_url}/health"), timeout)
            .await
            .context("relay not healthy")?;
        info!("[wait] relay healthy");
    }

    if let Some(moq_pid) = manifest.moq_pid {
        // moq-relay has no HTTP health endpoint (QUIC only). Verify the
        // process is still alive as a basic liveness check.
        let alive = std::process::Command::new("kill")
            .args(["-0", &moq_pid.to_string()])
            .stdout(StdStdio::null())
            .stderr(StdStdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !alive {
            bail!("moq-relay (pid {moq_pid}) is not running");
        }
        info!("[wait] moq-relay alive (pid {moq_pid})");
    }

    if let Some(ref url) = manifest.server_url {
        health::wait_for_http(&format!("{url}/health-check"), timeout)
            .await
            .context("server not healthy")?;
        info!("[wait] server healthy");
    }

    info!("[wait] all components healthy");
    Ok(())
}

pub async fn nuke(state_dir: &Path) -> Result<()> {
    // 1. Try a graceful down first (manifest-based).
    let manifest = Manifest::load(state_dir)?;
    if manifest.is_some() {
        info!("[nuke] running graceful down first...");
        let _ = down(state_dir).await;
    }

    // 2. Stop any postgres instance whose pgdata lives inside state_dir.
    let pgdata = state_dir.join("pgdata");
    if pgdata.join("postmaster.pid").exists() {
        info!("[nuke] stopping postgres in {}", pgdata.display());
        let _ = std::process::Command::new("pg_ctl")
            .args(["stop", "-D"])
            .arg(&pgdata)
            .args(["-m", "immediate"])
            .stdout(StdStdio::null())
            .stderr(StdStdio::null())
            .status();
    }

    // 3. Force-kill any manifest PIDs that survived graceful down.
    //    Uses fingerprint verification to avoid killing unrelated processes
    //    that may have reused the same PID.
    if let Some(ref m) = manifest {
        for (pid, start_time) in m.all_pids() {
            info!("[nuke] force-killing pid {pid}");
            kill_pid_safe(pid, start_time);
        }
        // Postgres PID has no recorded start_time, so use kill_pid directly
        // only if the pgdata dir belongs to this state_dir (already checked above).
        if let Some(pid) = m.postgres_pid {
            info!("[nuke] force-killing postgres pid {pid}");
            kill_pid(pid);
        }
    }

    // 4. Remove the state directory entirely.
    if state_dir.exists() {
        info!("[nuke] removing {}", state_dir.display());
        std::fs::remove_dir_all(state_dir)?;
    }

    info!("[nuke] done");
    Ok(())
}
