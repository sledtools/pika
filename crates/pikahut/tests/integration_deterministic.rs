use std::path::{Path, PathBuf};

use anyhow::Result;
use tokio::sync::Mutex;

use pikahut::testing::{
    ArtifactPolicy, CommandRunner, CommandSpec, Requirement, TestContext, scenarios,
    scenarios::{
        CliSmokeRequest, InteropRustBaselineRequest, ScenarioRequest, UiE2eLocalRequest, UiPlatform,
    },
    skip_if_missing_requirements,
};

mod support;

fn workspace_root() -> PathBuf {
    pikahut::config::find_workspace_root().unwrap_or_else(|_| {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap_or_else(|_| PathBuf::from(env!("CARGO_MANIFEST_DIR")))
    })
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

fn staged_test_binary_spec(
    env_name: &str,
    test_name: &str,
    capture_name: &str,
) -> Option<CommandSpec> {
    env_path(env_name).map(|binary| {
        CommandSpec::new(binary.to_string_lossy().to_string())
            .cwd(workspace_root())
            .args([test_name, "--exact", "--nocapture"])
            .capture_name(capture_name)
    })
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

static ENV_LOCK: Mutex<()> = Mutex::const_new(());

struct ScopedEnvVar {
    key: String,
    previous: Option<String>,
}

impl ScopedEnvVar {
    fn set(key: &str, value: &str) -> Self {
        let previous = std::env::var(key).ok();
        // SAFETY: integration tests serialize env mutations via ENV_LOCK.
        unsafe {
            std::env::set_var(key, value);
        }
        Self {
            key: key.to_string(),
            previous,
        }
    }
}

impl Drop for ScopedEnvVar {
    fn drop(&mut self) {
        if let Some(previous) = &self.previous {
            // SAFETY: integration tests serialize env mutations via ENV_LOCK.
            unsafe {
                std::env::set_var(&self.key, previous);
            }
        } else {
            // SAFETY: integration tests serialize env mutations via ENV_LOCK.
            unsafe {
                std::env::remove_var(&self.key);
            }
        }
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
#[ignore = "deterministic messaging/profile selector"]
fn dm_creation_and_first_message_delivery_boundary() -> Result<()> {
    // Keep the narrower relay-backed message-state semantics in `rust/tests/e2e_messaging.rs`;
    // this selector owns the readable end-user contract that a DM shell appears, the first
    // message sends, and the peer sees that delivery through the same `FfiApp` surface the apps
    // exercise in CI.
    let mut context = TestContext::builder("dm-creation-and-first-message-delivery")
        .artifact_policy(ArtifactPolicy::PreserveOnFailure)
        .build()?;
    support::run_dm_creation_and_first_message_delivery(&context)?;
    context.mark_success();
    Ok(())
}

#[test]
#[ignore = "deterministic messaging/profile selector"]
fn late_joiner_group_profile_visibility_after_explicit_refresh_boundary() -> Result<()> {
    // Keep the narrower rebroadcast/member-state semantics in `rust/tests/e2e_group_profiles.rs`;
    // this selector owns only the readable user-facing contract that a late joiner opens the
    // group and sees member profile names after the existing members explicitly refresh them under
    // local fixtures.
    let mut context =
        TestContext::builder("late-joiner-group-profile-visibility-after-explicit-refresh")
            .artifact_policy(ArtifactPolicy::PreserveOnFailure)
            .build()?;
    support::run_late_joiner_group_profile_visibility_after_explicit_refresh(&context)?;
    context.mark_success();
    Ok(())
}

#[test]
#[ignore = "deterministic messaging/profile selector"]
fn dm_local_profile_override_visibility_boundary() -> Result<()> {
    // Keep the narrower per-chat profile-state semantics in
    // `rust/tests/e2e_group_profiles.rs`; this selector owns the readable DM-local contract that
    // the override is visible in the DM and does not leak into a separate chat with the same
    // peer.
    let mut context = TestContext::builder("dm-local-profile-override-visibility")
        .artifact_policy(ArtifactPolicy::PreserveOnFailure)
        .build()?;
    support::run_dm_local_profile_override_visibility(&context)?;
    context.mark_success();
    Ok(())
}

#[test]
#[ignore = "deterministic post-rebase regression selector"]
fn post_rebase_invalid_event_rejection_boundary() -> Result<()> {
    // Keep the narrow invalid-invite semantics owned by `rust/tests/e2e_messaging.rs`; this
    // selector exists to pin that behavior into the CI-facing deterministic contract.
    let mut context = TestContext::builder("regression-invalid-event-rejection")
        .artifact_policy(ArtifactPolicy::PreserveOnFailure)
        .build()?;
    let runner = CommandRunner::new(&context);
    let spec = staged_test_binary_spec(
        "PIKAHUT_TEST_PIKA_CORE_E2E_MESSAGING_BIN",
        "call_invite_with_invalid_relay_auth_is_rejected",
        "regression-invalid-event-rejection",
    )
    .unwrap_or_else(|| {
        CommandSpec::cargo()
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
            .capture_name("regression-invalid-event-rejection")
    });
    runner.run(&spec)?;
    context.mark_success();
    Ok(())
}

#[test]
#[ignore = "deterministic post-rebase regression selector"]
fn post_rebase_logout_session_convergence_boundary() -> Result<()> {
    // Keep the narrower single-app runtime-reset semantics in `rust/tests/app_flows.rs`; this
    // selector owns the readable lifecycle contract that logout clears Rust-owned app state and a
    // fresh process still starts clean until some outer layer explicitly restores a session.
    let mut context = TestContext::builder("regression-logout-session-convergence")
        .artifact_policy(ArtifactPolicy::PreserveOnFailure)
        .build()?;
    support::run_logout_reset_across_restart(&context)?;
    context.mark_success();
    Ok(())
}

#[test]
#[ignore = "deterministic startup/router selector"]
fn chat_deep_link_opens_note_to_self_boundary() -> Result<()> {
    // Keep lower-level create-account router semantics and invalid peer-key routing in
    // `rust/tests/app_flows.rs`; this checked-in deterministic selector captures the readable
    // signed-in deep-link contract that a raw `pika://chat/<npub>` payload lands in the intended
    // chat state.
    let mut context = TestContext::builder("chat-deep-link-note-to-self")
        .artifact_policy(ArtifactPolicy::PreserveOnFailure)
        .build()?;
    support::run_chat_deep_link_opens_note_to_self(&context)?;
    context.mark_success();
    Ok(())
}

#[test]
#[ignore = "deterministic external signer selector"]
fn external_signer_login_success_boundary() -> Result<()> {
    // Keep current-user hint and restored current-user plumbing in `rust/tests/app_flows.rs`;
    // this checked-in deterministic selector captures the readable direct signer login contract.
    let mut context = TestContext::builder("external-signer-login-success")
        .artifact_policy(ArtifactPolicy::PreserveOnFailure)
        .build()?;
    support::run_external_signer_login_success(&context)?;
    context.mark_success();
    Ok(())
}

#[test]
#[ignore = "deterministic external signer selector"]
fn external_signer_login_timeout_failure_boundary() -> Result<()> {
    // Keep lower-level timeout-to-toast mapping and current-user plumbing in
    // `rust/tests/app_flows.rs`; this checked-in deterministic selector captures the fuller
    // readable direct signer failure contract.
    let mut context = TestContext::builder("external-signer-login-timeout")
        .artifact_policy(ArtifactPolicy::PreserveOnFailure)
        .build()?;
    support::run_external_signer_login_timeout_failure(&context)?;
    context.mark_success();
    Ok(())
}

#[test]
#[ignore = "deterministic external signer selector"]
fn bunker_login_success_boundary() -> Result<()> {
    // Keep descriptor/client-key plumbing in `rust/tests/app_flows.rs`; this checked-in
    // deterministic selector captures the readable direct bunker login contract.
    let mut context = TestContext::builder("bunker-login-success")
        .artifact_policy(ArtifactPolicy::PreserveOnFailure)
        .build()?;
    support::run_bunker_login_success(&context)?;
    context.mark_success();
    Ok(())
}

#[test]
#[ignore = "deterministic external signer selector"]
fn bunker_login_invalid_uri_failure_boundary() -> Result<()> {
    // Keep lower-level bunker URI plumbing in `rust/tests/app_flows.rs`; this checked-in
    // deterministic selector captures the readable invalid-URI failure contract.
    let mut context = TestContext::builder("bunker-login-invalid-uri")
        .artifact_policy(ArtifactPolicy::PreserveOnFailure)
        .build()?;
    support::run_bunker_login_invalid_uri_failure(&context)?;
    context.mark_success();
    Ok(())
}

#[test]
#[ignore = "deterministic external signer selector"]
fn nostr_connect_login_success_boundary() -> Result<()> {
    // Keep the narrower callback-gating and retry semantics in `rust/tests/app_flows.rs`; this
    // checked-in deterministic selector captures the readable signer success contract. Native
    // tests still own callback URL injection/parsing glue.
    let mut context = TestContext::builder("nostr-connect-login-success")
        .artifact_policy(ArtifactPolicy::PreserveOnFailure)
        .build()?;
    support::run_nostr_connect_login_success(&context)?;
    context.mark_success();
    Ok(())
}

#[test]
#[ignore = "deterministic external signer selector"]
fn nostr_connect_new_secret_retry_boundary() -> Result<()> {
    // Keep the lower-level retry branch mechanics in `rust/tests/app_flows.rs`; this checked-in
    // deterministic selector captures only the readable recovery contract.
    let mut context = TestContext::builder("nostr-connect-new-secret-retry")
        .artifact_policy(ArtifactPolicy::PreserveOnFailure)
        .build()?;
    support::run_nostr_connect_new_secret_retry(&context)?;
    context.mark_success();
    Ok(())
}

#[test]
#[ignore = "deterministic external signer selector"]
fn nostr_connect_non_secret_rejection_stops_without_retry_boundary() -> Result<()> {
    // Keep the exact retry-sequence branch logic in `rust/tests/app_flows.rs`; this checked-in
    // deterministic selector captures the readable failure contract.
    let mut context = TestContext::builder("nostr-connect-non-secret-rejection")
        .artifact_policy(ArtifactPolicy::PreserveOnFailure)
        .build()?;
    support::run_nostr_connect_non_secret_rejection_stops_without_retry(&context)?;
    context.mark_success();
    Ok(())
}

#[test]
#[ignore = "deterministic external signer selector"]
fn pending_nostr_connect_login_survives_restart_boundary() -> Result<()> {
    // Keep the narrower pending-state persistence semantics in `rust/tests/app_flows.rs`; this
    // checked-in deterministic selector captures the readable restart contract.
    let mut context = TestContext::builder("nostr-connect-pending-restart")
        .artifact_policy(ArtifactPolicy::PreserveOnFailure)
        .build()?;
    support::run_pending_nostr_connect_login_survives_restart(&context)?;
    context.mark_success();
    Ok(())
}

#[test]
#[ignore = "deterministic external signer selector"]
fn restore_session_bunker_signs_in_boundary() -> Result<()> {
    // Keep the exact stored-client-key plumbing in `rust/tests/app_flows.rs`; this checked-in
    // deterministic selector captures the readable bunker-restore sign-in contract.
    let mut context = TestContext::builder("restore-session-bunker")
        .artifact_policy(ArtifactPolicy::PreserveOnFailure)
        .build()?;
    support::run_restore_session_bunker_signs_in(&context)?;
    context.mark_success();
    Ok(())
}

#[test]
#[ignore = "nightly call-path regression selector"]
fn call_over_local_moq_relay_boundary() -> Result<()> {
    let mut context = TestContext::builder("regression-call-over-local-moq-relay")
        .artifact_policy(ArtifactPolicy::PreserveOnFailure)
        .build()?;
    support::run_call_over_local_moq_relay(&context)?;
    context.mark_success();
    Ok(())
}

#[test]
#[ignore = "nightly call-path regression selector"]
fn call_with_pikachat_daemon_boundary() -> Result<()> {
    let _env_lock = ENV_LOCK.blocking_lock();
    // Keep auto-generated state paths short enough for the daemon unix socket
    // while preserving TestContext cleanup / PreserveOnFailure behavior.
    let _tmpdir_env = ScopedEnvVar::set("TMPDIR", "/tmp");
    let mut context = TestContext::builder("regression-call-with-pikachat-daemon")
        .artifact_policy(ArtifactPolicy::PreserveOnFailure)
        .build()?;
    support::run_call_with_pikachat_daemon(&context)?;
    context.mark_success();
    Ok(())
}
