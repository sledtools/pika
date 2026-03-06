use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use anyhow::Result;
use anyhow::{Context, anyhow, bail};
use base64::Engine;
use nostr_sdk::ToBech32;
use nostr_sdk::prelude::{EventBuilder, Keys, Kind, Tag, TagKind};
use reqwest::Method;
use reqwest::StatusCode;
use serde_json::Value;
use tokio::sync::Mutex;

use pikahut::config::ProfileName;
use pikahut::testing::{
    ArtifactPolicy, CommandRunner, CommandSpec, FixtureSpec, Requirement, TestContext, scenarios,
    scenarios::{
        CliSmokeRequest, InteropRustBaselineRequest, ScenarioRequest, UiE2eLocalRequest, UiPlatform,
    },
    skip_if_missing_requirements, start_fixture,
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

fn spawn_mock_vm_spawner(
    expected_requests: usize,
) -> Result<(String, thread::JoinHandle<Result<Vec<String>>>)> {
    let listener = TcpListener::bind("127.0.0.1:0").context("bind mock vm-spawner")?;
    let addr = listener.local_addr().context("read mock vm-spawner addr")?;
    let url = format!("http://{}", addr);

    let handle = thread::spawn(move || -> Result<Vec<String>> {
        let mut request_lines = Vec::with_capacity(expected_requests);
        for _ in 0..expected_requests {
            let (mut stream, _) = listener.accept().context("accept spawner request")?;
            stream
                .set_read_timeout(Some(Duration::from_secs(10)))
                .context("set spawner read timeout")?;

            let mut buf = Vec::new();
            let mut header_end = None;
            while header_end.is_none() {
                let mut chunk = [0u8; 1024];
                let n = stream.read(&mut chunk).context("read spawner headers")?;
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&chunk[..n]);
                if let Some(idx) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                    header_end = Some(idx + 4);
                }
            }

            let header_end = header_end.ok_or_else(|| anyhow!("missing HTTP header terminator"))?;
            let header_text = String::from_utf8_lossy(&buf[..header_end]);
            let request_line = header_text.lines().next().unwrap_or_default().to_string();

            let mut content_length = 0usize;
            for line in header_text.lines().skip(1) {
                let mut parts = line.splitn(2, ':');
                let name = parts.next().unwrap_or_default().trim();
                let value = parts.next().unwrap_or_default().trim();
                if name.eq_ignore_ascii_case("content-length") {
                    content_length = value.parse::<usize>().unwrap_or(0);
                }
            }

            let already_body = buf.len().saturating_sub(header_end);
            if content_length > already_body {
                let mut remaining = vec![0u8; content_length - already_body];
                stream
                    .read_exact(&mut remaining)
                    .context("read spawner request body")?;
                buf.extend_from_slice(&remaining);
            }

            let (status, body) = if request_line.starts_with("POST /vms ") {
                (
                    "200 OK",
                    r#"{"id":"vm-test-1","ip":"10.0.0.2","status":"starting"}"#,
                )
            } else if request_line.starts_with("GET /vms/vm-test-1 ") {
                ("200 OK", r#"{"id":"vm-test-1","status":"running"}"#)
            } else if request_line.starts_with("POST /vms/vm-test-1/recover ") {
                (
                    "200 OK",
                    r#"{"id":"vm-test-1","ip":"10.0.0.2","status":"running"}"#,
                )
            } else {
                ("404 Not Found", r#"{"error":"unexpected path"}"#)
            };

            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream
                .write_all(response.as_bytes())
                .context("write spawner response")?;
            stream.flush().context("flush spawner response")?;

            request_lines.push(request_line);
        }

        Ok(request_lines)
    });

    Ok((url, handle))
}

fn build_nip98_authorization_header(keys: &Keys, method: Method, url: &str) -> Result<String> {
    let event = EventBuilder::new(Kind::Custom(27235), "")
        .tags([
            Tag::custom(TagKind::custom("u"), [url]),
            Tag::custom(
                TagKind::custom("method"),
                [method.as_str().to_ascii_uppercase()],
            ),
        ])
        .sign_with_keys(keys)
        .context("sign NIP-98 event")?;
    let payload = serde_json::to_vec(&event).context("serialize NIP-98 event")?;
    let encoded = base64::engine::general_purpose::STANDARD.encode(payload);
    Ok(format!("Nostr {encoded}"))
}

fn insert_agent_allowlist_row(database_url: &str, npub: &str) -> Result<()> {
    let escaped_npub = npub.replace('\'', "''");
    let sql = format!(
        "INSERT INTO agent_allowlist (npub, active, note, updated_by, updated_at) \
         VALUES ('{escaped_npub}', TRUE, 'deterministic', '{escaped_npub}', now()) \
         ON CONFLICT (npub) DO UPDATE \
         SET active = EXCLUDED.active, note = EXCLUDED.note, updated_by = EXCLUDED.updated_by, updated_at = now();"
    );
    let output = std::process::Command::new("psql")
        .args(["-v", "ON_ERROR_STOP=1", "-d", database_url, "-c", &sql])
        .output()
        .context("run psql to upsert agent allowlist")?;
    if !output.status.success() {
        bail!(
            "psql failed upserting agent allowlist row: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

fn update_agent_instance_agent_id(
    database_url: &str,
    owner_npub: &str,
    agent_npub: &str,
) -> Result<()> {
    let escaped_owner = owner_npub.replace('\'', "''");
    let escaped_agent = agent_npub.replace('\'', "''");
    let sql = format!(
        "UPDATE agent_instances \
         SET agent_id = '{escaped_agent}', updated_at = now() \
         WHERE owner_npub = '{escaped_owner}' AND phase IN ('creating', 'ready');"
    );
    let output = std::process::Command::new("psql")
        .args(["-v", "ON_ERROR_STOP=1", "-d", database_url, "-c", &sql])
        .output()
        .context("run psql to update agent_instance agent_id")?;
    if !output.status.success() {
        bail!(
            "psql failed updating agent_instances.agent_id: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

fn run_pikachat(
    runner: &CommandRunner,
    args: &[String],
    capture_name: &str,
) -> Result<pikahut::testing::CommandOutput> {
    runner.run(
        &CommandSpec::cargo()
            .cwd(workspace_root())
            .args(["run", "-q", "-p", "pikachat", "--"])
            .args(args.iter().cloned())
            .timeout(Duration::from_secs(30))
            .capture_name(capture_name),
    )
}

fn run_pikachat_json(runner: &CommandRunner, args: &[String], capture_name: &str) -> Result<Value> {
    let output = run_pikachat(runner, args, capture_name)?;
    serde_json::from_slice(&output.stdout).context("decode pikachat json output")
}

fn read_text(path: &Path) -> Result<String> {
    std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))
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

#[test]
#[ignore = "nightly call-path regression selector"]
fn call_over_local_moq_relay_boundary() -> Result<()> {
    let mut context = TestContext::builder("regression-call-over-local-moq-relay")
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
                "e2e_calls",
                "call_over_local_moq_relay",
                "--",
                "--ignored",
                "--nocapture",
            ])
            .capture_name("regression-call-over-local-moq-relay"),
    )?;
    context.mark_success();
    Ok(())
}

#[test]
#[ignore = "nightly call-path regression selector"]
fn call_with_pikachat_daemon_boundary() -> Result<()> {
    let mut context = TestContext::builder("regression-call-with-pikachat-daemon")
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
                "e2e_calls",
                "call_with_pikachat_daemon",
                "--",
                "--ignored",
                "--nocapture",
            ])
            .capture_name("regression-call-with-pikachat-daemon"),
    )?;
    context.mark_success();
    Ok(())
}

