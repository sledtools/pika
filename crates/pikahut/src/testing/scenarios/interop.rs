use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};

use crate::config::{self, ProfileName};
use crate::health;
use crate::testing::{
    ArtifactPolicy, CommandRunner, CommandSpec, FixtureSpec, TestContext, start_fixture,
};

use super::artifacts::{self, CommandOutcomeRecord};
use super::common::{check_mdk_skew, dirs_home, extract_field, parse_url_port};
use super::types::{InteropRustBaselineRequest, ScenarioRunOutput};

fn build_context(state_dir: Option<PathBuf>, keep: bool) -> Result<TestContext> {
    let policy = if keep {
        ArtifactPolicy::PreserveAlways
    } else {
        ArtifactPolicy::PreserveOnFailure
    };

    let mut builder = TestContext::builder("interop-rust-baseline").artifact_policy(policy);
    if let Some(path) = state_dir {
        builder = builder.state_dir(path);
    }
    builder.build()
}

pub async fn run_interop_rust_baseline(
    args: InteropRustBaselineRequest,
) -> Result<ScenarioRunOutput> {
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

    let mut context = build_context(explicit_state, args.keep)?;
    let fixture =
        start_fixture(&context, &FixtureSpec::builder(ProfileName::Relay).build()).await?;
    let relay_url = fixture
        .relay_url()
        .ok_or_else(|| anyhow!("manifest missing relay_url"))?
        .to_string();
    let relay_port = parse_url_port(&relay_url)?;
    let android_relay_url = format!("ws://10.0.2.2:{relay_port}");

    println!("relay_url={relay_url}");
    println!("android_relay_url={android_relay_url}");

    let bot_timeout_sec = args.bot_timeout_sec.unwrap_or_else(|| {
        std::env::var("BOT_TIMEOUT_SEC")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(900)
    });

    let runner = CommandRunner::new(&context);
    let spawn = runner.spawn(
        &CommandSpec::cargo()
            .cwd(&rust_interop_dir)
            .args(["run", "-q", "-p", "rust_harness", "--", "bot", "--relay"])
            .arg(relay_url.clone())
            .args([
                "--state-dir",
                &context.state_dir().join("bot").to_string_lossy(),
            ])
            .args(["--timeout-sec", &bot_timeout_sec.to_string()])
            .capture_name("interop-rust-bot"),
    )?;
    let bot_log = spawn.stdout_path.clone();
    let bot_err = spawn.stderr_path.clone();
    let _bot_guard = spawn;
    let mut command_outcomes = Vec::new();

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
        let baseline_cmd = runner.run(
            &CommandSpec::cargo()
                .cwd(&root)
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
                .arg(&bot_npub)
                .capture_name("interop-rust-baseline"),
        )?;
        command_outcomes.push(CommandOutcomeRecord::from_output(
            "interop-rust-baseline",
            &baseline_cmd,
        ));
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

    let mut result = ScenarioRunOutput::completed(context.state_dir().to_path_buf())
        .with_artifact(bot_log)
        .with_artifact(bot_err)
        .with_metadata("relay_url", relay_url)
        .with_metadata("bot_npub", bot_npub)
        .with_metadata("manual", args.manual.to_string());
    let summary = artifacts::write_standard_summary(
        &context,
        "interop::rust_baseline",
        &result,
        command_outcomes,
        BTreeMap::new(),
    )?;
    result = result.with_summary(summary);

    context.mark_success();
    Ok(result)
}
