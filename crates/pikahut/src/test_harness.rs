use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow, bail};
use clap::{Args, Subcommand, ValueEnum};
use serde_json::{Value, json};

use crate::config::{self, BotOverlay, OverlayConfig, ProfileName, ResolvedConfig};
use crate::{fixture, health, manifest::Manifest};

#[derive(Debug, Subcommand)]
pub enum TestCommand {
    /// Run a pikachat scenario with optional local relay fixture wiring.
    Scenario(TestScenarioArgs),
    /// Run full OpenClaw gateway integration E2E.
    OpenclawE2e(OpenclawE2eArgs),
    /// Run CLI smoke test (invite + welcome + message, optional media).
    CliSmoke(CliSmokeArgs),
    /// Run deterministic local UI E2E against relay+bot fixture.
    UiE2eLocal(UiE2eLocalArgs),
    /// Run baseline interop against external rust_harness bot.
    InteropRustBaseline(InteropRustBaselineArgs),
}

#[derive(Debug, Args)]
pub struct TestScenarioArgs {
    pub scenario: String,

    #[arg(long)]
    pub state_dir: Option<PathBuf>,

    #[arg(long)]
    pub relay: Option<String>,

    #[arg(last = true)]
    pub extra_args: Vec<String>,
}

#[derive(Debug, Args)]
pub struct OpenclawE2eArgs {
    #[arg(long)]
    pub state_dir: Option<PathBuf>,

    #[arg(long)]
    pub relay_url: Option<String>,

    #[arg(long)]
    pub openclaw_dir: Option<PathBuf>,

    /// Keep generated state/artifacts on success too.
    #[arg(long, default_value_t = false)]
    pub keep_state: bool,
}

#[derive(Debug, Args)]
pub struct CliSmokeArgs {
    #[arg(long)]
    pub relay: Option<String>,

    #[arg(long, default_value_t = false)]
    pub with_media: bool,

    #[arg(long)]
    pub state_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum UiPlatform {
    Android,
    Ios,
    Desktop,
}

#[derive(Debug, Args)]
pub struct UiE2eLocalArgs {
    #[arg(long, value_enum)]
    pub platform: UiPlatform,

    #[arg(long)]
    pub state_dir: Option<PathBuf>,

    /// Keep state directory after completion (also honored via KEEP=1).
    #[arg(long, default_value_t = false)]
    pub keep: bool,

    #[arg(long)]
    pub bot_timeout_sec: Option<u64>,
}

#[derive(Debug, Args)]
pub struct InteropRustBaselineArgs {
    #[arg(long, default_value_t = false)]
    pub manual: bool,

    #[arg(long, default_value_t = false)]
    pub keep: bool,

    #[arg(long)]
    pub state_dir: Option<PathBuf>,

    #[arg(long)]
    pub rust_interop_dir: Option<PathBuf>,

    #[arg(long)]
    pub bot_timeout_sec: Option<u64>,
}

struct HarnessCleanup {
    state_dir: PathBuf,
    started_fixture: bool,
    auto_state_dir: bool,
    keep_dir: bool,
    preserve_on_error: bool,
    success: bool,
}

impl HarnessCleanup {
    fn new(
        state_dir: PathBuf,
        started_fixture: bool,
        auto_state_dir: bool,
        keep_dir: bool,
        preserve_on_error: bool,
    ) -> Self {
        Self {
            state_dir,
            started_fixture,
            auto_state_dir,
            keep_dir,
            preserve_on_error,
            success: false,
        }
    }

    fn mark_success(&mut self) {
        self.success = true;
    }
}

impl Drop for HarnessCleanup {
    fn drop(&mut self) {
        let keep = self.keep_dir || (!self.success && self.preserve_on_error);

        if self.started_fixture {
            let _ = fixture::down_sync(&self.state_dir);
        }

        if self.auto_state_dir && !keep {
            let _ = fs::remove_dir_all(&self.state_dir);
        } else if self.auto_state_dir && keep {
            eprintln!("note: keeping state dir: {}", self.state_dir.display());
        }
    }
}

struct ChildGuard {
    child: Option<Child>,
}

impl ChildGuard {
    fn new(child: Child) -> Self {
        Self { child: Some(child) }
    }