#[tokio::test]
#[ignore = "integration scenario; run in deterministic lane"]
async fn agent_http_ensure_local() -> Result<()> {
    let _env_lock = ENV_LOCK.lock().await;
    // Keep temp paths short enough for postgres unix socket limits while preserving
    // TestContext auto-state lifecycle (including PreserveOnFailure behavior).
    let _tmpdir_env = ScopedEnvVar::set("TMPDIR", "/tmp");
    let mut context = TestContext::builder("agent-http-ensure-local")
        .artifact_policy(ArtifactPolicy::PreserveOnFailure)
        .build()?;

    let (spawner_url, spawner_thread) = spawn_mock_vm_spawner(2)?;
    let owner_keys = Keys::generate();
    let owner_npub = owner_keys
        .public_key()
        .to_bech32()
        .context("encode owner npub")?;

    let _spawner_env = ScopedEnvVar::set("PIKA_AGENT_MICROVM_SPAWNER_URL", &spawner_url);
    let _admin_bootstrap_env = ScopedEnvVar::set("PIKA_ADMIN_BOOTSTRAP_NPUBS", &owner_npub);
    let _admin_secret_env = ScopedEnvVar::set(
        "PIKA_ADMIN_SESSION_SECRET",
        "pikahut-deterministic-admin-secret",
    );

    let fixture = start_fixture(
        &context,
        &FixtureSpec::builder(ProfileName::Backend)
            .moq_port(0)
            .server_port(0)
            .build(),
    )
    .await?;
    let server_url = fixture
        .server_url()
        .ok_or_else(|| anyhow!("fixture manifest missing server_url"))?
        .to_string();
    let database_url = fixture
        .manifest()
        .database_url
        .as_deref()
        .ok_or_else(|| anyhow!("fixture manifest missing database_url"))?
        .to_string();

    insert_agent_allowlist_row(&database_url, &owner_npub)?;

    let client = reqwest::Client::new();
    let ensure_url = format!("{server_url}/v1/agents/ensure");
    let ensure_auth = build_nip98_authorization_header(&owner_keys, Method::POST, &ensure_url)?;
    let ensure_resp = client
        .post(&ensure_url)
        .header("Authorization", ensure_auth)
        .json(&serde_json::json!({}))
        .send()
        .await
        .context("send /v1/agents/ensure")?;
    let ensure_status = ensure_resp.status();
    let ensure_body = ensure_resp.text().await.unwrap_or_default();
    if ensure_status != StatusCode::ACCEPTED {
        bail!("expected 202 from ensure, got {ensure_status}: {ensure_body}");
    }
    let ensure_json: Value =
        serde_json::from_str(&ensure_body).context("decode ensure response json")?;
    let state = ensure_json
        .get("state")
        .and_then(Value::as_str)
        .unwrap_or("");
    if state != "creating" {
        bail!("expected state=creating after ensure, got: {state}");
    }
    let agent_id = ensure_json
        .get("agent_id")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("ensure response missing agent_id"))?;
    if !agent_id.starts_with("npub1") {
        bail!("ensure returned unexpected agent_id format: {agent_id}");
    }
    let vm_id = ensure_json
        .get("vm_id")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("ensure response missing vm_id"))?;
    if vm_id != "vm-test-1" {
        bail!("expected vm_id=vm-test-1, got: {vm_id}");
    }

    let me_url = format!("{server_url}/v1/agents/me");
    let me_auth = build_nip98_authorization_header(&owner_keys, Method::GET, &me_url)?;
    let me_resp = client
        .get(&me_url)
        .header("Authorization", me_auth)
        .send()
        .await
        .context("send /v1/agents/me")?;
    let me_status = me_resp.status();
    let me_body = me_resp.text().await.unwrap_or_default();
    if me_status != StatusCode::OK {
        bail!("expected 200 from /v1/agents/me, got {me_status}: {me_body}");
    }
    let me_json: Value = serde_json::from_str(&me_body).context("decode /me response json")?;
    let me_agent_id = me_json
        .get("agent_id")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("/me response missing agent_id"))?;
    if me_agent_id != agent_id {
        bail!("agent_id mismatch between ensure and /me");
    }
    let me_state = me_json.get("state").and_then(Value::as_str).unwrap_or("");
    if me_state != "ready" {
        bail!("expected state=ready from /v1/agents/me, got: {me_state}");
    }

    let spawner_request_lines = spawner_thread
        .join()
        .map_err(|_| anyhow!("mock vm-spawner thread panicked"))??;
    let spawner_request_line = spawner_request_lines
        .first()
        .ok_or_else(|| anyhow!("mock vm-spawner captured no requests"))?;
    if !spawner_request_line.starts_with("POST /vms ") {
        bail!("expected vm-spawner POST /vms, got: {spawner_request_line}");
    }

    context.mark_success();
    Ok(())
}

