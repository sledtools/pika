use std::path::{Path, PathBuf};

use anyhow::Result;

use pikahut::testing::{
    ArtifactPolicy, CommandRunner, CommandSpec, Requirement, TestContext, scenarios,
    scenarios::{
        CliSmokeRequest, InteropRustBaselineRequest, ScenarioRequest, UiE2eLocalRequest, UiPlatform,
    },
    skip_if_missing_requirements,
};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap_or_else(|_| PathBuf::from(env!("CARGO_MANIFEST_DIR")))
}

fn env_opt(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn env_path(name: &str) -> Option<PathBuf> {
    env_opt(name).map(PathBuf::from)
}

fn cli_smoke_request(with_media: bool) -> CliSmokeRequest {
    CliSmokeRequest {
        relay: env_opt("PIKAHUT_CLI_SMOKE_RELAY"),
        with_media,
        state_dir: env_path("PIKAHUT_CLI_SMOKE_STATE_DIR"),
    }
}

fn scenario_extra_args() -> Vec<String> {
    const SEP: char = '\u{1f}';
    env_opt("PIKAHUT_SCENARIO_EXTRA_ARGS")
        .map(|raw| raw.split(SEP).map(str::to_string).collect())
        .unwrap_or_default()
}

fn scenario_request(name: &str) -> ScenarioRequest {
    ScenarioRequest {
        scenario: name.to_string(),
        state_dir: env_path("PIKAHUT_SCENARIO_STATE_DIR"),
        relay: env_opt("PIKAHUT_SCENARIO_RELAY_URL"),
        extra_args: scenario_extra_args(),
    }
}

#[tokio::test]
#[ignore = "integration scenario; run in deterministic lane"]
async fn cli_smoke_local() -> Result<()> {
    scenarios::run_cli_smoke(cli_smoke_request(false))
        .await
        .map(|_| ())
}

#[tokio::test]
#[ignore = "integration scenario; run in deterministic lane with network"]
async fn cli_smoke_media_local() -> Result<()> {
    if skip_if_missing_requirements(&workspace_root(), &[Requirement::PublicNetwork]) {
        return Ok(());
    }

    scenarios::run_cli_smoke(cli_smoke_request(true))
        .await
        .map(|_| ())
}

#[tokio::test]
#[ignore = "requires Android SDK/emulator"]
async fn ui_e2e_local_android() -> Result<()> {
    if skip_if_missing_requirements(
        &workspace_root(),
        &[Requirement::AndroidTools, Requirement::AndroidEmulatorAvd],
    ) {
        return Ok(());
    }

    scenarios::run_ui_e2e_local(UiE2eLocalRequest {
        platform: UiPlatform::Android,
        state_dir: None,
        keep: false,
        bot_timeout_sec: None,
    })
    .await
    .map(|_| ())
}

#[tokio::test]
#[ignore = "requires macOS + Xcode"]
async fn ui_e2e_local_ios() -> Result<()> {
    if skip_if_missing_requirements(
        &workspace_root(),
        &[Requirement::HostMacOs, Requirement::Xcode],
    ) {
        return Ok(());
    }

    scenarios::run_ui_e2e_local(UiE2eLocalRequest {
        platform: UiPlatform::Ios,
        state_dir: None,
        keep: false,
        bot_timeout_sec: None,
    })
    .await
    .map(|_| ())
}

#[tokio::test]
#[ignore = "desktop UI e2e can be heavy in CI"]
async fn ui_e2e_local_desktop() -> Result<()> {
    scenarios::run_ui_e2e_local(UiE2eLocalRequest {
        platform: UiPlatform::Desktop,
        state_dir: None,
        keep: false,
        bot_timeout_sec: None,
    })
    .await
    .map(|_| ())
}

#[tokio::test]
#[ignore = "requires external rust interop repo"]
async fn interop_rust_baseline() -> Result<()> {
    if skip_if_missing_requirements(&workspace_root(), &[Requirement::InteropRustRepo]) {
        return Ok(());
    }

    scenarios::run_interop_rust_baseline(InteropRustBaselineRequest {
        manual: false,
        keep: false,
        state_dir: None,
        rust_interop_dir: None,
        bot_timeout_sec: None,
    })
    .await
    .map(|_| ())
}

#[tokio::test]
#[ignore = "deterministic OpenClaw scenario"]
async fn openclaw_scenario_invite_and_chat() -> Result<()> {
    scenarios::run_scenario(scenario_request("invite-and-chat"))
        .await
        .map(|_| ())
}

#[tokio::test]
#[ignore = "deterministic OpenClaw scenario"]
async fn openclaw_scenario_invite_and_chat_rust_bot() -> Result<()> {
    scenarios::run_scenario(scenario_request("invite-and-chat-rust-bot"))
        .await
        .map(|_| ())
}

#[tokio::test]
#[ignore = "deterministic OpenClaw scenario"]
async fn openclaw_scenario_invite_and_chat_daemon() -> Result<()> {
    scenarios::run_scenario(scenario_request("invite-and-chat-daemon"))
        .await
        .map(|_| ())
}

#[tokio::test]
#[ignore = "deterministic OpenClaw scenario"]
async fn openclaw_scenario_audio_echo() -> Result<()> {
    scenarios::run_scenario(scenario_request("audio-echo"))
        .await
        .map(|_| ())
}

#[test]
#[ignore = "deterministic post-rebase regression selector"]
fn post_rebase_invalid_event_rejection_boundary() -> Result<()> {
    let mut context = TestContext::builder("regression-invalid-event-rejection")
        .artifact_policy(ArtifactPolicy::PreserveOnFailure)
        .build()?;
    let runner = CommandRunner::new(&context);
    runner.run(
        &CommandSpec::cargo()
            .cwd(workspace_root())
            .args([
                "test",
                "-p",
                "pika_core",
                "--test",
                "e2e_messaging",
                "call_invite_with_invalid_relay_auth_is_rejected",
                "--",
                "--nocapture",
            ])
            .capture_name("regression-invalid-event-rejection"),
    )?;
    context.mark_success();
    Ok(())
}

#[test]
#[ignore = "deterministic post-rebase regression selector"]
fn post_rebase_logout_session_convergence_boundary() -> Result<()> {
    let mut context = TestContext::builder("regression-logout-session-convergence")
        .artifact_policy(ArtifactPolicy::PreserveOnFailure)
        .build()?;
    let runner = CommandRunner::new(&context);
    runner.run(
        &CommandSpec::cargo()
            .cwd(workspace_root())
            .args([
                "test",
                "-p",
                "pika_core",
                "--test",
                "app_flows",
                "logout_resets_state",
                "--",
                "--nocapture",
            ])
            .capture_name("regression-logout-session-convergence"),
    )?;
    context.mark_success();
    Ok(())
}