    fn try_wait(&mut self) -> Result<Option<std::process::ExitStatus>> {
        match self.child.as_mut() {
            Some(child) => Ok(child.try_wait()?),
            None => Ok(None),
        }
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

pub async fn run(command: TestCommand) -> Result<()> {
    match command {
        TestCommand::Scenario(args) => run_scenario(args).await,
        TestCommand::OpenclawE2e(args) => run_openclaw_e2e(args).await,
        TestCommand::CliSmoke(args) => run_cli_smoke(args).await,
        TestCommand::UiE2eLocal(args) => run_ui_e2e_local(args).await,
        TestCommand::InteropRustBaseline(args) => run_interop_rust_baseline(args).await,
    }
}

async fn run_scenario(args: TestScenarioArgs) -> Result<()> {
    let root = config::find_workspace_root()?;
    let (state_dir, auto_state_dir) =
        prepare_state_dir(args.state_dir, "pikachat-openclaw-scenario")?;

    let (relay_url, started_fixture) = if let Some(relay) = args.relay {
        (relay, false)
    } else {
        let manifest = start_profile(ProfileName::Relay, &state_dir, None).await?;
        (
            manifest
                .relay_url
                .ok_or_else(|| anyhow!("manifest missing relay_url"))?,
            true,
        )
    };

    let mut cleanup = HarnessCleanup::new(
        state_dir.clone(),
        started_fixture,
        auto_state_dir,
        false,
        false,
    );

    let mut cmd = cargo_pikachat_cmd(&root);
    cmd.arg("scenario")
        .arg(&args.scenario)
        .arg("--relay")
        .arg(&relay_url)
        .arg("--state-dir")
        .arg(&state_dir);
    cmd.args(&args.extra_args);

    run_status(&mut cmd, "run pikachat scenario")?;
    cleanup.mark_success();
    Ok(())
}

async fn run_openclaw_e2e(args: OpenclawE2eArgs) -> Result<()> {
    let root = config::find_workspace_root()?;
    let (state_dir, auto_state_dir) = prepare_state_dir(args.state_dir, "pikachat-openclaw-e2e")?;

    let mut started_fixture = false;
    let relay_url = if let Some(relay) = args.relay_url {
        relay
    } else {
        let manifest = start_profile(ProfileName::Relay, &state_dir, None).await?;
        started_fixture = true;
        manifest
            .relay_url
            .ok_or_else(|| anyhow!("manifest missing relay_url"))?
    };

    let mut cleanup = HarnessCleanup::new(
        state_dir.clone(),
        started_fixture,
        auto_state_dir,
        args.keep_state,
        true,
    );

    let openclaw_dir = resolve_openclaw_dir(&root, args.openclaw_dir)?;
    if !openclaw_dir.join("package.json").is_file() {
        bail!(
            "openclaw checkout not found at {} (set --openclaw-dir or OPENCLAW_DIR)",
            openclaw_dir.display()
        );
    }

    let artifact_dir = state_dir.join("artifacts/openclaw-e2e");
    let openclaw_state_dir = state_dir.join("openclaw/state");
    let openclaw_config_path = state_dir.join("openclaw/openclaw.json");
    let sidecar_state_dir = state_dir.join("cli/pikachat/default");
    let plugin_path = root.join("pikachat-openclaw/openclaw/extensions/pikachat-openclaw");
    let openclaw_log = artifact_dir.join("openclaw.log");
    let scenario_log = artifact_dir.join("scenario.log");

    fs::create_dir_all(&artifact_dir)?;
    fs::create_dir_all(&openclaw_state_dir)?;
    fs::create_dir_all(&sidecar_state_dir)?;
    if let Some(parent) = openclaw_config_path.parent() {
        fs::create_dir_all(parent)?;
    }

    run_status(
        Command::new("cargo")
            .arg("build")
            .arg("--manifest-path")
            .arg(root.join("Cargo.toml"))
            .arg("-p")
            .arg("pikachat"),
        "build pikachat sidecar",
    )?;

    let sidecar_cmd = root.join("target/debug/pikachat");

    if command_exists("pnpm") {
        run_status(
            Command::new("pnpm")
                .arg("-C")
                .arg(&openclaw_dir)
                .arg("install")
                .stdout(Stdio::null())
                .stderr(Stdio::null()),
            "install openclaw pnpm dependencies",
        )?;
    } else {
        run_status(
            Command::new("npx")
                .args(["--yes", "pnpm@10", "-C"])
                .arg(&openclaw_dir)
                .arg("install")
                .stdout(Stdio::null())
                .stderr(Stdio::null()),
            "install openclaw pnpm dependencies via npx",
        )?;
    }

    let gw_port = pick_free_port()?;
    let gw_token = format!(
        "e2e-{}-{}",
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
        std::process::id()
    );

    let sidecar_args = vec![
        "daemon".to_string(),
        "--relay".to_string(),
        relay_url.clone(),
        "--state-dir".to_string(),
        sidecar_state_dir.to_string_lossy().to_string(),
    ];

    let config_json = json!({
      "plugins": {
        "enabled": true,
        "allow": ["pikachat-openclaw"],
        "load": { "paths": [plugin_path] },
        "slots": { "memory": "none" },
        "entries": {
          "pikachat-openclaw": {
            "enabled": true,
            "config": {
              "relays": [relay_url],
              "groupPolicy": "open",
              "autoAcceptWelcomes": true,
              "stateDir": sidecar_state_dir,
              "sidecarCmd": sidecar_cmd,
              "sidecarArgs": sidecar_args,
            }
          }
        }
      },
      "channels": {
        "pikachat-openclaw": {
          "relays": [relay_url],
          "groupPolicy": "open",
          "autoAcceptWelcomes": true,
          "stateDir": sidecar_state_dir,
          "sidecarCmd": sidecar_cmd,
          "sidecarArgs": sidecar_args,
        }
      }
    });

    fs::write(
        &openclaw_config_path,
        format!("{}\n", serde_json::to_string_pretty(&config_json)?),
    )?;
    fs::copy(&openclaw_config_path, artifact_dir.join("openclaw.json"))?;

    let log_file = File::create(&openclaw_log)?;
    let err_file = log_file.try_clone()?;

    let child = Command::new("node")
        .current_dir(&openclaw_dir)
        .env("OPENCLAW_STATE_DIR", &openclaw_state_dir)
        .env("OPENCLAW_CONFIG_PATH", &openclaw_config_path)
        .env("OPENCLAW_GATEWAY_TOKEN", &gw_token)
        .env("OPENCLAW_SKIP_BROWSER_CONTROL_SERVER", "1")
        .env("OPENCLAW_SKIP_GMAIL_WATCHER", "1")
        .env("OPENCLAW_SKIP_CANVAS_HOST", "1")
        .env("OPENCLAW_SKIP_CRON", "1")
        .arg("scripts/run-node.mjs")
        .arg("gateway")
        .arg("--port")
        .arg(gw_port.to_string())
        .arg("--allow-unconfigured")
        .stdout(Stdio::from(log_file))
        .stderr(Stdio::from(err_file))
        .spawn()
        .context("spawn openclaw gateway")?;
    let mut child_guard = ChildGuard::new(child);

    let identity_path = sidecar_state_dir.join("identity.json");
    let mut ready = false;
    for _ in 0..80 {
        if identity_path.is_file() {
            ready = true;
            break;
        }
        if child_guard.try_wait()?.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }

    if !ready {
        eprintln!("OpenClaw/pikachat-openclaw sidecar did not start (missing identity.json)");
        eprintln!(
            "openclaw e2e failed; artifacts preserved at: {}",
            artifact_dir.display()
        );
        eprintln!("{}", tail_lines(&openclaw_log, 120));
        bail!("openclaw sidecar startup failed");
    }

    let identity: Value = serde_json::from_str(
        &fs::read_to_string(&identity_path)
            .with_context(|| format!("read {}", identity_path.display()))?,
    )
    .context("parse sidecar identity.json")?;
    let peer_pubkey = identity
        .get("public_key_hex")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("identity.json missing public_key_hex"))?;

    let output = run_output_raw(
        Command::new("cargo")
            .current_dir(&root)
            .arg("run")
            .arg("--manifest-path")
            .arg(root.join("Cargo.toml"))
            .arg("-p")
            .arg("pikachat")
            .arg("--")
            .arg("scenario")
            .arg("invite-and-chat-peer")
            .arg("--relay")
            .arg(&relay_url)
            .arg("--state-dir")
            .arg(&state_dir)
            .arg("--peer-pubkey")
            .arg(peer_pubkey),
        "run invite-and-chat-peer scenario",
    )?;

    let mut scenario_bytes = Vec::new();
    scenario_bytes.extend_from_slice(&output.output.stdout);
    scenario_bytes.extend_from_slice(&output.output.stderr);
    fs::write(&scenario_log, &scenario_bytes)?;
    print!("{}", String::from_utf8_lossy(&output.output.stdout));
    eprint!("{}", String::from_utf8_lossy(&output.output.stderr));

    if !output.output.status.success() {
        eprintln!(
            "openclaw e2e failed; artifacts preserved at: {}",
            artifact_dir.display()
        );
        eprintln!("{}", tail_lines(&openclaw_log, 120));
        bail!(
            "invite-and-chat-peer scenario failed with status {}",
            output.output.status
        );
    }

    cleanup.mark_success();
    Ok(())
}

async fn run_cli_smoke(args: CliSmokeArgs) -> Result<()> {
    let root = config::find_workspace_root()?;
    let (state_dir, auto_state_dir) = prepare_state_dir(args.state_dir, "pikachat-smoke")?;

    let (relay_url, started_fixture) = if let Some(relay) = args.relay {
        (relay, false)
    } else {
        let manifest = start_profile(ProfileName::Relay, &state_dir, None).await?;
        let relay = manifest
            .relay_url
            .ok_or_else(|| anyhow!("manifest missing relay_url"))?;
        println!("relay: {relay}");
        (relay, true)
    };

    let mut cleanup = HarnessCleanup::new(
        state_dir.clone(),
        started_fixture,
        auto_state_dir,
        false,
        false,
    );

    println!("=== Alice: create identity ===");
    let alice = run_pikachat_json(
        &root,
        &[
            "--state-dir",
            path_to_str(&state_dir.join("alice"))?,
            "--relay",
            &relay_url,
            "identity",
        ],
    )?;
    let alice_pk = json_str(&alice, "pubkey")?;
    println!("Alice pubkey: {alice_pk}");

    println!("=== Bob: create identity ===");
    let bob = run_pikachat_json(
        &root,
        &[
            "--state-dir",
            path_to_str(&state_dir.join("bob"))?,
            "--relay",
            &relay_url,
            "identity",
        ],
    )?;
    let bob_pk = json_str(&bob, "pubkey")?;
    println!("Bob pubkey: {bob_pk}");

    println!("=== Both: publish key packages ===");
    run_pikachat_ok(
        &root,
        &[
            "--state-dir",
            path_to_str(&state_dir.join("alice"))?,
            "--relay",
            &relay_url,
            "publish-kp",
        ],
    )?;
    run_pikachat_ok(
        &root,
        &[
            "--state-dir",
            path_to_str(&state_dir.join("bob"))?,
            "--relay",
            &relay_url,
            "publish-kp",
        ],
    )?;

    println!("=== Alice: invite Bob ===");
    let invite = run_pikachat_json(
        &root,
        &[
            "--state-dir",
            path_to_str(&state_dir.join("alice"))?,
            "--relay",
            &relay_url,
            "invite",
            "--peer",
            &bob_pk,
        ],
    )?;
    let group = json_str(&invite, "nostr_group_id")?;
    println!("Group: {group}");

    println!("=== Bob: sync welcomes (listen 3s) ===");
    let _ = run_pikachat_allow_failure(
        &root,
        &[
            "--state-dir",
            path_to_str(&state_dir.join("bob"))?,
            "--relay",
            &relay_url,
            "listen",
            "--timeout",
            "3",
            "--lookback",
            "300",
        ],
    )?;

    println!("=== Bob: check welcomes ===");
    let welcomes = run_pikachat_json(
        &root,
        &[
            "--state-dir",
            path_to_str(&state_dir.join("bob"))?,
            "--relay",
            &relay_url,
            "welcomes",
        ],
    )?;
    println!("{}", serde_json::to_string_pretty(&welcomes)?);

    let wrapper = welcomes
        .get("welcomes")
        .and_then(Value::as_array)
        .and_then(|arr| arr.first())
        .and_then(|w| w.get("wrapper_event_id"))
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("welcomes missing welcomes[0].wrapper_event_id"))?
        .to_string();