#[tokio::test]
#[ignore = "integration scenario; run in deterministic lane"]
async fn agent_http_cli_new_local() -> Result<()> {
    let _env_lock = ENV_LOCK.lock().await;
    // Keep temp paths short enough for postgres unix socket limits while preserving
    // TestContext auto-state lifecycle (including PreserveOnFailure behavior).
    let _tmpdir_env = ScopedEnvVar::set("TMPDIR", "/tmp");
    let mut context = TestContext::builder("agent-http-cli-new-local")
        .artifact_policy(ArtifactPolicy::PreserveOnFailure)
        .build()?;

    let (spawner_url, spawner_thread) = spawn_mock_vm_spawner(2)?;
    let owner_keys = Keys::generate();
    let owner_npub = owner_keys
        .public_key()
        .to_bech32()
        .context("encode owner npub")?;
    let owner_nsec = owner_keys.secret_key().to_secret_hex();

    let _spawner_env = ScopedEnvVar::set("PIKA_AGENT_MICROVM_SPAWNER_URL", &spawner_url);
    let _admin_bootstrap_env = ScopedEnvVar::set("PIKA_ADMIN_BOOTSTRAP_NPUBS", &owner_npub);
    let _admin_secret_env = ScopedEnvVar::set(
        "PIKA_ADMIN_SESSION_SECRET",
        "pikahut-deterministic-admin-secret",
    );

    let fixture = start_fixture(
        &context,
        &FixtureSpec::builder(ProfileName::Backend)
            .moq_port(0)
            .server_port(0)
            .build(),
    )
    .await?;
    let server_url = fixture
        .server_url()
        .ok_or_else(|| anyhow!("fixture manifest missing server_url"))?
        .to_string();
    let database_url = fixture
        .manifest()
        .database_url
        .as_deref()
        .ok_or_else(|| anyhow!("fixture manifest missing database_url"))?
        .to_string();

    insert_agent_allowlist_row(&database_url, &owner_npub)?;

    let runner = CommandRunner::new(&context);
    let output = runner.run(
        &CommandSpec::cargo()
            .cwd(workspace_root())
            .args([
                "run",
                "-q",
                "-p",
                "pikachat",
                "--",
                "agent",
                "new",
                "--api-base-url",
                &server_url,
                "--nsec",
                &owner_nsec,
            ])
            .capture_name("agent-http-cli-new"),
    )?;
    let stdout = String::from_utf8(output.stdout).context("decode pikachat stdout")?;
    let cli_json: Value = serde_json::from_str(stdout.trim()).context("decode cli json output")?;
    if cli_json.get("operation").and_then(Value::as_str) != Some("ensure") {
        bail!("unexpected CLI operation payload: {cli_json}");
    }
    let state = cli_json
        .get("agent")
        .and_then(|value| value.get("state"))
        .and_then(Value::as_str)
        .unwrap_or("");
    if state != "creating" {
        bail!("expected CLI ensure state=creating, got: {state}");
    }

    let me_url = format!("{server_url}/v1/agents/me");
    let me_auth = build_nip98_authorization_header(&owner_keys, Method::GET, &me_url)?;
    let me_resp = reqwest::Client::new()
        .get(&me_url)
        .header("Authorization", me_auth)
        .send()
        .await
        .context("send /v1/agents/me")?;
    if me_resp.status() != StatusCode::OK {
        let body = me_resp.text().await.unwrap_or_default();
        bail!("expected 200 from /v1/agents/me after CLI ensure, got body: {body}");
    }
    let me_json: Value = me_resp.json().await.context("decode /v1/agents/me json")?;
    let me_state = me_json.get("state").and_then(Value::as_str).unwrap_or("");
    if me_state != "ready" {
        bail!("expected /v1/agents/me state=ready after CLI ensure, got: {me_state}");
    }

    let spawner_request_lines = spawner_thread
        .join()
        .map_err(|_| anyhow!("mock vm-spawner thread panicked"))??;
    if spawner_request_lines.len() != 2 {
        bail!("expected two vm-spawner requests, got: {spawner_request_lines:?}");
    }
    if !spawner_request_lines[0].starts_with("POST /vms ") {
        bail!(
            "expected vm-spawner POST /vms, got: {}",
            spawner_request_lines[0]
        );
    }
    if !spawner_request_lines[1].starts_with("GET /vms/vm-test-1 ") {
        bail!(
            "expected vm-spawner GET /vms/vm-test-1, got: {}",
            spawner_request_lines[1]
        );
    }

    context.mark_success();
    Ok(())
}

