use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow, bail};
use serde_json::Value;

use crate::config::{self, BotOverlay, OverlayConfig, ProfileName};
use crate::testing::{
    ArtifactPolicy, CommandOutput, CommandRunner, CommandSpec, FixtureSpec, TestContext,
    start_fixture,
};

use super::artifacts::{self, CommandOutcomeRecord};
use super::common::{
    command_exists, env_truthy, extract_udid, in_ci, parse_url_port, resolve_ui_client_nsec,
};
use super::types::{
    CliSmokeRequest, ScenarioRequest, ScenarioRunOutput, UiE2eLocalRequest, UiPlatform,
};

fn build_context(
    run_name: &str,
    state_dir: Option<PathBuf>,
    artifact_policy: ArtifactPolicy,
) -> Result<TestContext> {
    let mut builder = TestContext::builder(run_name).artifact_policy(artifact_policy);
    if let Some(path) = state_dir {
        builder = builder.state_dir(path);
    }
    builder.build()
}

fn pikachat_spec(root: &Path, args: &[String], capture: &str) -> CommandSpec {
    CommandSpec::cargo()
        .cwd(root)
        .args(["run", "-q", "-p", "pikachat", "--"])
        .args(args.iter().cloned())
        .capture_name(capture)
}

fn run_pikachat_ok(
    runner: &CommandRunner<'_>,
    root: &Path,
    args: &[String],
    capture: &str,
) -> Result<CommandOutput> {
    runner.run(&pikachat_spec(root, args, capture))
}

fn run_pikachat_json(
    runner: &CommandRunner<'_>,
    root: &Path,
    args: &[String],
    capture: &str,
) -> Result<(Value, CommandOutput)> {
    let output = runner.run(&pikachat_spec(root, args, capture))?;
    let stdout = String::from_utf8(output.stdout.clone()).context("pikachat output not utf-8")?;
    let value = serde_json::from_str(stdout.trim())
        .with_context(|| format!("failed to parse pikachat JSON output: {stdout}"))?;
    Ok((value, output))
}

fn run_pikachat_allow_failure(
    runner: &CommandRunner<'_>,
    root: &Path,
    args: &[String],
    capture: &str,
) -> Option<CommandOutput> {
    runner.run(&pikachat_spec(root, args, capture)).ok()
}

fn path_arg(path: &Path) -> String {
    path.to_string_lossy().to_string()
}

pub async fn run_scenario(args: ScenarioRequest) -> Result<ScenarioRunOutput> {
    let root = config::find_workspace_root()?;
    let mut context = build_context(
        &format!("scenario-{}", args.scenario),
        args.state_dir,
        ArtifactPolicy::PreserveOnFailure,
    )?;

    let fixture = if args.relay.is_none() {
        Some(start_fixture(&context, &FixtureSpec::builder(ProfileName::Relay).build()).await?)
    } else {
        None
    };

    let relay_url = match args.relay {
        Some(relay) => relay,
        None => fixture
            .as_ref()
            .and_then(|handle| handle.relay_url().map(ToOwned::to_owned))
            .ok_or_else(|| anyhow!("fixture manifest missing relay_url"))?,
    };

    let runner = CommandRunner::new(&context);
    let scenario_name = args.scenario.clone();
    let mut scenario_args = vec![
        "scenario".to_string(),
        scenario_name.clone(),
        "--relay".to_string(),
        relay_url.clone(),
        "--state-dir".to_string(),
        path_arg(context.state_dir()),
    ];
    scenario_args.extend(args.extra_args);

    let output = runner.run(
        &CommandSpec::cargo()
            .cwd(&root)
            .args(["run", "-q", "-p", "pikachat", "--"])
            .args(scenario_args)
            .capture_name("pikachat-scenario"),
    )?;

    let mut result = ScenarioRunOutput::completed(context.state_dir().to_path_buf())
        .with_artifact(output.stdout_path.clone())
        .with_artifact(output.stderr_path.clone())
        .with_metadata("relay_url", relay_url)
        .with_metadata("scenario_name", scenario_name);
    let summary = artifacts::write_standard_summary(
        &context,
        "deterministic::scenario",
        &result,
        vec![CommandOutcomeRecord::from_output(
            "pikachat-scenario",
            &output,
        )],
        BTreeMap::new(),
    )?;
    result = result.with_summary(summary);

    context.mark_success();
    Ok(result)
}