    println!("=== Bob: accept welcome ===");
    run_pikachat_ok(
        &root,
        &[
            "--state-dir",
            path_to_str(&state_dir.join("bob"))?,
            "--relay",
            &relay_url,
            "accept-welcome",
            "--wrapper-event-id",
            &wrapper,
        ],
    )?;

    println!("=== Alice: send message ===");
    run_pikachat_ok(
        &root,
        &[
            "--state-dir",
            path_to_str(&state_dir.join("alice"))?,
            "--relay",
            &relay_url,
            "send",
            "--group",
            &group,
            "--content",
            "hello from alice",
        ],
    )?;

    println!("=== Bob: sync inbox (listen 3s) ===");
    let _ = run_pikachat_allow_failure(
        &root,
        &[
            "--state-dir",
            path_to_str(&state_dir.join("bob"))?,
            "--relay",
            &relay_url,
            "listen",
            "--timeout",
            "3",
            "--lookback",
            "300",
        ],
    )?;

    println!("=== Bob: read messages ===");
    let messages = run_pikachat_json(
        &root,
        &[
            "--state-dir",
            path_to_str(&state_dir.join("bob"))?,
            "--relay",
            &relay_url,
            "messages",
            "--group",
            &group,
        ],
    )?;
    println!("{}", serde_json::to_string_pretty(&messages)?);