#[tokio::test]
#[ignore = "integration scenario; run in deterministic lane"]
async fn agent_http_cli_new_idempotent_local() -> Result<()> {
    let _env_lock = ENV_LOCK.lock().await;
    let _tmpdir_env = ScopedEnvVar::set("TMPDIR", "/tmp");
    let mut context = TestContext::builder("agent-http-cli-new-idempotent-local")
        .artifact_policy(ArtifactPolicy::PreserveOnFailure)
        .build()?;

    let (spawner_url, spawner_thread) = spawn_mock_vm_spawner(2)?;
    let owner_keys = Keys::generate();
    let owner_npub = owner_keys
        .public_key()
        .to_bech32()
        .context("encode owner npub")?;
    let owner_nsec = owner_keys.secret_key().to_secret_hex();

    let _spawner_env = ScopedEnvVar::set("PIKA_AGENT_MICROVM_SPAWNER_URL", &spawner_url);
    let _admin_bootstrap_env = ScopedEnvVar::set("PIKA_ADMIN_BOOTSTRAP_NPUBS", &owner_npub);
    let _admin_secret_env = ScopedEnvVar::set(
        "PIKA_ADMIN_SESSION_SECRET",
        "pikahut-deterministic-admin-secret",
    );

    let fixture = start_fixture(
        &context,
        &FixtureSpec::builder(ProfileName::Backend)
            .moq_port(0)
            .server_port(0)
            .build(),
    )
    .await?;
    let server_url = fixture
        .server_url()
        .ok_or_else(|| anyhow!("fixture manifest missing server_url"))?
        .to_string();
    let database_url = fixture
        .manifest()
        .database_url
        .as_deref()
        .ok_or_else(|| anyhow!("fixture manifest missing database_url"))?
        .to_string();

    insert_agent_allowlist_row(&database_url, &owner_npub)?;

    let runner = CommandRunner::new(&context);

    let first_new_output = runner.run(
        &CommandSpec::cargo()
            .cwd(workspace_root())
            .args([
                "run",
                "-q",
                "-p",
                "pikachat",
                "--",
                "agent",
                "new",
                "--api-base-url",
                &server_url,
                "--nsec",
                &owner_nsec,
            ])
            .capture_name("agent-http-cli-new-idempotent-first"),
    )?;
    let first_stdout =
        String::from_utf8(first_new_output.stdout).context("decode first new stdout")?;
    let first_json: Value =
        serde_json::from_str(first_stdout.trim()).context("decode first new")?;
    if first_json.get("operation").and_then(Value::as_str) != Some("ensure") {
        bail!("unexpected first CLI new payload: {first_json}");
    }
    if first_json.get("created").and_then(Value::as_bool) != Some(true) {
        bail!("expected first CLI new to set created=true, got: {first_json}");
    }
    let first_agent_id = first_json
        .get("agent")
        .and_then(|value| value.get("agent_id"))
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("first CLI new missing agent.agent_id"))?;
    let first_state = first_json
        .get("agent")
        .and_then(|value| value.get("state"))
        .and_then(Value::as_str)
        .unwrap_or("");
    if first_state != "creating" {
        bail!("expected first CLI new state=creating, got: {first_state}");
    }

    let second_new_output = runner.run(
        &CommandSpec::cargo()
            .cwd(workspace_root())
            .args([
                "run",
                "-q",
                "-p",
                "pikachat",
                "--",
                "agent",
                "new",
                "--api-base-url",
                &server_url,
                "--nsec",
                &owner_nsec,
            ])
            .capture_name("agent-http-cli-new-idempotent-second"),
    )?;
    let second_stdout =
        String::from_utf8(second_new_output.stdout).context("decode second new stdout")?;
    let second_json: Value =
        serde_json::from_str(second_stdout.trim()).context("decode second new")?;
    if second_json.get("operation").and_then(Value::as_str) != Some("ensure") {
        bail!("unexpected second CLI new payload: {second_json}");
    }
    if second_json.get("created").and_then(Value::as_bool) != Some(false) {
        bail!("expected second CLI new to set created=false, got: {second_json}");
    }
    let second_agent_id = second_json
        .get("agent")
        .and_then(|value| value.get("agent_id"))
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("second CLI new missing agent.agent_id"))?;
    if second_agent_id != first_agent_id {
        bail!("expected same agent_id across idempotent new calls");
    }
    let second_state = second_json
        .get("agent")
        .and_then(|value| value.get("state"))
        .and_then(Value::as_str)
        .unwrap_or("");
    if second_state != "ready" {
        bail!("expected second CLI new state=ready after /me lookup, got: {second_state}");
    }

    let spawner_request_lines = spawner_thread
        .join()
        .map_err(|_| anyhow!("mock vm-spawner thread panicked"))??;
    if spawner_request_lines.len() != 2 {
        bail!("expected exactly two vm-spawner requests, got: {spawner_request_lines:?}");
    }
    if !spawner_request_lines[0].starts_with("POST /vms ") {
        bail!(
            "expected vm-spawner POST /vms on first ensure, got: {}",
            spawner_request_lines[0]
        );
    }
    if !spawner_request_lines[1].starts_with("GET /vms/vm-test-1 ") {
        bail!(
            "expected vm-spawner GET /vms/vm-test-1 on idempotent ensure, got: {}",
            spawner_request_lines[1]
        );
    }

    context.mark_success();
    Ok(())
}

