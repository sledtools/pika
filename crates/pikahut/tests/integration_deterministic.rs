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

#[tokio::test]
#[ignore = "integration scenario; run in deterministic lane"]
async fn cli_smoke_local() -> Result<()> {
    scenarios::run_cli_smoke(CliSmokeRequest {
        relay: None,
        with_media: false,
        state_dir: None,
    })
    .await
    .map(|_| ())
}

#[tokio::test]
#[ignore = "integration scenario; run in deterministic lane with network"]
async fn cli_smoke_media_local() -> Result<()> {
    if skip_if_missing_requirements(&workspace_root(), &[Requirement::PublicNetwork]) {
        return Ok(());
    }

    scenarios::run_cli_smoke(CliSmokeRequest {
        relay: None,
        with_media: true,
        state_dir: None,
    })
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
    scenarios::run_scenario(ScenarioRequest {
        scenario: "invite-and-chat".to_string(),
        state_dir: None,
        relay: None,
        extra_args: Vec::new(),
    })
    .await
    .map(|_| ())
}

#[tokio::test]
#[ignore = "deterministic OpenClaw scenario"]
async fn openclaw_scenario_invite_and_chat_rust_bot() -> Result<()> {
    scenarios::run_scenario(ScenarioRequest {
        scenario: "invite-and-chat-rust-bot".to_string(),
        state_dir: None,
        relay: None,
        extra_args: Vec::new(),
    })
    .await
    .map(|_| ())
}

#[tokio::test]
#[ignore = "deterministic OpenClaw scenario"]
async fn openclaw_scenario_invite_and_chat_daemon() -> Result<()> {
    scenarios::run_scenario(ScenarioRequest {
        scenario: "invite-and-chat-daemon".to_string(),
        state_dir: None,
        relay: None,
        extra_args: Vec::new(),
    })
    .await
    .map(|_| ())
}

#[tokio::test]
#[ignore = "deterministic OpenClaw scenario"]
async fn openclaw_scenario_audio_echo() -> Result<()> {
    scenarios::run_scenario(ScenarioRequest {
        scenario: "audio-echo".to_string(),
        state_dir: None,
        relay: None,
        extra_args: Vec::new(),
    })
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