    if args.with_media {
        println!("=== Alice: send media ===");
        let media_src = state_dir.join("sample-media.txt");
        fs::write(
            &media_src,
            format!(
                "hello media {}\n",
                SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs()
            ),
        )?;

        run_pikachat_ok(
            &root,
            &[
                "--state-dir",
                path_to_str(&state_dir.join("alice"))?,
                "--relay",
                &relay_url,
                "send",
                "--group",
                &group,
                "--media",
                path_to_str(&media_src)?,
                "--mime-type",
                "text/plain",
                "--content",
                "media from alice",
            ],
        )?;

        println!("=== Bob: sync media message (listen 5s) ===");
        let _ = run_pikachat_allow_failure(
            &root,
            &[
                "--state-dir",
                path_to_str(&state_dir.join("bob"))?,
                "--relay",
                &relay_url,
                "listen",
                "--timeout",
                "5",
                "--lookback",
                "300",
            ],
        )?;

        println!("=== Bob: read messages with media ===");
        let bob_msgs = run_pikachat_json(
            &root,
            &[
                "--state-dir",
                path_to_str(&state_dir.join("bob"))?,
                "--relay",
                &relay_url,
                "messages",
                "--group",
                &group,
            ],
        )?;
        println!("{}", serde_json::to_string_pretty(&bob_msgs)?);

        let media_msg_id = bob_msgs
            .get("messages")
            .and_then(Value::as_array)
            .and_then(|arr| {
                arr.iter().rev().find_map(|msg| {
                    let has_media = msg
                        .get("media")
                        .and_then(Value::as_array)
                        .map(|media| !media.is_empty())
                        .unwrap_or(false);
                    if has_media {
                        msg.get("message_id")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                    } else {
                        None
                    }
                })
            })
            .ok_or_else(|| anyhow!("could not find media attachment in Bob's messages"))?;

        println!("=== Bob: download/decrypt media ===");
        let media_out = state_dir.join("bob-downloaded-media.txt");
        run_pikachat_ok(
            &root,
            &[
                "--state-dir",
                path_to_str(&state_dir.join("bob"))?,
                "--relay",
                &relay_url,
                "download-media",
                &media_msg_id,
                "--output",
                path_to_str(&media_out)?,
            ],
        )?;

        let src = fs::read(&media_src)?;
        let out = fs::read(&media_out)?;
        if src != out {
            bail!("downloaded media does not match source file");
        }
    }