#[tokio::test]
#[ignore = "integration scenario; run in deterministic lane"]
async fn agent_http_cli_new_me_recover_local() -> Result<()> {
    let _env_lock = ENV_LOCK.lock().await;
    let _tmpdir_env = ScopedEnvVar::set("TMPDIR", "/tmp");
    let mut context = TestContext::builder("agent-http-cli-new-me-recover-local")
        .artifact_policy(ArtifactPolicy::PreserveOnFailure)
        .build()?;

    let (spawner_url, spawner_thread) = spawn_mock_vm_spawner(3)?;
    let owner_keys = Keys::generate();
    let owner_npub = owner_keys
        .public_key()
        .to_bech32()
        .context("encode owner npub")?;
    let owner_nsec = owner_keys.secret_key().to_secret_hex();

    let _spawner_env = ScopedEnvVar::set("PIKA_AGENT_MICROVM_SPAWNER_URL", &spawner_url);
    let _admin_bootstrap_env = ScopedEnvVar::set("PIKA_ADMIN_BOOTSTRAP_NPUBS", &owner_npub);
    let _admin_secret_env = ScopedEnvVar::set(
        "PIKA_ADMIN_SESSION_SECRET",
        "pikahut-deterministic-admin-secret",
    );

    let fixture = start_fixture(
        &context,
        &FixtureSpec::builder(ProfileName::Backend)
            .moq_port(0)
            .server_port(0)
            .build(),
    )
    .await?;
    let server_url = fixture
        .server_url()
        .ok_or_else(|| anyhow!("fixture manifest missing server_url"))?
        .to_string();
    let database_url = fixture
        .manifest()
        .database_url
        .as_deref()
        .ok_or_else(|| anyhow!("fixture manifest missing database_url"))?
        .to_string();

    insert_agent_allowlist_row(&database_url, &owner_npub)?;

    let runner = CommandRunner::new(&context);
    let new_output = runner.run(
        &CommandSpec::cargo()
            .cwd(workspace_root())
            .args([
                "run",
                "-q",
                "-p",
                "pikachat",
                "--",
                "agent",
                "new",
                "--api-base-url",
                &server_url,
                "--nsec",
                &owner_nsec,
            ])
            .capture_name("agent-http-cli-new-me-recover-new"),
    )?;
    let new_stdout = String::from_utf8(new_output.stdout).context("decode pikachat new stdout")?;
    let new_json: Value = serde_json::from_str(new_stdout.trim()).context("decode new json")?;
    if new_json.get("operation").and_then(Value::as_str) != Some("ensure") {
        bail!("unexpected CLI new payload: {new_json}");
    }
    let new_state = new_json
        .get("agent")
        .and_then(|value| value.get("state"))
        .and_then(Value::as_str)
        .unwrap_or("");
    if new_state != "creating" {
        bail!("expected CLI new state=creating, got: {new_state}");
    }

    let me_output = runner.run(
        &CommandSpec::cargo()
            .cwd(workspace_root())
            .args([
                "run",
                "-q",
                "-p",
                "pikachat",
                "--",
                "agent",
                "me",
                "--api-base-url",
                &server_url,
                "--nsec",
                &owner_nsec,
            ])
            .capture_name("agent-http-cli-new-me-recover-me"),
    )?;
    let me_stdout = String::from_utf8(me_output.stdout).context("decode pikachat me stdout")?;
    let me_json: Value = serde_json::from_str(me_stdout.trim()).context("decode me json")?;
    if me_json.get("operation").and_then(Value::as_str) != Some("me") {
        bail!("unexpected CLI me payload: {me_json}");
    }
    let me_state = me_json
        .get("agent")
        .and_then(|value| value.get("state"))
        .and_then(Value::as_str)
        .unwrap_or("");
    if me_state != "ready" {
        bail!("expected CLI me state=ready, got: {me_state}");
    }

    let recover_output = runner.run(
        &CommandSpec::cargo()
            .cwd(workspace_root())
            .args([
                "run",
                "-q",
                "-p",
                "pikachat",
                "--",
                "agent",
                "recover",
                "--api-base-url",
                &server_url,
                "--nsec",
                &owner_nsec,
            ])
            .capture_name("agent-http-cli-new-me-recover-recover"),
    )?;
    let recover_stdout =
        String::from_utf8(recover_output.stdout).context("decode pikachat recover stdout")?;
    let recover_json: Value =
        serde_json::from_str(recover_stdout.trim()).context("decode recover json")?;
    if recover_json.get("operation").and_then(Value::as_str) != Some("recover") {
        bail!("unexpected CLI recover payload: {recover_json}");
    }
    let recover_state = recover_json
        .get("agent")
        .and_then(|value| value.get("state"))
        .and_then(Value::as_str)
        .unwrap_or("");
    if recover_state != "ready" {
        bail!("expected CLI recover state=ready, got: {recover_state}");
    }

    let spawner_request_lines = spawner_thread
        .join()
        .map_err(|_| anyhow!("mock vm-spawner thread panicked"))??;
    if !spawner_request_lines
        .iter()
        .any(|line| line.starts_with("POST /vms "))
    {
        bail!("expected vm-spawner POST /vms request, got: {spawner_request_lines:?}");
    }
    if !spawner_request_lines
        .iter()
        .any(|line| line.starts_with("POST /vms/vm-test-1/recover "))
    {
        bail!(
            "expected vm-spawner POST /vms/vm-test-1/recover request, got: {spawner_request_lines:?}"
        );
    }
    if !spawner_request_lines
        .iter()
        .any(|line| line.starts_with("GET /vms/vm-test-1 "))
    {
        bail!("expected vm-spawner GET /vms/vm-test-1 request, got: {spawner_request_lines:?}");
    }

    context.mark_success();
    Ok(())
}

