use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow, bail};
use nostr_sdk::prelude::{Client, PublicKey, RelayUrl};
use pika_marmot_runtime::relay::fetch_latest_key_package;
use serde_json::{Value, json};

use crate::config::{self, ProfileName};
use crate::testing::{
    ArtifactPolicy, CommandRunner, CommandSpec, FixtureSpec, TenantNamespace, TestContext,
    command::SpawnHandle, start_fixture,
};

use super::artifacts::{self, CommandOutcomeRecord};
use super::common::{pick_free_port, resolve_openclaw_dir, tail_lines};
use super::types::{OpenclawE2eRequest, ScenarioRunOutput};

const PIKACHAT_BIN_ENV: &str = "PIKAHUT_TEST_PIKACHAT_BIN";

fn pikachat_peer_spec(
    root: &Path,
    binary: &str,
    relay_url: &str,
    state_dir: &Path,
    peer_pubkey: &str,
) -> CommandSpec {
    CommandSpec::new(binary)
        .cwd(root)
        .args(["scenario", "invite-and-chat-peer", "--relay"])
        .arg(relay_url.to_string())
        .args([
            "--state-dir".to_string(),
            state_dir.to_string_lossy().to_string(),
        ])
        .args(["--peer-pubkey".to_string(), peer_pubkey.to_string()])
        .capture_name("openclaw-invite-and-chat-peer")
}

fn build_context(state_dir: Option<std::path::PathBuf>) -> Result<TestContext> {
    let mut builder =
        TestContext::builder("openclaw-e2e").artifact_policy(ArtifactPolicy::PreserveOnFailure);
    if let Some(path) = state_dir {
        builder = builder.state_dir(path);
    }
    builder.build()
}

fn read_identity_pubkey_hex(identity_path: &Path) -> Result<String> {
    let identity: Value = serde_json::from_str(
        &fs::read_to_string(identity_path)
            .with_context(|| format!("read {}", identity_path.display()))?,
    )
    .context("parse sidecar identity.json")?;
    identity
        .get("public_key_hex")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .ok_or_else(|| anyhow!("identity.json missing public_key_hex"))
}

async fn wait_for_sidecar_keypackage(
    relay_url: &str,
    sidecar_state_dir: &Path,
    gateway: &mut SpawnHandle,
) -> Result<String> {
    let relay_url = RelayUrl::parse(relay_url).context("parse openclaw relay url")?;
    let relay_urls = vec![relay_url.clone()];
    let identity_path = sidecar_state_dir.join("identity.json");
    let client = Client::default();
    client
        .add_relay(relay_url.clone())
        .await
        .with_context(|| format!("add relay {relay_url}"))?;
    client.connect().await;

    let mut peer_pubkey_hex: Option<String> = None;
    let mut last_fetch_err: Option<String> = None;

    for _ in 0..240 {
        if peer_pubkey_hex.is_none() && identity_path.is_file() {
            peer_pubkey_hex = Some(read_identity_pubkey_hex(&identity_path)?);
        }

        if let Some(peer_pubkey_hex) = peer_pubkey_hex.as_deref() {
            let peer_pubkey = PublicKey::from_hex(peer_pubkey_hex)
                .with_context(|| format!("parse bot pubkey from {}", identity_path.display()))?;
            match fetch_latest_key_package(
                &client,
                &peer_pubkey,
                &relay_urls,
                Duration::from_secs(2),
            )
            .await
            {
                Ok(_) => {
                    client.shutdown().await;
                    return Ok(peer_pubkey_hex.to_string());
                }
                Err(err) => {
                    last_fetch_err = Some(format!("{err:#}"));
                }
            }
        }

        if let Some(status) = gateway.try_wait()? {
            client.shutdown().await;
            bail!(
                "openclaw gateway exited before bot keypackage was published (status={status}, identity_present={}, last_fetch_error={})",
                identity_path.is_file(),
                last_fetch_err.unwrap_or_else(|| "none".to_string())
            );
        }

        tokio::time::sleep(Duration::from_millis(250)).await;
    }

    client.shutdown().await;
    bail!(
        "timed out waiting for OpenClaw bot keypackage publication (identity_present={}, last_fetch_error={})",
        identity_path.is_file(),
        last_fetch_err.unwrap_or_else(|| "none".to_string())
    )
}