    println!("=== SMOKE TEST PASSED ===");
    cleanup.mark_success();
    Ok(())
}

async fn run_ui_e2e_local(args: UiE2eLocalArgs) -> Result<()> {
    let root = config::find_workspace_root()?;
    let keep = args.keep || env_truthy("KEEP");
    let bot_timeout_sec = args.bot_timeout_sec.unwrap_or_else(|| {
        std::env::var("BOT_TIMEOUT_SEC")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(900)
    });

    if matches!(args.platform, UiPlatform::Android) {
        let adb_bin = std::env::var("ADB").unwrap_or_else(|_| "adb".to_string());
        let emulator_bin = std::env::var("EMULATOR").unwrap_or_else(|_| "emulator".to_string());
        let avd_name =
            std::env::var("PIKA_ANDROID_AVD_NAME").unwrap_or_else(|_| "pika_api35".to_string());

        let tools_missing = !command_exists(&adb_bin) || !command_exists(&emulator_bin);
        if tools_missing {
            if in_ci() {
                eprintln!("SKIP: android ui e2e local requires adb+emulator on PATH");
                return Ok(());
            }
            bail!("missing adb/emulator on PATH; run inside nix develop");
        }

        let list = run_output_raw(Command::new(&emulator_bin).arg("-list-avds"), "list AVDs")?;
        let avds = String::from_utf8_lossy(&list.output.stdout);
        let found = avds.lines().any(|line| line.trim() == avd_name);
        if !found {
            if in_ci() {
                eprintln!(
                    "SKIP: android ui e2e local requires AVD '{}' (not present)",
                    avd_name
                );
                return Ok(());
            }
            bail!("android AVD '{}' not found", avd_name);
        }
    }

    let (state_dir, auto_state_dir) = prepare_state_dir(args.state_dir, "pika-ui-e2e-local")?;

    let overlay = OverlayConfig {
        bot: Some(BotOverlay {
            timeout_secs: Some(bot_timeout_sec),
        }),
        ..OverlayConfig::default()
    };

    let manifest = start_profile(ProfileName::RelayBot, &state_dir, Some(overlay)).await?;
    let relay_url = manifest
        .relay_url
        .ok_or_else(|| anyhow!("manifest missing relay_url"))?;
    let bot_npub = manifest
        .bot_npub
        .ok_or_else(|| anyhow!("manifest missing bot_npub"))?;
    let relay_port = parse_url_port(&relay_url)?;
    let android_relay_url = format!("ws://10.0.2.2:{relay_port}");

    println!("relay_url={relay_url}");
    println!("android_relay_url={android_relay_url}");
    println!("bot_npub={bot_npub}");

    let mut cleanup = HarnessCleanup::new(state_dir.clone(), true, auto_state_dir, keep, false);

    let client_nsec = resolve_ui_client_nsec(&root)?;

    match args.platform {
        UiPlatform::Android => {
            let test_class = std::env::var("PIKA_ANDROID_E2E_TEST_CLASS")
                .unwrap_or_else(|_| "com.pika.app.PikaE2eUiTest".to_string());
            let test_suffix = std::env::var("PIKA_ANDROID_TEST_APPLICATION_ID_SUFFIX")
                .unwrap_or_else(|_| ".test".to_string());
            let test_app_id = format!("org.pikachat.pika{test_suffix}");

            run_status(
                &mut Command::new(root.join("tools/android-emulator-ensure")),
                "ensure android emulator",
            )?;
            run_status(
                Command::new("just")
                    .current_dir(&root)
                    .args(["gen-kotlin", "android-rust", "android-local-properties"])
                    .stdout(Stdio::null()),
                "prepare android build inputs",
            )?;
            run_status(
                Command::new(root.join("tools/android-ensure-debug-installable"))
                    .env("PIKA_ANDROID_APP_ID", &test_app_id)
                    .stdout(Stdio::null()),
                "ensure android debug installable",
            )?;

            run_status(
                Command::new("./gradlew")
                    .current_dir(root.join("android"))
                    .arg(":app:connectedDebugAndroidTest")
                    .arg(format!("-PPIKA_ANDROID_APPLICATION_ID_SUFFIX={test_suffix}"))
                    .arg(format!(
                        "-Pandroid.testInstrumentationRunnerArguments.class={test_class}"
                    ))
                    .arg("-Pandroid.testInstrumentationRunnerArguments.pika_e2e=1")
                    .arg("-Pandroid.testInstrumentationRunnerArguments.pika_disable_network=false")
                    .arg("-Pandroid.testInstrumentationRunnerArguments.pika_reset=1")
                    .arg(format!(
                        "-Pandroid.testInstrumentationRunnerArguments.pika_peer_npub={bot_npub}"
                    ))
                    .arg(format!(
                        "-Pandroid.testInstrumentationRunnerArguments.pika_relay_urls={android_relay_url}"
                    ))
                    .arg(format!(
                        "-Pandroid.testInstrumentationRunnerArguments.pika_key_package_relay_urls={android_relay_url}"
                    ))
                    .arg(format!(
                        "-Pandroid.testInstrumentationRunnerArguments.pika_nsec={client_nsec}"
                    )),
                "run android local UI E2E",
            )?;
        }
        UiPlatform::Ios => {
            run_status(
                Command::new("just")
                    .current_dir(&root)
                    .args(["ios-xcframework", "ios-xcodeproj"])
                    .stdout(Stdio::null()),
                "prepare ios build inputs",
            )?;

            let sim_output = run_output(
                Command::new(root.join("tools/ios-sim-ensure"))
                    .env("PIKA_UI_E2E_NSEC", &client_nsec)
                    .env("PIKA_UI_E2E_BOT_NPUB", &bot_npub)
                    .env("PIKA_UI_E2E_RELAYS", &relay_url)
                    .env("PIKA_UI_E2E_KP_RELAYS", &relay_url),
                "ensure ios simulator",
            )?;
            let sim_stdout = String::from_utf8_lossy(&sim_output.stdout);
            let udid = extract_udid(&sim_stdout).ok_or_else(|| {
                anyhow!("could not determine simulator udid from ios-sim-ensure output")
            })?;

            run_status(
                Command::new(root.join("tools/xcode-run"))
                    .env("PIKA_UI_E2E", "1")
                    .env("PIKA_UI_E2E_BOT_NPUB", &bot_npub)
                    .env("PIKA_UI_E2E_RELAYS", &relay_url)
                    .env("PIKA_UI_E2E_KP_RELAYS", &relay_url)
                    .env("PIKA_UI_E2E_NSEC", &client_nsec)
                    .arg("xcodebuild")
                    .args(["-project", "ios/Pika.xcodeproj", "-scheme", "Pika"])
                    .arg("-destination")
                    .arg(format!("id={udid}"))
                    .args([
                        "test",
                        "CODE_SIGNING_ALLOWED=NO",
                        "-only-testing:PikaUITests/PikaUITests/testE2E_deployedRustBot_pingPong",
                    ]),
                "run ios local UI E2E",
            )?;
        }
        UiPlatform::Desktop => {
            run_status(
                Command::new("cargo")
                    .current_dir(&root)
                    .env("PIKA_UI_E2E", "1")
                    .env("PIKA_UI_E2E_BOT_NPUB", &bot_npub)
                    .env("PIKA_UI_E2E_RELAYS", &relay_url)
                    .env("PIKA_UI_E2E_KP_RELAYS", &relay_url)
                    .env("PIKA_UI_E2E_NSEC", &client_nsec)
                    .args([
                        "test",
                        "-p",
                        "pika-desktop",
                        "desktop_e2e_local_ping_pong_with_bot",
                        "--",
                        "--ignored",
                        "--nocapture",
                    ]),
                "run desktop local UI E2E",
            )?;
        }
    }

    cleanup.mark_success();
    Ok(())
}

async fn run_interop_rust_baseline(args: InteropRustBaselineArgs) -> Result<()> {
    let root = config::find_workspace_root()?;

    let rust_interop_dir = args.rust_interop_dir.or_else(|| {
        std::env::var("PIKACHAT_INTEROP_RUST_DIR")
            .ok()
            .map(PathBuf::from)
    });
    let rust_interop_dir = rust_interop_dir.unwrap_or_else(|| {
        dirs_home()
            .unwrap_or_else(|| PathBuf::from("~"))
            .join("code/marmot-interop-lab-rust")
    });

    if !rust_interop_dir.is_dir() {
        bail!(
            "missing rust interop repo at {} (set --rust-interop-dir or PIKACHAT_INTEROP_RUST_DIR)",
            rust_interop_dir.display()
        );
    }

    check_mdk_skew(&rust_interop_dir)?;

    let explicit_state = args.state_dir.or_else(|| {
        std::env::var("PIKA_INTEROP_STATE_DIR")
            .ok()
            .map(PathBuf::from)
    });
    let (state_dir, auto_state_dir) = prepare_state_dir(explicit_state, "pika-interop-rustbot")?;

    let keep = args.keep || !auto_state_dir;
    let bot_timeout_sec = args.bot_timeout_sec.unwrap_or_else(|| {
        std::env::var("BOT_TIMEOUT_SEC")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(900)
    });

    let manifest = start_profile(ProfileName::Relay, &state_dir, None).await?;
    let relay_url = manifest
        .relay_url
        .ok_or_else(|| anyhow!("manifest missing relay_url"))?;
    let relay_port = parse_url_port(&relay_url)?;
    let android_relay_url = format!("ws://10.0.2.2:{relay_port}");

    println!("relay_url={relay_url}");
    println!("android_relay_url={android_relay_url}");

    let mut cleanup = HarnessCleanup::new(state_dir.clone(), true, auto_state_dir, keep, false);

    let bot_log = state_dir.join("rustbot.log");
    println!("starting rust bot (logs at: {})", bot_log.display());

    let log_file = File::create(&bot_log)?;
    let err_file = log_file.try_clone()?;
    let child = Command::new("cargo")
        .current_dir(&rust_interop_dir)
        .args(["run", "-q", "-p", "rust_harness", "--", "bot", "--relay"])
        .arg(&relay_url)
        .args(["--state-dir"])
        .arg(state_dir.join("bot"))
        .args(["--timeout-sec", &bot_timeout_sec.to_string()])
        .stdout(Stdio::from(log_file))
        .stderr(Stdio::from(err_file))
        .spawn()
        .context("spawn rust_harness bot")?;
    let _bot_guard = ChildGuard::new(child);

    let ready_line = health::wait_for_log_line(
        &bot_log,
        "[openclaw_bot] ready pubkey=",
        Duration::from_secs(bot_timeout_sec),
    )
    .await
    .with_context(|| format!("failed to detect bot readiness in {}", bot_log.display()))?;

    let bot_pubkey = extract_field(&ready_line, "pubkey=")
        .ok_or_else(|| anyhow!("ready line missing pubkey field"))?;
    let bot_npub = extract_field(&ready_line, "npub=")
        .ok_or_else(|| anyhow!("ready line missing npub field"))?;

    println!("bot_npub={bot_npub}");
    println!("bot_pubkey_hex={bot_pubkey}");

    if !args.manual {
        println!("running pika_core baseline against rust bot...");
        run_status(
            Command::new("cargo")
                .current_dir(&root)
                .args([
                    "run",
                    "-q",
                    "-p",
                    "pika_core",
                    "--bin",
                    "interop_rustbot_baseline",
                    "--",
                ])
                .arg(&relay_url)
                .arg(&bot_npub),
            "run pika_core interop baseline",
        )?;
    }

    println!(
        "\nManual validation (Android emulator):\n- Run: PIKA_RELAY_URLS=\"{}\" just run-android\n- In-app: Create Account -> New Chat -> paste bot_npub -> Start Chat -> send \"ping\" -> expect \"pong\"\n",
        android_relay_url
    );
    println!(
        "Manual validation (iOS simulator):\n- Run: PIKA_RELAY_URLS=\"{}\" just run-ios\n- Same in-app flow as above\n",
        relay_url
    );
    println!("Bot logs: {}", bot_log.display());

    if args.manual {
        println!("\nmanual mode: keeping relay+bot running; press Ctrl-C to stop.");
        tokio::signal::ctrl_c().await?;
    }

    cleanup.mark_success();
    Ok(())
}

async fn start_profile(
    profile: ProfileName,
    state_dir: &Path,
    overlay: Option<OverlayConfig>,
) -> Result<Manifest> {
    let resolved = ResolvedConfig::new(
        profile,
        overlay,
        false,
        Some(0),
        Some(0),
        None,
        Some(state_dir.to_path_buf()),
    )?;
    fixture::up_background(&resolved).await?;
    let manifest = Manifest::load(state_dir)?.ok_or_else(|| {
        anyhow!(
            "manifest missing after starting profile {} at {}",
            profile,
            state_dir.display()
        )
    })?;
    fixture::wait(state_dir, 30).await?;
    Ok(manifest)
}

fn prepare_state_dir(state_dir: Option<PathBuf>, prefix: &str) -> Result<(PathBuf, bool)> {
    match state_dir {
        Some(path) => {
            fs::create_dir_all(&path)?;
            Ok((path, false))
        }
        None => {
            let path = tempfile::Builder::new().prefix(prefix).tempdir()?.keep();
            Ok((path, true))
        }
    }
}

fn cargo_pikachat_cmd(root: &Path) -> Command {
    let mut cmd = Command::new("cargo");
    cmd.current_dir(root)
        .arg("run")
        .arg("-q")
        .arg("-p")
        .arg("pikachat")
        .arg("--");
    cmd
}

fn run_pikachat_ok(root: &Path, args: &[&str]) -> Result<()> {
    let mut cmd = cargo_pikachat_cmd(root);
    cmd.args(args);
    run_status(&mut cmd, "run pikachat command")
}

fn run_pikachat_json(root: &Path, args: &[&str]) -> Result<Value> {
    let mut cmd = cargo_pikachat_cmd(root);
    cmd.args(args);
    let output = run_output(&mut cmd, "run pikachat json command")?;
    let stdout = String::from_utf8(output.stdout).context("pikachat output not utf-8")?;
    serde_json::from_str(stdout.trim())
        .with_context(|| format!("failed to parse pikachat JSON output: {stdout}"))
}

fn run_pikachat_allow_failure(root: &Path, args: &[&str]) -> Result<Output> {
    let mut cmd = cargo_pikachat_cmd(root);
    cmd.args(args);
    Ok(run_output_raw(&mut cmd, "run pikachat command")?.output)
}

fn run_status(cmd: &mut Command, context: &str) -> Result<()> {
    let desc = command_description(cmd);
    let status = cmd
        .status()
        .with_context(|| format!("{context}: spawn failed for `{desc}`"))?;
    if !status.success() {
        bail!("{context}: `{desc}` failed with status {status}");
    }
    Ok(())
}

fn run_output(cmd: &mut Command, context: &str) -> Result<Output> {
    let command_output = run_output_raw(cmd, context)?;
    if !command_output.output.status.success() {
        let desc = command_output.command;
        bail!(
            "{context}: `{desc}` failed with status {}\nstdout:\n{}\nstderr:\n{}",
            command_output.output.status,
            String::from_utf8_lossy(&command_output.output.stdout),
            String::from_utf8_lossy(&command_output.output.stderr)
        );
    }
    Ok(command_output.output)
}

struct CommandOutput {
    output: Output,
    command: String,
}

fn run_output_raw(cmd: &mut Command, context: &str) -> Result<CommandOutput> {
    let desc = command_description(cmd);
    let output = cmd
        .output()
        .with_context(|| format!("{context}: spawn failed for `{desc}`"))?;
    Ok(CommandOutput {
        output,
        command: desc,
    })
}

fn command_description(cmd: &Command) -> String {
    let mut parts = Vec::new();
    parts.push(cmd.get_program().to_string_lossy().to_string());
    parts.extend(cmd.get_args().map(|arg| arg.to_string_lossy().to_string()));
    parts.join(" ")
}

fn json_str(value: &Value, key: &str) -> Result<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| anyhow!("missing string field '{}' in JSON: {}", key, value))
}