pub async fn run_cli_smoke(args: CliSmokeRequest) -> Result<ScenarioRunOutput> {
    let root = config::find_workspace_root()?;
    let mut context = build_context(
        "cli-smoke",
        args.state_dir,
        ArtifactPolicy::PreserveOnFailure,
    )?;

    let fixture = if args.relay.is_none() {
        Some(start_fixture(&context, &FixtureSpec::builder(ProfileName::Relay).build()).await?)
    } else {
        None
    };

    let relay_url = match args.relay {
        Some(relay) => relay,
        None => fixture
            .as_ref()
            .and_then(|handle| handle.relay_url().map(ToOwned::to_owned))
            .ok_or_else(|| anyhow!("fixture manifest missing relay_url"))?,
    };

    let runner = CommandRunner::new(&context);
    let mut command_outcomes = Vec::new();
    let alice_state = context.state_dir().join("alice");
    let bob_state = context.state_dir().join("bob");

    println!("=== Alice: create identity ===");
    let (alice, alice_identity_cmd) = run_pikachat_json(
        &runner,
        &root,
        &[
            "--state-dir".into(),
            path_arg(&alice_state),
            "--relay".into(),
            relay_url.clone(),
            "identity".into(),
        ],
        "cli-smoke-alice-identity",
    )?;
    command_outcomes.push(CommandOutcomeRecord::from_output(
        "cli-smoke-alice-identity",
        &alice_identity_cmd,
    ));
    let alice_pk = alice
        .get("pubkey")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("missing pubkey in alice identity output"))?
        .to_string();
    println!("Alice pubkey: {alice_pk}");

    println!("=== Bob: create identity ===");
    let (bob, bob_identity_cmd) = run_pikachat_json(
        &runner,
        &root,
        &[
            "--state-dir".into(),
            path_arg(&bob_state),
            "--relay".into(),
            relay_url.clone(),
            "identity".into(),
        ],
        "cli-smoke-bob-identity",
    )?;
    command_outcomes.push(CommandOutcomeRecord::from_output(
        "cli-smoke-bob-identity",
        &bob_identity_cmd,
    ));
    let bob_pk = bob
        .get("pubkey")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("missing pubkey in bob identity output"))?
        .to_string();
    println!("Bob pubkey: {bob_pk}");

    println!("=== Both: publish key packages ===");
    let alice_publish_cmd = run_pikachat_ok(
        &runner,
        &root,
        &[
            "--state-dir".into(),
            path_arg(&alice_state),
            "--relay".into(),
            relay_url.clone(),
            "publish-kp".into(),
        ],
        "cli-smoke-alice-publish-kp",
    )?;
    command_outcomes.push(CommandOutcomeRecord::from_output(
        "cli-smoke-alice-publish-kp",
        &alice_publish_cmd,
    ));
    let bob_publish_cmd = run_pikachat_ok(
        &runner,
        &root,
        &[
            "--state-dir".into(),
            path_arg(&bob_state),
            "--relay".into(),
            relay_url.clone(),
            "publish-kp".into(),
        ],
        "cli-smoke-bob-publish-kp",
    )?;
    command_outcomes.push(CommandOutcomeRecord::from_output(
        "cli-smoke-bob-publish-kp",
        &bob_publish_cmd,
    ));

    println!("=== Alice: invite Bob ===");
    let (invite, invite_cmd) = run_pikachat_json(
        &runner,
        &root,
        &[
            "--state-dir".into(),
            path_arg(&alice_state),
            "--relay".into(),
            relay_url.clone(),
            "invite".into(),
            "--peer".into(),
            bob_pk,
        ],
        "cli-smoke-invite",
    )?;
    command_outcomes.push(CommandOutcomeRecord::from_output(
        "cli-smoke-invite",
        &invite_cmd,
    ));
    let group = invite
        .get("nostr_group_id")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("missing nostr_group_id in invite output"))?
        .to_string();
    println!("Group: {group}");

    println!("=== Bob: sync welcomes (listen 3s) ===");
    if let Some(cmd) = run_pikachat_allow_failure(
        &runner,
        &root,
        &[
            "--state-dir".into(),
            path_arg(&bob_state),
            "--relay".into(),
            relay_url.clone(),
            "listen".into(),
            "--timeout".into(),
            "3".into(),
            "--lookback".into(),
            "300".into(),
        ],
        "cli-smoke-listen-welcomes",
    ) {
        command_outcomes.push(CommandOutcomeRecord::from_output(
            "cli-smoke-listen-welcomes",
            &cmd,
        ));
    }

    println!("=== Bob: check welcomes ===");
    let (welcomes, welcomes_cmd) = run_pikachat_json(
        &runner,
        &root,
        &[
            "--state-dir".into(),
            path_arg(&bob_state),
            "--relay".into(),
            relay_url.clone(),
            "welcomes".into(),
        ],
        "cli-smoke-welcomes",
    )?;
    command_outcomes.push(CommandOutcomeRecord::from_output(
        "cli-smoke-welcomes",
        &welcomes_cmd,
    ));
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
    let accept_cmd = run_pikachat_ok(
        &runner,
        &root,
        &[
            "--state-dir".into(),
            path_arg(&bob_state),
            "--relay".into(),
            relay_url.clone(),
            "accept-welcome".into(),
            "--wrapper-event-id".into(),
            wrapper,
        ],
        "cli-smoke-accept-welcome",
    )?;
    command_outcomes.push(CommandOutcomeRecord::from_output(
        "cli-smoke-accept-welcome",
        &accept_cmd,
    ));

    let text_probe = "hello from alice";
    println!("=== Alice: send message ===");
    let send_cmd = run_pikachat_ok(
        &runner,
        &root,
        &[
            "--state-dir".into(),
            path_arg(&alice_state),
            "--relay".into(),
            relay_url.clone(),
            "send".into(),
            "--group".into(),
            group.clone(),
            "--content".into(),
            text_probe.into(),
        ],
        "cli-smoke-send-message",
    )?;
    command_outcomes.push(CommandOutcomeRecord::from_output(
        "cli-smoke-send-message",
        &send_cmd,
    ));

    println!("=== Bob: sync inbox (listen 3s) ===");
    if let Some(cmd) = run_pikachat_allow_failure(
        &runner,
        &root,
        &[
            "--state-dir".into(),
            path_arg(&bob_state),
            "--relay".into(),
            relay_url.clone(),
            "listen".into(),
            "--timeout".into(),
            "3".into(),
            "--lookback".into(),
            "300".into(),
        ],
        "cli-smoke-listen-inbox",
    ) {
        command_outcomes.push(CommandOutcomeRecord::from_output(
            "cli-smoke-listen-inbox",
            &cmd,
        ));
    }

    println!("=== Bob: read messages ===");
    let (messages, messages_cmd) = run_pikachat_json(
        &runner,
        &root,
        &[
            "--state-dir".into(),
            path_arg(&bob_state),
            "--relay".into(),
            relay_url.clone(),
            "messages".into(),
            "--group".into(),
            group.clone(),
        ],
        "cli-smoke-messages",
    )?;
    command_outcomes.push(CommandOutcomeRecord::from_output(
        "cli-smoke-messages",
        &messages_cmd,
    ));
    println!("{}", serde_json::to_string_pretty(&messages)?);
    let saw_probe = messages
        .get("messages")
        .and_then(Value::as_array)
        .map(|msgs| {
            msgs.iter().any(|msg| {
                msg.get("content")
                    .and_then(Value::as_str)
                    .map(|content| content == text_probe)
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false);
    if !saw_probe {
        bail!("bob inbox does not contain expected message content: {text_probe}");
    }

    if args.with_media {
        println!("=== Alice: send media ===");
        let media_src = context.state_dir().join("sample-media.txt");
        fs::write(
            &media_src,
            format!(
                "hello media {}\n",
                SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs()
            ),
        )?;

        let send_media_cmd = run_pikachat_ok(
            &runner,
            &root,
            &[
                "--state-dir".into(),
                path_arg(&alice_state),
                "--relay".into(),
                relay_url.clone(),
                "send".into(),
                "--group".into(),
                group.clone(),
                "--media".into(),
                path_arg(&media_src),
                "--mime-type".into(),
                "text/plain".into(),
                "--content".into(),
                "media from alice".into(),
            ],
            "cli-smoke-send-media",
        )?;
        command_outcomes.push(CommandOutcomeRecord::from_output(
            "cli-smoke-send-media",
            &send_media_cmd,
        ));

        println!("=== Bob: sync media message (listen 5s) ===");
        if let Some(cmd) = run_pikachat_allow_failure(
            &runner,
            &root,
            &[
                "--state-dir".into(),
                path_arg(&bob_state),
                "--relay".into(),
                relay_url.clone(),
                "listen".into(),
                "--timeout".into(),
                "5".into(),
                "--lookback".into(),
                "300".into(),
            ],
            "cli-smoke-listen-media",
        ) {
            command_outcomes.push(CommandOutcomeRecord::from_output(
                "cli-smoke-listen-media",
                &cmd,
            ));
        }

        println!("=== Bob: read messages with media ===");
        let (bob_msgs, media_messages_cmd) = run_pikachat_json(
            &runner,
            &root,
            &[
                "--state-dir".into(),
                path_arg(&bob_state),
                "--relay".into(),
                relay_url.clone(),
                "messages".into(),
                "--group".into(),
                group.clone(),
            ],
            "cli-smoke-media-messages",
        )?;
        command_outcomes.push(CommandOutcomeRecord::from_output(
            "cli-smoke-media-messages",
            &media_messages_cmd,
        ));
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
        let media_out = context.state_dir().join("bob-downloaded-media.txt");
        let download_cmd = run_pikachat_ok(
            &runner,
            &root,
            &[
                "--state-dir".into(),
                path_arg(&bob_state),
                "--relay".into(),
                relay_url.clone(),
                "download-media".into(),
                media_msg_id,
                "--output".into(),
                path_arg(&media_out),
            ],
            "cli-smoke-download-media",
        )?;
        command_outcomes.push(CommandOutcomeRecord::from_output(
            "cli-smoke-download-media",
            &download_cmd,
        ));

        let src = fs::read(&media_src)?;
        let out = fs::read(&media_out)?;
        if src != out {
            bail!("downloaded media does not match source file");
        }
    }

    println!("=== SMOKE TEST PASSED ===");
    let mut result = ScenarioRunOutput::completed(context.state_dir().to_path_buf())
        .with_metadata("relay_url", relay_url)
        .with_metadata("with_media", args.with_media.to_string());
    let summary = artifacts::write_standard_summary(
        &context,
        "deterministic::cli_smoke",
        &result,
        command_outcomes,
        BTreeMap::new(),
    )?;
    result = result.with_summary(summary);
    context.mark_success();
    Ok(result)
}

pub async fn run_ui_e2e_local(args: UiE2eLocalRequest) -> Result<ScenarioRunOutput> {
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
                return Ok(ScenarioRunOutput::skipped(
                    "android ui e2e local requires adb+emulator on PATH",
                ));
            }
            bail!("missing adb/emulator on PATH; run inside nix develop");
        }

        let list = CommandSpec::new(&emulator_bin)
            .arg("-list-avds")
            .cwd(&root)
            .capture_name("android-list-avds");
        let mut context = build_context(
            "ui-e2e-local-android-capability",
            None,
            ArtifactPolicy::PreserveOnFailure,
        )?;
        let runner = CommandRunner::new(&context);
        let avds_out = runner.run(&list)?;
        let avds = String::from_utf8_lossy(&avds_out.stdout);
        let found = avds.lines().any(|line| line.trim() == avd_name);
        if !found {
            if in_ci() {
                eprintln!(
                    "SKIP: android ui e2e local requires AVD '{}' (not present)",
                    avd_name
                );
                return Ok(ScenarioRunOutput::skipped(format!(
                    "android ui e2e local requires AVD '{avd_name}' (not present)"
                )));
            }
            bail!("android AVD '{}' not found", avd_name);
        }
        // Capability probe context is intentionally not marked success so state is preserved on failures.
        context.mark_success();
    }

    let policy = if keep {
        ArtifactPolicy::PreserveAlways
    } else {
        ArtifactPolicy::PreserveOnFailure
    };
    let mut context = build_context("ui-e2e-local", args.state_dir, policy)?;

    let overlay = OverlayConfig {
        bot: Some(BotOverlay {
            timeout_secs: Some(bot_timeout_sec),
        }),
        ..OverlayConfig::default()
    };

    let fixture = start_fixture(
        &context,
        &FixtureSpec::builder(ProfileName::RelayBot)
            .overlay(overlay)
            .build(),
    )
    .await?;

    let relay_url = fixture
        .relay_url()
        .ok_or_else(|| anyhow!("manifest missing relay_url"))?
        .to_string();
    let bot_npub = fixture
        .bot_npub()
        .ok_or_else(|| anyhow!("manifest missing bot_npub"))?
        .to_string();
    let relay_port = parse_url_port(&relay_url)?;
    let android_relay_url = format!("ws://10.0.2.2:{relay_port}");

    println!("relay_url={relay_url}");
    println!("android_relay_url={android_relay_url}");
    println!("bot_npub={bot_npub}");

    let client_nsec = resolve_ui_client_nsec(&root)?;
    let runner = CommandRunner::new(&context);
    let mut command_outcomes = Vec::new();

    match args.platform {
        UiPlatform::Android => {
            let test_class = std::env::var("PIKA_ANDROID_E2E_TEST_CLASS")
                .unwrap_or_else(|_| "com.pika.app.PikaE2eUiTest".to_string());
            let test_suffix = std::env::var("PIKA_ANDROID_TEST_APPLICATION_ID_SUFFIX")
                .unwrap_or_else(|_| ".test".to_string());
            let test_app_id = format!("org.pikachat.pika{test_suffix}");

            let emulator_ensure = runner.run(
                &CommandSpec::new(
                    root.join("tools/android-emulator-ensure")
                        .to_string_lossy()
                        .to_string(),
                )
                .cwd(&root)
                .capture_name("android-emulator-ensure"),
            )?;
            command_outcomes.push(CommandOutcomeRecord::from_output(
                "android-emulator-ensure",
                &emulator_ensure,
            ));

            let android_prepare = runner.run(
                &CommandSpec::new("just")
                    .cwd(&root)
                    .args(["gen-kotlin", "android-rust", "android-local-properties"])
                    .capture_name("android-prepare"),
            )?;
            command_outcomes.push(CommandOutcomeRecord::from_output(
                "android-prepare",
                &android_prepare,
            ));

            let ensure_installable = runner.run(
                &CommandSpec::new(
                    root.join("tools/android-ensure-debug-installable")
                        .to_string_lossy()
                        .to_string(),
                )
                .cwd(&root)
                .env("PIKA_ANDROID_APP_ID", &test_app_id)
                .capture_name("android-ensure-installable"),
            )?;
            command_outcomes.push(CommandOutcomeRecord::from_output(
                "android-ensure-installable",
                &ensure_installable,
            ));

            let android_ui = runner.run(
                &CommandSpec::gradlew()
                    .cwd(root.join("android"))
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
                    ))
                    .capture_name("android-ui-e2e-local"),
            )?;
            command_outcomes.push(CommandOutcomeRecord::from_output(
                "android-ui-e2e-local",
                &android_ui,
            ));
        }
        UiPlatform::Ios => {
            let ios_prepare = runner.run(
                &CommandSpec::new("just")
                    .cwd(&root)
                    .args(["ios-xcframework", "ios-xcodeproj"])
                    .capture_name("ios-prepare"),
            )?;
            command_outcomes.push(CommandOutcomeRecord::from_output(
                "ios-prepare",
                &ios_prepare,
            ));

            let sim_output = runner.run(
                &CommandSpec::new(
                    root.join("tools/ios-sim-ensure")
                        .to_string_lossy()
                        .to_string(),
                )
                .cwd(&root)
                .env("PIKA_UI_E2E_NSEC", &client_nsec)
                .env("PIKA_UI_E2E_BOT_NPUB", &bot_npub)
                .env("PIKA_UI_E2E_RELAYS", &relay_url)
                .env("PIKA_UI_E2E_KP_RELAYS", &relay_url)
                .capture_name("ios-sim-ensure"),
            )?;
            command_outcomes.push(CommandOutcomeRecord::from_output(
                "ios-sim-ensure",
                &sim_output,
            ));
            let sim_stdout = String::from_utf8_lossy(&sim_output.stdout);
            let udid = extract_udid(&sim_stdout).ok_or_else(|| {
                anyhow!("could not determine simulator udid from ios-sim-ensure output")
            })?;

            let ios_ui = runner.run(
                &CommandSpec::new(root.join("tools/xcode-run").to_string_lossy().to_string())
                    .cwd(&root)
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
                    ])
                    .capture_name("ios-ui-e2e-local"),
            )?;
            command_outcomes.push(CommandOutcomeRecord::from_output(
                "ios-ui-e2e-local",
                &ios_ui,
            ));
        }
        UiPlatform::Desktop => {
            let desktop_ui = runner.run(
                &CommandSpec::cargo()
                    .cwd(&root)
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
                    ])
                    .capture_name("desktop-ui-e2e-local"),
            )?;
            command_outcomes.push(CommandOutcomeRecord::from_output(
                "desktop-ui-e2e-local",
                &desktop_ui,
            ));
        }
    }

    let mut result = ScenarioRunOutput::completed(context.state_dir().to_path_buf())
        .with_metadata("relay_url", relay_url)
        .with_metadata("bot_npub", bot_npub)
        .with_metadata("platform", format!("{:?}", args.platform));
    let summary = artifacts::write_standard_summary(
        &context,
        "deterministic::ui_e2e_local",
        &result,
        command_outcomes,
        BTreeMap::new(),
    )?;
    result = result.with_summary(summary);

    context.mark_success();
    Ok(result)
}
