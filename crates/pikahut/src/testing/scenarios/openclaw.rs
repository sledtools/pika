use std::collections::BTreeMap;
use std::fs;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};

use crate::config::{self, ProfileName};
use crate::testing::{
    ArtifactPolicy, CommandRunner, CommandSpec, FixtureSpec, TestContext, start_fixture,
};

use super::artifacts::{self, CommandOutcomeRecord};
use super::common::{pick_free_port, resolve_openclaw_dir, tail_lines};
use super::types::{OpenclawE2eRequest, ScenarioRunOutput};

fn build_context(state_dir: Option<std::path::PathBuf>) -> Result<TestContext> {
    let mut builder =
        TestContext::builder("openclaw-e2e").artifact_policy(ArtifactPolicy::PreserveOnFailure);
    if let Some(path) = state_dir {
        builder = builder.state_dir(path);
    }
    builder.build()
}

pub async fn run_openclaw_e2e(args: OpenclawE2eRequest) -> Result<ScenarioRunOutput> {
    let root = config::find_workspace_root()?;
    let mut context = build_context(args.state_dir)?;

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

    let sidecar_cmd = root
        .join("target/debug/pikachat")
        .to_string_lossy()
        .to_string();
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

    let identity_path = sidecar_state_dir.join("identity.json");
    let mut ready = false;
    for _ in 0..80 {
        if identity_path.is_file() {
            ready = true;
            break;
        }
        if gateway.try_wait()?.is_some() {
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
        let _ = artifacts::write_failure_tail(&context, "openclaw-gateway", &openclaw_log, 120);
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
        .ok_or_else(|| anyhow!("identity.json missing public_key_hex"))?
        .to_string();

    let scenario_run = runner.run(
        &CommandSpec::cargo()
            .cwd(&root)
            .args(["run", "--manifest-path"])
            .arg(root.join("Cargo.toml").to_string_lossy().to_string())
            .args([
                "-p",
                "pikachat",
                "--",
                "scenario",
                "invite-and-chat-peer",
                "--relay",
            ])
            .arg(relay_url.clone())
            .args(["--state-dir", &context.state_dir().to_string_lossy()])
            .args(["--peer-pubkey", &peer_pubkey])
            .capture_name("openclaw-invite-and-chat-peer"),
    );

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