fn command_exists(cmd: &str) -> bool {
    Command::new("sh")
        .arg("-lc")
        .arg(format!("command -v {} >/dev/null 2>&1", shell_escape(cmd)))
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn shell_escape(s: &str) -> String {
    if s.chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '/'))
    {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

fn pick_free_port() -> Result<u16> {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0))?;
    Ok(listener.local_addr()?.port())
}

fn parse_url_port(url: &str) -> Result<u16> {
    let after_scheme = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);
    let host_port = after_scheme.split('/').next().unwrap_or(after_scheme);
    let port_str = host_port
        .rsplit_once(':')
        .map(|(_, port)| port)
        .ok_or_else(|| anyhow!("URL has no port: {url}"))?;
    port_str
        .parse::<u16>()
        .with_context(|| format!("invalid port in URL: {url}"))
}

fn path_to_str(path: &Path) -> Result<&str> {
    path.to_str()
        .ok_or_else(|| anyhow!("path is not valid UTF-8: {}", path.display()))
}

fn tail_lines(path: &Path, count: usize) -> String {
    let Ok(content) = fs::read_to_string(path) else {
        return String::new();
    };
    let lines: Vec<&str> = content.lines().collect();
    let start = lines.len().saturating_sub(count);
    lines[start..].join("\n")
}