#[tokio::test]
#[ignore = "integration scenario; run in deterministic lane"]
async fn agent_http_cli_chat_reply_local() -> Result<()> {
    let _env_lock = ENV_LOCK.lock().await;
    let _tmpdir_env = ScopedEnvVar::set("TMPDIR", "/tmp");
    let mut context = TestContext::builder("agent-http-cli-chat-reply-local")
        .artifact_policy(ArtifactPolicy::PreserveOnFailure)
        .build()?;

    let (spawner_url, spawner_thread) = spawn_mock_vm_spawner(3)?;
    let owner_keys = Keys::generate();
    let owner_npub = owner_keys
        .public_key()
        .to_bech32()
        .context("encode owner npub")?;
    let owner_nsec = owner_keys.secret_key().to_secret_hex();
    let fake_agent_keys = Keys::generate();
    let fake_agent_npub = fake_agent_keys
        .public_key()
        .to_bech32()
        .context("encode fake agent npub")?;
    let fake_agent_nsec = fake_agent_keys.secret_key().to_secret_hex();

    let _spawner_env = ScopedEnvVar::set("PIKA_AGENT_MICROVM_SPAWNER_URL", &spawner_url);
    let _admin_bootstrap_env = ScopedEnvVar::set("PIKA_ADMIN_BOOTSTRAP_NPUBS", &owner_npub);
    let _admin_secret_env = ScopedEnvVar::set(
        "PIKA_ADMIN_SESSION_SECRET",
        "pikahut-deterministic-admin-secret",
    );

    let fixture = start_fixture(
        &context,
        &FixtureSpec::builder(ProfileName::Backend).build(),
    )
    .await?;
    let server_url = fixture
        .server_url()
        .ok_or_else(|| anyhow!("fixture manifest missing server_url"))?
        .to_string();
    let relay_url = fixture
        .relay_url()
        .ok_or_else(|| anyhow!("fixture manifest missing relay_url"))?
        .to_string();
    let database_url = fixture
        .manifest()
        .database_url
        .as_deref()
        .ok_or_else(|| anyhow!("fixture manifest missing database_url"))?
        .to_string();

    insert_agent_allowlist_row(&database_url, &owner_npub)?;

    let runner = CommandRunner::new(&context);
    let owner_state = context.state_dir().join("agent-chat-owner");
    let fake_agent_state = context.state_dir().join("agent-chat-fake-agent");

    run_pikachat(
        &runner,
        &[
            "--state-dir".to_string(),
            fake_agent_state.display().to_string(),
            "--relay".to_string(),
            relay_url.clone(),
            "--kp-relay".to_string(),
            relay_url.clone(),
            "init".to_string(),
            "--nsec".to_string(),
            fake_agent_nsec.clone(),
        ],
        "agent-http-cli-chat-reply-fake-init",
    )?;

    let new_json = run_pikachat_json(
        &runner,
        &[
            "--state-dir".to_string(),
            owner_state.display().to_string(),
            "--relay".to_string(),
            relay_url.clone(),
            "--kp-relay".to_string(),
            relay_url.clone(),
            "agent".to_string(),
            "new".to_string(),
            "--api-base-url".to_string(),
            server_url.clone(),
            "--nsec".to_string(),
            owner_nsec.clone(),
        ],
        "agent-http-cli-chat-reply-new",
    )?;
    let new_state = new_json
        .get("agent")
        .and_then(|value| value.get("state"))
        .and_then(Value::as_str)
        .unwrap_or("");
    if new_state != "creating" {
        bail!("expected CLI new state=creating before chat, got: {new_state}");
    }

    update_agent_instance_agent_id(&database_url, &owner_npub, &fake_agent_npub)?;

    let chat_spec = CommandSpec::cargo()
        .cwd(workspace_root())
        .args(["run", "-q", "-p", "pikachat", "--"])
        .args([
            "--state-dir".to_string(),
            owner_state.display().to_string(),
            "--relay".to_string(),
            relay_url.clone(),
            "--kp-relay".to_string(),
            relay_url.clone(),
            "agent".to_string(),
            "chat".to_string(),
            "--api-base-url".to_string(),
            server_url.clone(),
            "--nsec".to_string(),
            owner_nsec.clone(),
            "--listen-timeout".to_string(),
            "10".to_string(),
            "--poll-attempts".to_string(),
            "4".to_string(),
            "--poll-delay-sec".to_string(),
            "1".to_string(),
            "hello from owner".to_string(),
        ])
        .capture_name("agent-http-cli-chat-reply");
    let mut chat_handle = runner.spawn(&chat_spec)?;

    tokio::time::sleep(Duration::from_secs(1)).await;

    run_pikachat(
        &runner,
        &[
            "--state-dir".to_string(),
            fake_agent_state.display().to_string(),
            "--relay".to_string(),
            relay_url.clone(),
            "listen".to_string(),
            "--timeout".to_string(),
            "3".to_string(),
            "--lookback".to_string(),
            "300".to_string(),
        ],
        "agent-http-cli-chat-reply-fake-listen",
    )?;
    let welcomes_json = run_pikachat_json(
        &runner,
        &[
            "--state-dir".to_string(),
            fake_agent_state.display().to_string(),
            "--relay".to_string(),
            relay_url.clone(),
            "welcomes".to_string(),
        ],
        "agent-http-cli-chat-reply-fake-welcomes",
    )?;
    let wrapper_event_id = welcomes_json
        .get("welcomes")
        .and_then(Value::as_array)
        .and_then(|welcomes| welcomes.first())
        .and_then(|welcome| welcome.get("wrapper_event_id"))
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("fake agent welcomes missing wrapper_event_id"))?
        .to_string();
    run_pikachat(
        &runner,
        &[
            "--state-dir".to_string(),
            fake_agent_state.display().to_string(),
            "--relay".to_string(),
            relay_url.clone(),
            "accept-welcome".to_string(),
            "--wrapper-event-id".to_string(),
            wrapper_event_id,
        ],
        "agent-http-cli-chat-reply-fake-accept",
    )?;
    run_pikachat(
        &runner,
        &[
            "--state-dir".to_string(),
            fake_agent_state.display().to_string(),
            "--relay".to_string(),
            relay_url.clone(),
            "--kp-relay".to_string(),
            relay_url.clone(),
            "send".to_string(),
            "--to".to_string(),
            owner_npub.clone(),
            "--content".to_string(),
            "reply from fake agent".to_string(),
        ],
        "agent-http-cli-chat-reply-fake-send",
    )?;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    let chat_status = loop {
        if let Some(status) = chat_handle.try_wait()? {
            break status;
        }
        if tokio::time::Instant::now() >= deadline {
            chat_handle.kill()?;
            bail!("timed out waiting for agent chat reply command to finish");
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    if !chat_status.success() {
        let stderr = read_text(&chat_handle.stderr_path)?;
        bail!("expected successful agent chat reply command, stderr: {stderr}");
    }
    let chat_stdout = read_text(&chat_handle.stdout_path)?;
    if !chat_stdout.contains("reply from fake agent") {
        bail!("expected agent chat stdout to include reply content, got: {chat_stdout}");
    }

    let spawner_request_lines = spawner_thread
        .join()
        .map_err(|_| anyhow!("mock vm-spawner thread panicked"))??;
    if !spawner_request_lines
        .iter()
        .any(|line| line.starts_with("POST /vms "))
    {
        bail!("expected vm-spawner POST /vms request, got: {spawner_request_lines:?}");
    }
    let get_count = spawner_request_lines
        .iter()
        .filter(|line| line.starts_with("GET /vms/vm-test-1 "))
        .count();
    if get_count < 2 {
        bail!(
            "expected repeated vm-spawner GET /vms/vm-test-1 readiness checks, got: {spawner_request_lines:?}"
        );
    }

    context.mark_success();
    Ok(())
}

#[tokio::test]
#[ignore = "integration scenario; run in deterministic lane"]
async fn agent_http_cli_chat_no_reply_timeout_local() -> Result<()> {
    let _env_lock = ENV_LOCK.lock().await;
    let _tmpdir_env = ScopedEnvVar::set("TMPDIR", "/tmp");
    let mut context = TestContext::builder("agent-http-cli-chat-no-reply-timeout-local")
        .artifact_policy(ArtifactPolicy::PreserveOnFailure)
        .build()?;

    let (spawner_url, spawner_thread) = spawn_mock_vm_spawner(3)?;
    let owner_keys = Keys::generate();
    let owner_npub = owner_keys
        .public_key()
        .to_bech32()
        .context("encode owner npub")?;
    let owner_nsec = owner_keys.secret_key().to_secret_hex();
    let fake_agent_keys = Keys::generate();
    let fake_agent_npub = fake_agent_keys
        .public_key()
        .to_bech32()
        .context("encode fake agent npub")?;
    let fake_agent_nsec = fake_agent_keys.secret_key().to_secret_hex();

    let _spawner_env = ScopedEnvVar::set("PIKA_AGENT_MICROVM_SPAWNER_URL", &spawner_url);
    let _admin_bootstrap_env = ScopedEnvVar::set("PIKA_ADMIN_BOOTSTRAP_NPUBS", &owner_npub);
    let _admin_secret_env = ScopedEnvVar::set(
        "PIKA_ADMIN_SESSION_SECRET",
        "pikahut-deterministic-admin-secret",
    );

    let fixture = start_fixture(
        &context,
        &FixtureSpec::builder(ProfileName::Backend).build(),
    )
    .await?;
    let server_url = fixture
        .server_url()
        .ok_or_else(|| anyhow!("fixture manifest missing server_url"))?
        .to_string();
    let relay_url = fixture
        .relay_url()
        .ok_or_else(|| anyhow!("fixture manifest missing relay_url"))?
        .to_string();
    let database_url = fixture
        .manifest()
        .database_url
        .as_deref()
        .ok_or_else(|| anyhow!("fixture manifest missing database_url"))?
        .to_string();

    insert_agent_allowlist_row(&database_url, &owner_npub)?;

    let runner = CommandRunner::new(&context);
    let owner_state = context.state_dir().join("agent-chat-timeout-owner");
    let fake_agent_state = context.state_dir().join("agent-chat-timeout-fake-agent");

    run_pikachat(
        &runner,
        &[
            "--state-dir".to_string(),
            fake_agent_state.display().to_string(),
            "--relay".to_string(),
            relay_url.clone(),
            "--kp-relay".to_string(),
            relay_url.clone(),
            "init".to_string(),
            "--nsec".to_string(),
            fake_agent_nsec,
        ],
        "agent-http-cli-chat-timeout-fake-init",
    )?;
    run_pikachat(
        &runner,
        &[
            "--state-dir".to_string(),
            owner_state.display().to_string(),
            "--relay".to_string(),
            relay_url.clone(),
            "--kp-relay".to_string(),
            relay_url.clone(),
            "agent".to_string(),
            "new".to_string(),
            "--api-base-url".to_string(),
            server_url.clone(),
            "--nsec".to_string(),
            owner_nsec.clone(),
        ],
        "agent-http-cli-chat-timeout-new",
    )?;
    update_agent_instance_agent_id(&database_url, &owner_npub, &fake_agent_npub)?;

    let err = run_pikachat(
        &runner,
        &[
            "--state-dir".to_string(),
            owner_state.display().to_string(),
            "--relay".to_string(),
            relay_url.clone(),
            "--kp-relay".to_string(),
            relay_url.clone(),
            "agent".to_string(),
            "chat".to_string(),
            "--api-base-url".to_string(),
            server_url.clone(),
            "--nsec".to_string(),
            owner_nsec,
            "--listen-timeout".to_string(),
            "2".to_string(),
            "--poll-attempts".to_string(),
            "4".to_string(),
            "--poll-delay-sec".to_string(),
            "1".to_string(),
            "hello with no reply".to_string(),
        ],
        "agent-http-cli-chat-timeout",
    )
    .expect_err("agent chat without reply must fail");
    if !err.to_string().contains("no_reply_within_timeout") {
        bail!("expected no_reply_within_timeout error, got: {err:#}");
    }

    let spawner_request_lines = spawner_thread
        .join()
        .map_err(|_| anyhow!("mock vm-spawner thread panicked"))??;
    let get_count = spawner_request_lines
        .iter()
        .filter(|line| line.starts_with("GET /vms/vm-test-1 "))
        .count();
    if get_count < 2 {
        bail!(
            "expected repeated vm-spawner GET /vms/vm-test-1 readiness checks, got: {spawner_request_lines:?}"
        );
    }

    context.mark_success();
    Ok(())
}