pub async fn run_openclaw_e2e(args: OpenclawE2eRequest) -> Result<ScenarioRunOutput> {
    let root = config::find_workspace_root()?;
    let mut context = build_context(args.state_dir)?;
    let tenant_namespace =
        TenantNamespace::new(format!("{}-{}", context.run_name(), context.run_id()))
            .context("derive tenant namespace for openclaw-e2e")?;
    let tenant_relay_namespace = tenant_namespace.relay_namespace("openclaw-gateway");
    let tenant_moq_namespace = tenant_namespace.moq_namespace("openclaw-gateway");

    let fixture = if args.relay_url.is_none() {
        Some(start_fixture(&context, &FixtureSpec::builder(ProfileName::Relay).build()).await?)
    } else {
        None
    };

    let relay_url = match args.relay_url {
        Some(relay) => relay,
        None => fixture
            .as_ref()
            .and_then(|handle| handle.relay_url().map(ToOwned::to_owned))
            .ok_or_else(|| anyhow!("fixture manifest missing relay_url"))?,
    };

    let openclaw_dir = resolve_openclaw_dir(&root, args.openclaw_dir)?;
    if !openclaw_dir.join("package.json").is_file() {
        bail!(
            "openclaw checkout not found at {} (set --openclaw-dir or OPENCLAW_DIR)",
            openclaw_dir.display()
        );
    }

    let artifact_dir = context.ensure_artifact_subdir("openclaw-e2e")?;
    let openclaw_state_dir = context.state_dir().join("openclaw/state");
    let openclaw_config_path = context.state_dir().join("openclaw/openclaw.json");
    let sidecar_state_dir = context.state_dir().join("cli/pikachat/default");
    let plugin_path = root.join("pikachat-openclaw/openclaw/extensions/pikachat-openclaw");

    fs::create_dir_all(&artifact_dir)?;
    fs::create_dir_all(&openclaw_state_dir)?;
    fs::create_dir_all(&sidecar_state_dir)?;
    if let Some(parent) = openclaw_config_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let runner = CommandRunner::new(&context);
    let mut command_outcomes = Vec::new();

    let sidecar_cmd = if let Ok(binary) = std::env::var(PIKACHAT_BIN_ENV) {
        binary
    } else {
        let build_cmd = runner.run(
            &CommandSpec::cargo()
                .cwd(&root)
                .args(["build", "--manifest-path"])
                .arg(root.join("Cargo.toml").to_string_lossy().to_string())
                .args(["-p", "pikachat"])
                .capture_name("openclaw-build-pikachat"),
        )?;
        command_outcomes.push(CommandOutcomeRecord::from_output(
            "openclaw-build-pikachat",
            &build_cmd,
        ));
        root.join("target/debug/pikachat")
            .to_string_lossy()
            .to_string()
    };

    if super::common::command_exists("pnpm") {
        let pnpm_cmd = runner.run(
            &CommandSpec::new("pnpm")
                .cwd(&openclaw_dir)
                .args(["install"])
                .capture_name("openclaw-pnpm-install"),
        )?;
        command_outcomes.push(CommandOutcomeRecord::from_output(
            "openclaw-pnpm-install",
            &pnpm_cmd,
        ));
    } else {
        let npx_cmd = runner.run(
            &CommandSpec::new("npx")
                .cwd(&openclaw_dir)
                .args(["--yes", "pnpm@10", "install"])
                .capture_name("openclaw-npx-pnpm-install"),
        )?;
        command_outcomes.push(CommandOutcomeRecord::from_output(
            "openclaw-npx-pnpm-install",
            &npx_cmd,
        ));
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
    let openclaw_config_copy = artifact_dir.join("openclaw.json");
    fs::copy(&openclaw_config_path, &openclaw_config_copy)?;

    let mut gateway = runner.spawn(
        &CommandSpec::node()
            .cwd(&openclaw_dir)
            .env(
                "OPENCLAW_STATE_DIR",
                openclaw_state_dir.to_string_lossy().to_string(),
            )
            .env(
                "OPENCLAW_CONFIG_PATH",
                openclaw_config_path.to_string_lossy().to_string(),
            )
            .env("OPENCLAW_GATEWAY_TOKEN", gw_token)
            .env("OPENCLAW_SKIP_BROWSER_CONTROL_SERVER", "1")
            .env("OPENCLAW_SKIP_GMAIL_WATCHER", "1")
            .env("OPENCLAW_SKIP_CANVAS_HOST", "1")
            .env("OPENCLAW_SKIP_CRON", "1")
            .arg("scripts/run-node.mjs")
            .arg("gateway")
            .arg("--port")
            .arg(gw_port.to_string())
            .arg("--allow-unconfigured")
            .capture_name("openclaw-gateway"),
    )?;

    let openclaw_log = gateway.stdout_path.clone();
    let openclaw_err = gateway.stderr_path.clone();

    let peer_pubkey = wait_for_sidecar_keypackage(&relay_url, &sidecar_state_dir, &mut gateway)
        .await
        .inspect_err(|err| {
            eprintln!(
                "OpenClaw/pikachat-openclaw bot did not publish a usable keypackage: {err:#}"
            );
        });

    let peer_pubkey = match peer_pubkey {
        Ok(peer_pubkey) => peer_pubkey,
        Err(err) => {
            eprintln!(
                "openclaw e2e failed; artifacts preserved at: {}",
                artifact_dir.display()
            );
            let _ = artifacts::write_failure_tail(&context, "openclaw-gateway", &openclaw_log, 120);
            eprintln!("{}", tail_lines(&openclaw_log, 120));
            return Err(err);
        }
    };

    let scenario_run = runner.run(&pikachat_peer_spec(
        &root,
        &sidecar_cmd,
        &relay_url,
        context.state_dir(),
        &peer_pubkey,
    ));

    let scenario_output = match scenario_run {
        Ok(output) => output,
        Err(err) => {
            eprintln!(
                "openclaw e2e failed; artifacts preserved at: {}",
                artifact_dir.display()
            );
            let _ = artifacts::write_failure_tail(&context, "openclaw-gateway", &openclaw_log, 120);
            eprintln!("{}", tail_lines(&openclaw_log, 120));
            return Err(err);
        }
    };
    command_outcomes.push(CommandOutcomeRecord::from_output(
        "openclaw-invite-and-chat-peer",
        &scenario_output,
    ));

    let mut result = ScenarioRunOutput::completed(context.state_dir().to_path_buf())
        .with_artifact(openclaw_log)
        .with_artifact(openclaw_err)
        .with_artifact(openclaw_config_copy)
        .with_artifact(scenario_output.stdout_path.clone())
        .with_artifact(scenario_output.stderr_path.clone())
        .with_metadata("relay_url", relay_url)
        .with_metadata("tenant_relay_namespace", tenant_relay_namespace)
        .with_metadata("tenant_moq_namespace", tenant_moq_namespace)
        .with_metadata("openclaw_dir", openclaw_dir.to_string_lossy().to_string())
        .with_metadata("gateway_port", gw_port.to_string());
    let summary = artifacts::write_standard_summary(
        &context,
        "openclaw::gateway_e2e",
        &result,
        command_outcomes,
        BTreeMap::new(),
    )?;
    result = result.with_summary(summary);

    context.mark_success();
    Ok(result)
}