fn resolve_openclaw_dir(root: &Path, cli_value: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(dir) = cli_value {
        return Ok(dir);
    }
    if let Ok(from_env) = std::env::var("OPENCLAW_DIR")
        && !from_env.trim().is_empty()
    {
        return Ok(PathBuf::from(from_env));
    }

    let direct = root.join("openclaw");
    if direct.join("package.json").is_file() {
        return Ok(direct);
    }

    if let Some(parent) = root.parent() {
        let sibling = parent.join("openclaw");
        if sibling.join("package.json").is_file() {
            return Ok(sibling);
        }
    }

    Ok(direct)
}

fn resolve_ui_client_nsec(root: &Path) -> Result<String> {
    if let Ok(nsec) = std::env::var("PIKA_UI_E2E_NSEC")
        && !nsec.trim().is_empty()
    {
        return Ok(nsec);
    }

    let nsec_file = root.join(".pikachat-test-nsec");
    if nsec_file.is_file() {
        let s = fs::read_to_string(&nsec_file)?;
        let trimmed = s.trim();
        if !trimmed.is_empty() {
            return Ok(trimmed.to_string());
        }
    }

    let script = r#"
import secrets
CHARSET='qpzry9x8gf2tvdw0s3jn54khce6mua7l'
def bech32_polymod(values):
  GEN=[0x3b6a57b2,0x26508e6d,0x1ea119fa,0x3d4233dd,0x2a1462b3]
  chk=1
  for v in values:
    b=chk>>25;chk=((chk&0x1ffffff)<<5)^v
    for i in range(5): chk^=GEN[i] if((b>>i)&1)else 0
  return chk
def bech32_hrp_expand(hrp):
  return [ord(x)>>5 for x in hrp]+[0]+[ord(x)&31 for x in hrp]
def bech32_create_checksum(hrp,data):
  values=bech32_hrp_expand(hrp)+data
  polymod=bech32_polymod(values+[0,0,0,0,0,0])^1
  return [(polymod>>5*(5-i))&31 for i in range(6)]
def convertbits(data,frombits,tobits,pad=True):
  acc=0;bits=0;ret=[];maxv=(1<<tobits)-1
  for b in data:
    acc=(acc<<frombits)|b;bits+=frombits
    while bits>=tobits: bits-=tobits;ret.append((acc>>bits)&maxv)
  if pad and bits: ret.append((acc<<(tobits-bits))&maxv)
  return ret
sk=secrets.token_bytes(32)
data5=convertbits(list(sk),8,5,True)
combined=data5+bech32_create_checksum('nsec',data5)
print('nsec'+'1'+''.join([CHARSET[d] for d in combined]))
"#;

    let output = run_output(
        Command::new("python3").arg("-c").arg(script),
        "generate ephemeral nsec",
    )?;
    let generated = String::from_utf8(output.stdout)?.trim().to_string();
    if generated.is_empty() {
        bail!("python nsec generator returned empty output");
    }
    eprintln!(
        "note: generated ephemeral local e2e nsec (set PIKA_UI_E2E_NSEC or .pikachat-test-nsec to override)"
    );
    Ok(generated)
}

fn in_ci() -> bool {
    env_truthy("CI") || env_truthy("GITHUB_ACTIONS")
}

fn env_truthy(key: &str) -> bool {
    std::env::var(key)
        .ok()
        .map(|v| {
            let s = v.trim().to_ascii_lowercase();
            s == "1" || s == "true" || s == "yes" || s == "on"
        })
        .unwrap_or(false)
}

fn extract_udid(output: &str) -> Option<String> {
    for line in output.lines() {
        let prefix = "ok: ios simulator ready (udid=";
        if let Some(rest) = line.strip_prefix(prefix)
            && let Some(udid) = rest.strip_suffix(')')
        {
            return Some(udid.to_string());
        }
    }
    None
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

fn check_mdk_skew(rust_interop_dir: &Path) -> Result<()> {
    let mdk_dir = std::env::var("MDK_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| dirs_home().unwrap_or_default().join("code/mdk"));

    if !mdk_dir.join(".git").is_dir() {
        return Ok(());
    }

    let mdk_head = String::from_utf8(
        run_output(
            Command::new("git")
                .current_dir(&mdk_dir)
                .args(["rev-parse", "HEAD"]),
            "read mdk git HEAD",
        )?
        .stdout,
    )?
    .trim()
    .to_string();

    let harness_cargo = rust_interop_dir.join("rust_harness/Cargo.toml");
    let harness_text = fs::read_to_string(&harness_cargo)
        .with_context(|| format!("read {}", harness_cargo.display()))?;

    let mut harness_rev = None;
    for line in harness_text.lines() {
        if !line.contains("mdk-core") || !line.contains("rev = \"") {
            continue;
        }
        if let Some(start) = line.find("rev = \"") {
            let rest = &line[start + 7..];
            if let Some(end) = rest.find('"') {
                let rev = &rest[..end];
                if rev.len() == 40 && rev.chars().all(|c| c.is_ascii_hexdigit()) {
                    harness_rev = Some(rev.to_string());
                    break;
                }
            }
        }
    }

    let Some(harness_rev) = harness_rev else {
        return Ok(());
    };

    if mdk_head != harness_rev {
        bail!(
            "MDK version skew detected\n  pika uses local MDK at: {} (HEAD={})\n  rust harness pins MDK rev: {}\nfix: align one side before interop conclusions",
            mdk_dir.display(),
            mdk_head,
            harness_rev,
        );
    }

    println!("ok: MDK rev aligned: {mdk_head}");
    Ok(())
}

fn extract_field(line: &str, key: &str) -> Option<String> {
    let value = line.split(key).nth(1)?;
    Some(value.split_whitespace().next()?.to_string())
}
